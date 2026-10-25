//! Transcription with whisper.cpp: words with their times (DTW token times on the large-v3 turbo
//! models, word-split segments otherwise), heard only where a voice sings.

use soundcraft_wave_analysis::lyrics::Word;
use soundcraft_wave_analysis::voiced;
use std::path::Path;
use std::sync::Once;
use whisper_rs::{DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn quiet() {
    static ONCE: Once = Once::new();
    ONCE.call_once(whisper_rs::install_logging_hooks);
}

fn preset(id: &str) -> Option<DtwModelPreset> {
    Some(match id {
        "whisper-tiny" => DtwModelPreset::Tiny,
        "whisper-base" => DtwModelPreset::Base,
        "whisper-small" => DtwModelPreset::Small,
        "whisper-medium" => DtwModelPreset::Medium,
        id if id.starts_with("whisper-large-v3-turbo") => DtwModelPreset::LargeV3Turbo,
        _ => return None,
    })
}

/// Annotations like "[Musica]" or "(applausi)" and bare punctuation are not words.
fn is_word(text: &str) -> bool {
    let t = text.trim();
    let tagged = (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')'));
    !tagged && t.chars().any(char::is_alphanumeric)
}

/// A segment more likely silence than speech is dropped.
const NO_SPEECH: f32 = 0.6;
const RATE: usize = 16_000;

fn params(language: Option<&str>) -> FullParams<'_, '_> {
    let mut fp = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    fp.set_language(language);
    fp.set_translate(false);
    // Each region on its own: text carried over from one region invents words in the next.
    fp.set_no_context(true);
    fp.set_suppress_nst(true);
    fp.set_print_progress(false);
    fp.set_print_realtime(false);
    fp.set_print_special(false);
    fp.set_print_timestamps(false);
    fp.set_token_timestamps(true);
    fp.set_split_on_word(true);
    fp.set_max_len(1);
    fp.set_n_threads(std::thread::available_parallelism().map_or(4, |n| n.get()).min(8) as i32);
    fp
}

/// The words of the voiced regions of `mono_16k` (see `voiced::regions`), with their times; the
/// language, when not given, is detected on the first region and kept for the others.
pub fn transcribe(path: &Path, id: &str, mono_16k: &[f32], language: Option<&str>) -> Result<Vec<Word>, String> {
    quiet();
    let audio: Vec<f32> = mono_16k.iter().map(|v| if v.is_finite() { *v } else { 0.0 }).collect();
    let regions = voiced::regions(&audio, RATE as u32);
    if regions.is_empty() {
        return Ok(Vec::new());
    }
    let mut cp = WhisperContextParameters::default();
    if let Some(p) = preset(id) {
        cp.dtw_parameters(DtwParameters { mode: DtwMode::ModelPreset { model_preset: p }, ..DtwParameters::default() });
    }
    let ctx = WhisperContext::new_with_params(path, cp).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut state = ctx.create_state().map_err(|e| format!("whisper: {e}"))?;
    let mut lang: Option<String> = language.filter(|l| *l != "auto").map(str::to_string);
    let mut words = Vec::new();
    for r in regions {
        let offset = r.start as f32 / RATE as f32;
        // whisper.cpp wants at least a second.
        let mut part = audio.get(r).unwrap_or(&[]).to_vec();
        part.resize(part.len().max(RATE + RATE / 10), 0.0);
        state.full(params(lang.as_deref()), &part).map_err(|e| format!("transcription failed: {e}"))?;
        if lang.is_none() {
            lang = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
        }
        for i in 0..state.full_n_segments() {
            let Some(seg) = state.get_segment(i) else { continue };
            let text = seg.to_str_lossy().map(|t| t.trim().to_string()).unwrap_or_default();
            if !is_word(&text) || seg.no_speech_probability() > NO_SPEECH {
                continue;
            }
            let mut start = seg.start_timestamp() as f32 / 100.0;
            // DTW times are sharper: take the first text token's.
            if let Some(t) = (0..seg.n_tokens()).filter_map(|j| seg.get_token(j)).find(|t| t.to_str_lossy().is_ok_and(|s| is_word(&s))) {
                let dtw = t.token_data().t_dtw;
                if dtw >= 0 {
                    start = dtw as f32 / 100.0;
                }
            }
            let start = offset + start.max(0.0);
            words.push(Word { text, start, end: start });
        }
    }
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    voiced::settle_word_ends(&mut words, &audio, RATE as u32);
    Ok(words)
}
