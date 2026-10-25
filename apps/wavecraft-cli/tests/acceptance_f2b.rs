//! F2b acceptance (models), through the CLI.
//!
//! - Practice preview (no model; Signalsmith Stretch is linked): the acceptance song at 80 % and
//!   +2 semitones lasts 1.25 × as long, reads B minor and 76.8 ± 2 BPM.
//! - Ignored (needs the downloaded models and macOS `say`): a synthetic 4-stem song — drums, bass,
//!   pad and an Italian TTS voice whose words are placed at known times — is separated (vocal SDR ≥
//!   6 dB against the voice) and transcribed from the vocals stem (≥ 90 % of the words found, in
//!   order, starting within 0.5 s). Run with
//!   `cargo test --release -p wavecraft-cli --test acceptance_f2b -- --ignored --nocapture`.

use serde_json::Value;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_wave_analysis::testsignal::{acceptance_song, chord, drums, note, sdr, song};
use std::path::{Path, PathBuf};
use std::process::Command;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("wavecraft-cli-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn script(dir: &Path, text: &str) -> Vec<(String, Value)> {
    let path = dir.join("f2b.wavecraft");
    std::fs::write(&path, text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft-cli")).arg("script").arg(&path).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    stdout.lines().filter_map(|l| l.split_once(": ").and_then(|(id, j)| serde_json::from_str(j).ok().map(|v| (id.to_string(), v)))).collect()
}

fn result<'a>(results: &'a [(String, Value)], id: &str) -> &'a Value {
    &results.iter().rev().find(|(i, _)| i == id).unwrap_or_else(|| panic!("no {id} in {results:?}")).1
}

fn write_wav(path: &Path, sample_rate: u32, channels: Vec<Vec<f32>>) {
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    std::fs::write(path, soundcraft_audio_io::encode(&AudioBuffer { sample_rate, channels }, &opts).unwrap()).unwrap();
}

fn read_wav(path: &Path) -> (u32, Vec<Vec<f32>>) {
    let (info, buf) = soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), Some("wav")).unwrap();
    (info.sample_rate, buf.channels)
}

#[test]
fn practice_preview_on_the_synthetic_song() {
    let d = dir("acceptance_f2b_practice");
    let spec = acceptance_song();
    let (channels, _) = song(&spec);
    let src = d.join("song.wav");
    write_wav(&src, spec.sample_rate, channels);
    let out = d.join("practice.wav");
    script(
        &d,
        &format!(
            "wave.open {{\"path\": \"{}\"}}\n\
             wave.practice {{\"tempo\": 80, \"semitones\": 2}}\n\
             wave.practice_export {{\"path\": \"{}\", \"bit_depth\": \"32f\"}}\n",
            src.to_str().unwrap(),
            out.to_str().unwrap()
        ),
    );
    let (_, original) = read_wav(&src);
    let (_, practised) = read_wav(&out);
    let ratio = practised[0].len() as f64 / original[0].len() as f64;
    let results = script(&d, &format!("wave.open {{\"path\": \"{}\"}}\nwave.analyze_tempo {{}}\nwave.analyze_key {{}}\n", out.to_str().unwrap()));
    let bpm = result(&results, "wave.analyze_tempo")["bpm"].as_f64().unwrap();
    let key = result(&results, "wave.analyze_key")["key"].as_str().unwrap_or_default().to_string();
    println!("practice 80 % +2: length × {ratio:.4}, {bpm:.2} BPM, key {key}");
    assert!((ratio - 1.25).abs() <= 0.0125, "length × {ratio}");
    assert!((bpm - 76.8).abs() <= 2.0, "{bpm} BPM");
    assert_eq!(key, "B minor");
}

// ---- the model acceptance --------------------------------------------------------------------

const SR: u32 = 48_000;
const BPM: f32 = 96.0;
const BARS: usize = 16;
/// Phrases of the TTS voice, each word synthesised alone and placed after the previous one.
const PHRASES: [&str; 6] = [
    "il sole sorge sul mare",
    "la notte porta le stelle",
    "il vento canta piano",
    "una voce nel silenzio",
    "il cuore batte ancora",
    "la strada torna a casa",
];
const PHRASE_EVERY_S: f32 = 6.0;
const FIRST_PHRASE_S: f32 = 2.5;
const WORD_GAP_S: f32 = 0.12;

/// One word from macOS `say` (voice Alice), trimmed of its silence, at 48 kHz.
fn tts(d: &Path, word: &str) -> Vec<f32> {
    let aiff = d.join(format!("tts-{word}.aiff"));
    let ok = Command::new("say").args(["-v", "Alice", "-o"]).arg(&aiff).arg(word).status().map(|s| s.success()).unwrap_or(false);
    assert!(ok, "`say -v Alice` failed: macOS with the Italian voice is needed");
    let (info, buf) = soundcraft_audio_io::decode(&std::fs::read(&aiff).unwrap(), Some("aiff")).unwrap();
    let x = soundcraft_wave_analysis::resample(&buf.channels[0], info.sample_rate, SR);
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let on = |v: &&f32| v.abs() > 0.02 * peak;
    let first = x.iter().position(|v| on(&v)).unwrap_or(0);
    let last = x.iter().rposition(|v| on(&v)).map_or(x.len(), |i| i + 1);
    x[first..last].iter().map(|v| 0.6 * v / peak).collect()
}

fn add(dst: &mut [f32], at: usize, src: &[f32], gain: f32) {
    for (o, v) in dst.iter_mut().skip(at).zip(src) {
        *o += v * gain;
    }
}

/// Drums, bass, pad (Am F C G) and the voice as separate mono stems, with the words' true onsets.
fn stems(d: &Path) -> ([Vec<f32>; 4], Vec<(String, f32)>) {
    let starts: Vec<f32> = (0..PHRASES.len()).map(|i| FIRST_PHRASE_S + i as f32 * PHRASE_EVERY_S).collect();
    stems_with(d, BARS, &starts)
}

/// The same band over `bars` bars, phrase `i` of [`PHRASES`] starting at `starts[i]` seconds.
fn stems_with(d: &Path, bars: usize, starts: &[f32]) -> ([Vec<f32>; 4], Vec<(String, f32)>) {
    let beat = 60.0 / BPM;
    let bar = 4.0 * beat;
    let len = ((bars as f32 * bar + 1.0) * SR as f32) as usize;
    let (mut dr, _) = drums(BPM, bars, 4, SR);
    dr.resize(len, 0.0);
    let (mut bass, mut pad) = (vec![0.0f32; len], vec![0.0f32; len]);
    let prog: [(f32, [f32; 3]); 4] = [(45.0, [57.0, 60.0, 64.0]), (41.0, [57.0, 60.0, 65.0]), (48.0, [55.0, 60.0, 64.0]), (43.0, [55.0, 59.0, 62.0])];
    for b in 0..bars {
        let (root, triad) = prog[b % 4];
        let at = (b as f32 * bar * SR as f32) as usize;
        add(&mut pad, at, &chord(&triad, bar, 440.0, SR), 0.8);
        for k in 0..2 {
            add(&mut bass, at + (k as f32 * 2.0 * beat * SR as f32) as usize, &note(root - 12.0, 2.0 * beat, 440.0, SR), 1.6);
        }
    }
    let mut voice = vec![0.0f32; len];
    let mut truth = Vec::new();
    for (phrase, &start) in PHRASES.iter().zip(starts) {
        let mut t = start;
        for word in phrase.split_whitespace() {
            let w = tts(d, word);
            add(&mut voice, (t * SR as f32) as usize, &w, 1.0);
            truth.push((word.to_string(), t));
            t += w.len() as f32 / SR as f32 + WORD_GAP_S;
        }
    }
    ([dr, bass, pad, voice], truth)
}

fn normalise(word: &str) -> String {
    word.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Truth words found in order among the heard ones, starting within 0.5 s.
fn aligned(truth: &[(String, f32)], heard: &[(String, f32)]) -> usize {
    let (n, m) = (truth.len(), heard.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            let hit = normalise(&truth[i - 1].0) == normalise(&heard[j - 1].0) && (truth[i - 1].1 - heard[j - 1].1).abs() <= 0.5;
            dp[i][j] = (dp[i - 1][j]).max(dp[i][j - 1]).max(if hit { dp[i - 1][j - 1] + 1 } else { 0 });
        }
    }
    dp[n][m]
}

#[test]
#[ignore = "needs the htdemucs and default Whisper models and macOS `say`"]
fn models_meet_the_f2_targets() {
    let d = dir("acceptance_f2b_models");
    let ([dr, bass, pad, voice], truth) = stems(&d);
    let mix: Vec<f32> = (0..voice.len()).map(|i| dr[i] + bass[i] + pad[i] + voice[i]).collect();
    let src = d.join("mix.wav");
    write_wav(&src, SR, vec![mix.clone(), mix]);
    let t0 = std::time::Instant::now();
    let results = script(&d, &format!("wave.open {{\"path\": \"{}\"}}\nwave.separate_stems {{\"model\": 4}}\n", src.to_str().unwrap()));
    let separation_s = t0.elapsed().as_secs_f32();
    let stems = result(&results, "wave.separate_stems")["stems"].as_array().unwrap().clone();
    let path_of = |name: &str| PathBuf::from(stems.iter().find(|s| s["name"] == name).unwrap()["path"].as_str().unwrap());
    let refs = [("drums", &dr), ("bass", &bass), ("other", &pad), ("vocals", &voice)];
    let mut vocal_sdr = f32::NEG_INFINITY;
    for (name, reference) in refs {
        let (_, est) = read_wav(&path_of(name));
        let s = (sdr(reference, &est[0]) + sdr(reference, &est[1])) / 2.0;
        println!("stem {name}: SDR {s:.2} dB");
        if name == "vocals" {
            vocal_sdr = s;
        }
    }
    let t1 = std::time::Instant::now();
    let results = script(
        &d,
        &format!(
            "wave.open {{\"path\": \"{}\"}}\nwave.separate_stems {{\"model\": 4}}\nwave.transcribe {{\"language\": \"it\"}}\nwave.lyrics {{}}\n",
            src.to_str().unwrap()
        ),
    );
    let transcription_s = t1.elapsed().as_secs_f32() - separation_s;
    let lyrics = result(&results, "wave.lyrics");
    let heard: Vec<(String, f32)> = lyrics["lines"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|l| l["words"].as_array().unwrap().iter().map(|w| (w["text"].as_str().unwrap().to_string(), w["start"].as_f64().unwrap() as f32)))
        .collect();
    let found = aligned(&truth, &heard);
    let share = found as f32 / truth.len() as f32;
    println!("heard: {}", heard.iter().map(|(w, t)| format!("{w}@{t:.2}")).collect::<Vec<_>>().join(" "));
    println!(
        "models: vocal SDR {vocal_sdr:.2} dB; words {found}/{} = {:.1} % within 0.5 s; separation {separation_s:.1} s, transcription ≈ {transcription_s:.1} s ({} s of audio)",
        truth.len(),
        share * 100.0,
        voice.len() / SR as usize
    );
    assert!(vocal_sdr >= 6.0, "vocal SDR {vocal_sdr:.2} dB");
    assert!(share >= 0.9, "only {:.1} % of the words aligned", share * 100.0);
}

/// Heard words of `wave.lyrics` with their start times.
fn heard_words(lyrics: &Value) -> Vec<(String, f32)> {
    lyrics["lines"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|l| l["words"].as_array().unwrap().iter().map(|w| (w["text"].as_str().unwrap().to_string(), w["start"].as_f64().unwrap() as f32)))
        .collect()
}

#[test]
#[ignore = "needs the htdemucs and default Whisper models and macOS `say`"]
fn long_instrumental_passages_add_no_words() {
    // 32 s of band before the voice, three phrases, 35 s of band, three phrases.
    let d = dir("acceptance_f2b_instrumental");
    let starts = [32.0, 38.0, 44.0, 85.0, 91.0, 97.0];
    let ([dr, bass, pad, voice], truth) = stems_with(&d, 44, &starts);
    let mix: Vec<f32> = (0..voice.len()).map(|i| dr[i] + bass[i] + pad[i] + voice[i]).collect();
    let src = d.join("mix.wav");
    write_wav(&src, SR, vec![mix.clone(), mix]);
    let first_gap_end = 32.0 - 0.5;
    let last_of_first_half = truth.iter().filter(|(_, t)| *t < 60.0).map(|(_, t)| *t).fold(0.0f32, f32::max);
    let (gap_from, gap_to) = (last_of_first_half + 2.5, 85.0 - 0.5);
    for language in ["it", "auto"] {
        let results = script(
            &d,
            &format!(
                "wave.open {{\"path\": \"{}\"}}\nwave.separate_stems {{\"model\": 4}}\nwave.transcribe {{\"language\": \"{language}\"}}\nwave.lyrics {{}}\n",
                src.to_str().unwrap()
            ),
        );
        let heard = heard_words(result(&results, "wave.lyrics"));
        println!("{language}: {}", heard.iter().map(|(w, t)| format!("{w}@{t:.2}")).collect::<Vec<_>>().join(" "));
        let phantom: Vec<&(String, f32)> = heard.iter().filter(|(_, t)| *t < first_gap_end || (*t > gap_from && *t < gap_to)).collect();
        let found = aligned(&truth, &heard);
        println!("{language}: {found}/{} words aligned, {} in the instrumental passages", truth.len(), phantom.len());
        assert!(phantom.is_empty(), "{language}: words in the instrumental passages: {phantom:?}");
        assert!(found as f32 >= 0.9 * truth.len() as f32, "{language}: only {found}/{} words", truth.len());
    }
}

#[test]
#[ignore = "needs the htdemucs_6s model and macOS `say`"]
fn six_stem_model_names_its_stems() {
    let d = dir("acceptance_f2b_six");
    let ([dr, bass, pad, voice], _) = stems(&d);
    let mix: Vec<f32> = (0..voice.len()).map(|i| dr[i] + bass[i] + pad[i] + voice[i]).collect();
    let src = d.join("mix.wav");
    write_wav(&src, SR, vec![mix.clone(), mix]);
    let results = script(&d, &format!("wave.open {{\"path\": \"{}\"}}\nwave.separate_stems {{\"model\": 6}}\n", src.to_str().unwrap()));
    let stems = result(&results, "wave.separate_stems")["stems"].as_array().unwrap().clone();
    let names: Vec<&str> = stems.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["drums", "bass", "other", "vocals", "guitar", "piano"]);
    let sdr_of = |name: &str, reference: &[f32]| {
        let path = PathBuf::from(stems.iter().find(|s| s["name"] == name).unwrap()["path"].as_str().unwrap());
        let (sr, est) = read_wav(&path);
        assert_eq!((sr, est.len(), est[0].len()), (SR, 2, reference.len()));
        (sdr(reference, &est[0]) + sdr(reference, &est[1])) / 2.0
    };
    let (v, dsdr, b) = (sdr_of("vocals", &voice), sdr_of("drums", &dr), sdr_of("bass", &bass));
    println!("six stems: vocals {v:.2} dB, drums {dsdr:.2} dB, bass {b:.2} dB");
    assert!(v >= 6.0 && dsdr >= 6.0 && b >= 3.0, "stems out of order or poorly separated");
}
