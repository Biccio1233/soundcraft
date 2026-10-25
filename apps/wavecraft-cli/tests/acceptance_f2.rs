//! F2a acceptance (music analysis without models), on the synthetic song of the design: A minor,
//! 96 BPM, A4 = 436 Hz, intro | verse | chorus | verse | chorus | bridge | chorus | outro.
//!
//! - The CLI script measures tuning, tempo, key, chords on the bar grid and sections. Targets: A4
//!   436 ± 1 Hz, 96 ± 2 BPM, "A minor", ≥ 85 % of bars right (root + quality), every true section
//!   boundary within one beat.
//! - Ignored (needs the songwriting corpus outside the repo): the exported ChordPro is read by the
//!   corpus reader `chordpro_corpus.py`, which must find our chord labels in order. Run it with
//!   `cargo test -p wavecraft-cli --test acceptance_f2 -- --ignored` (`WAVECRAFT_CORPUS_READER`
//!   overrides the reader's path).

use serde_json::Value;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_wave_analysis::chords::{Chord, ChordSegment, bar_accuracy};
use soundcraft_wave_analysis::testsignal::{Truth, acceptance_song, song};
use std::path::{Path, PathBuf};
use std::process::Command;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("wavecraft-cli-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run a script; returns each command's result in order, as `(id, json)`.
fn script(dir: &Path, text: &str) -> Vec<(String, Value)> {
    let path = dir.join("f2.wavecraft");
    std::fs::write(&path, text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft-cli")).arg("script").arg(&path).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    stdout.lines().filter_map(|l| l.split_once(": ").and_then(|(id, j)| serde_json::from_str(j).ok().map(|v| (id.to_string(), v)))).collect()
}

fn result<'a>(results: &'a [(String, Value)], id: &str) -> &'a Value {
    &results.iter().find(|(i, _)| i == id).unwrap_or_else(|| panic!("no {id} in {results:?}")).1
}

/// The acceptance song written as a 24-bit WAV, with its truth.
fn rendered(d: &Path) -> (PathBuf, Truth) {
    let spec = acceptance_song();
    let (channels, truth) = song(&spec);
    let path = d.join("song.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None };
    std::fs::write(&path, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: spec.sample_rate, channels }, &opts).unwrap()).unwrap();
    (path, truth)
}

#[test]
fn synthetic_song_meets_the_analysis_targets() {
    let d = dir("acceptance_f2_synthetic");
    let (path, truth) = rendered(&d);
    let results = script(
        &d,
        &format!(
            "wave.open {{\"path\": \"{}\"}}\n\
             wave.analyze_tuning {{}}\n\
             wave.analyze_tempo {{}}\n\
             wave.analyze_key {{}}\n\
             wave.analyze_chords {{\"grid\": \"bar\"}}\n\
             wave.analyze_sections {{}}\n",
            path.to_str().unwrap()
        ),
    );
    let a4 = result(&results, "wave.analyze_tuning")["a4_hz"].as_f64().unwrap();
    let bpm = result(&results, "wave.analyze_tempo")["bpm"].as_f64().unwrap();
    let key = result(&results, "wave.analyze_key")["key"].as_str().unwrap_or_default().to_string();
    let segs: Vec<ChordSegment> = result(&results, "wave.analyze_chords")["chords"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| ChordSegment {
            start: c["start"].as_f64().unwrap() as f32,
            end: c["end"].as_f64().unwrap() as f32,
            chord: c["label"].as_str().and_then(Chord::parse),
            confidence: c["confidence"].as_f64().unwrap_or(0.0) as f32,
        })
        .collect();
    let accuracy = bar_accuracy(&segs, &truth.bars);
    let found: Vec<f64> =
        result(&results, "wave.analyze_sections")["sections"].as_array().unwrap().iter().map(|s| s["start"].as_f64().unwrap()).collect();
    let beat = 60.0 / 96.0;
    let missed: Vec<f32> =
        truth.sections.iter().skip(1).map(|s| s.0).filter(|t| !found.iter().any(|f| (f - f64::from(*t)).abs() <= beat + 1e-3)).collect();
    println!(
        "synthetic song: A4 {a4:.2} Hz, {bpm:.2} BPM, key {key}, chords {:.1} % of bars, sections {} found / {} true, missed {missed:?}",
        accuracy * 100.0,
        found.len(),
        truth.sections.len()
    );
    assert!((a4 - 436.0).abs() <= 1.0, "A4 {a4}");
    assert!((bpm - 96.0).abs() <= 2.0, "BPM {bpm}");
    assert_eq!(key, "A minor");
    assert!(accuracy >= 0.85, "chords right on {:.1} % of bars", accuracy * 100.0);
    assert!(missed.is_empty(), "section boundaries missed: {missed:?} (found {found:?})");
}

#[test]
#[ignore = "needs the songwriting corpus reader (WAVECRAFT_CORPUS_READER)"]
fn chordpro_is_read_by_the_corpus_reader() {
    let reader = std::env::var("WAVECRAFT_CORPUS_READER").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/Desktop/5. song/3. strumenti/corpus/chordpro_corpus.py")
    });
    assert!(Path::new(&reader).exists(), "{reader}: set WAVECRAFT_CORPUS_READER");
    let d = dir("acceptance_f2_corpus");
    let (path, _) = rendered(&d);
    let cho = d.join("song.cho");
    let words = r#"[{"word": "chiusa", "start": 10.3, "end": 10.8}, {"word": "vive", "start": 10.9, "end": 11.4}, {"word": "dentro", "start": 12.6, "end": 13.0}, {"word": "me", "start": 13.2, "end": 13.6}, {"word": "luce", "start": 30.4, "end": 30.9}, {"word": "nuova", "start": 31.0, "end": 31.6}]"#;
    script(
        &d,
        &format!(
            "wave.open {{\"path\": \"{}\"}}\n\
             wave.lyrics_set {{\"words\": {words}}}\n\
             wave.chordpro_export {{\"path\": \"{}\", \"title\": \"Prova\"}}\n",
            path.to_str().unwrap(),
            cho.to_str().unwrap()
        ),
    );
    let text = std::fs::read_to_string(&cho).unwrap();
    let ours: Vec<String> = text
        .lines()
        .filter(|l| !l.starts_with('{'))
        .flat_map(|l| l.split('[').skip(1).filter_map(|p| p.split_once(']').map(|(c, _)| c.to_string())))
        .collect();
    // The reader names files relative to its corpus root: point the root at our folder.
    let py = "import importlib.util, json, sys\nfrom pathlib import Path\n\
              spec = importlib.util.spec_from_file_location('corpus', sys.argv[1])\n\
              m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)\n\
              p = Path(sys.argv[2]); m.RAD = p.parent\n\
              r = m.estrai(p)\nprint(json.dumps(r['accordi'] if r else None))";
    let out = Command::new("python3").args(["-I", "-c", py, &reader, cho.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let theirs: Vec<String> = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    println!("corpus reader: {} chords read, ours {}", theirs.len(), ours.len());
    assert!(theirs.len() >= 8);
    assert_eq!(theirs, ours);
}
