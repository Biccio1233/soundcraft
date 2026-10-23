//! The Files panel: the open file, its format, and Open.

use crate::{WaveApp, clock};
use soundcraft_engine::cmd::wave::{WaveFile, format_name, sample_format_name};

/// "48000 Hz • 24-bit • Stereo".
pub fn summary(w: &WaveFile) -> String {
    let channels = if w.channels == 1 { "Mono" } else { "Stereo" };
    format!("{} Hz • {} • {channels}", w.sample_rate, depth(w))
}

pub fn show(ui: &mut egui::Ui, app: &mut WaveApp) {
    ui.strong("File");
    match app.engine.wave.clone() {
        Some(w) => {
            let name = w.path.as_deref().and_then(|p| std::path::Path::new(p).file_name()).and_then(|n| n.to_str()).unwrap_or("Untitled").to_string();
            let dirty = if app.engine.is_dirty() { " *" } else { "" };
            ui.label(format!("{name}{dirty}"));
            ui.label(summary(&w));
            ui.label(format!("{} · {}", clock(app.file_len(), w.sample_rate), format_name(w.format).to_ascii_uppercase()));
        }
        None => {
            ui.label("No file open");
        }
    }
    if ui.button("Open…").clicked() {
        app.open_dialog();
    }
}

/// Bit depth wording for the sample formats WaveCraft reports.
fn depth(w: &WaveFile) -> String {
    match sample_format_name(w.sample_format) {
        "int8" => "8-bit".into(),
        "int16" => "16-bit".into(),
        "int24" => "24-bit".into(),
        "int32" => "32-bit".into(),
        "float32" => "32-bit float".into(),
        "float64" => "64-bit float".into(),
        other => other.into(),
    }
}
