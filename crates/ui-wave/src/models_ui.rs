//! The model windows: Separate Stems, Transcribe Lyrics, Practice (tempo, transpose, stem levels)
//! and Models (what is installed, sizes, licences, downloads). The work runs as background jobs.

use crate::WaveApp;
use serde_json::{Value, json};
use soundcraft_engine::cmd::wave_ml::{current_practice, current_stems};
use soundcraft_wave_analysis::models::{DEFAULT_WHISPER, MODELS, ModelKind, model};

const LANGUAGES: [(&str, &str); 3] = [("auto", "Auto"), ("it", "Italiano"), ("en", "English")];

/// Which model windows are open and what they are set to.
pub struct State {
    pub stems_open: bool,
    pub stems: u8,
    pub transcribe_open: bool,
    pub language: &'static str,
    pub whisper: &'static str,
    pub practice_open: bool,
    pub tempo: f32,
    pub semitones: f32,
    pub gains: Vec<(String, f32)>,
    pub models_open: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            stems_open: false,
            stems: 4,
            transcribe_open: false,
            language: "auto",
            whisper: DEFAULT_WHISPER,
            practice_open: false,
            tempo: 100.0,
            semitones: 0.0,
            gains: Vec::new(),
            models_open: false,
        }
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1e6)
}

/// What the windows asked for this frame, run after they are drawn.
enum Action {
    Separate(Value),
    Transcribe(Value),
    Practice(Value),
    PracticeOff,
    ExportPractice,
    Download(&'static str),
}

fn installed(app: &WaveApp, id: &str) -> bool {
    app.engine.wave_models.as_ref().is_some_and(|m| m.installed(id).is_some())
}

/// "Separate", or "Download 169 MB and Separate" when the model is missing.
fn run_label(app: &WaveApp, id: &str, verb: &str) -> String {
    match model(id) {
        Some(m) if !installed(app, id) => format!("Download {} and {verb}", mb(m.bytes)),
        _ => verb.to_string(),
    }
}

fn no_models(ui: &mut egui::Ui, app: &WaveApp) -> bool {
    if app.engine.wave_models.is_none() {
        ui.label("This build has no model support.");
        return true;
    }
    false
}

fn stems_window(ctx: &egui::Context, app: &mut WaveApp, actions: &mut Vec<Action>) {
    let mut open = app.models.stems_open;
    let busy = app.job_running();
    egui::Window::new("Separate Stems")
        .id(egui::Id::new("wave_stems"))
        .pivot(egui::Align2::LEFT_TOP)
        .default_pos(ctx.content_rect().left_top() + egui::vec2(24.0, 80.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if no_models(ui, app) {
                return;
            }
            ui.horizontal(|ui| {
                ui.radio_value(&mut app.models.stems, 4, "4 stems (vocals, drums, bass, other)");
                ui.radio_value(&mut app.models.stems, 6, "6 stems (+ guitar, piano)");
            });
            let id = if app.models.stems == 6 { "htdemucs_6s" } else { "htdemucs" };
            let size = model(id).map_or_else(String::new, |m| mb(m.bytes));
            ui.label(format!("Model {id}, {size}: {}", if installed(app, id) { "installed" } else { "not downloaded yet" }));
            let label = run_label(app, id, "Separate");
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                actions.push(Action::Separate(json!({"model": app.models.stems, "download": true})));
            }
            if let Some(s) = app.engine.wave.as_ref().and_then(|w| w.stems.as_ref()) {
                ui.separator();
                let names: Vec<&str> = s.files.iter().map(|f| f.0.as_str()).collect();
                let stale = if current_stems(&app.engine).is_some() { "" } else { " (out of date: the file changed since)" };
                ui.label(format!("Stems: {}{stale}", names.join(", ")));
                ui.label(egui::RichText::new(format!("Written to {}", s.dir)).small());
            }
        });
    app.models.stems_open = open;
}

fn transcribe_window(ctx: &egui::Context, app: &mut WaveApp, actions: &mut Vec<Action>) {
    let mut open = app.models.transcribe_open;
    let busy = app.job_running();
    egui::Window::new("Transcribe Lyrics")
        .id(egui::Id::new("wave_transcribe"))
        .pivot(egui::Align2::LEFT_TOP)
        .default_pos(ctx.content_rect().left_top() + egui::vec2(24.0, 320.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if no_models(ui, app) {
                return;
            }
            ui.horizontal(|ui| {
                ui.label("Language");
                let current = LANGUAGES.iter().find(|l| l.0 == app.models.language).map_or("Auto", |l| l.1);
                egui::ComboBox::from_id_salt("wave_language").selected_text(current).show_ui(ui, |ui| {
                    for (code, name) in LANGUAGES {
                        ui.selectable_value(&mut app.models.language, code, name);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Model");
                egui::ComboBox::from_id_salt("wave_whisper").selected_text(app.models.whisper).show_ui(ui, |ui| {
                    for m in MODELS.iter().filter(|m| matches!(m.kind, ModelKind::Transcription)) {
                        let mark = if installed(app, m.id) { " ✓" } else { "" };
                        ui.selectable_value(&mut app.models.whisper, m.id, format!("{} · {}{mark}", m.id, mb(m.bytes)));
                    }
                });
            });
            let source = if current_stems(&app.engine).is_some_and(|s| s.stems.iter().any(|st| st.name == "vocals")) {
                "the vocals stem"
            } else {
                "the mix (separate stems first for cleaner words)"
            };
            ui.label(format!("Listens to {source}."));
            let label = run_label(app, app.models.whisper, "Transcribe");
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                actions.push(Action::Transcribe(json!({"language": app.models.language, "model": app.models.whisper, "download": true})));
            }
        });
    app.models.transcribe_open = open;
}

fn practice_window(ctx: &egui::Context, app: &mut WaveApp, actions: &mut Vec<Action>) {
    let mut open = app.models.practice_open;
    let busy = app.job_running();
    // One gain per current stem.
    let stem_names: Vec<String> = current_stems(&app.engine).map(|s| s.files.iter().map(|f| f.0.clone()).collect()).unwrap_or_default();
    if app.models.gains.iter().map(|g| &g.0).ne(stem_names.iter()) {
        app.models.gains = stem_names.iter().map(|n| (n.clone(), 1.0)).collect();
    }
    egui::Window::new("Practice")
        .id(egui::Id::new("wave_practice"))
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, 80.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if no_models(ui, app) {
                return;
            }
            ui.add(egui::Slider::new(&mut app.models.tempo, 50.0..=150.0).step_by(1.0).suffix(" %").text("Tempo"));
            ui.add(egui::Slider::new(&mut app.models.semitones, -12.0..=12.0).step_by(1.0).text("Transpose (semitones)"));
            for (name, gain) in &mut app.models.gains {
                ui.add(egui::Slider::new(gain, 0.0..=2.0).text(name.as_str()));
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Render")).clicked() {
                    let mut p = json!({"tempo": app.models.tempo, "semitones": app.models.semitones});
                    if !app.models.gains.is_empty() {
                        p["stems"] = Value::Object(app.models.gains.iter().map(|(n, g)| (n.clone(), json!(g))).collect());
                    }
                    actions.push(Action::Practice(p));
                }
                let on = app.engine.wave.as_ref().and_then(|w| w.practice.as_ref());
                if ui.add_enabled(on.is_some(), egui::Button::new("Off")).clicked() {
                    actions.push(Action::PracticeOff);
                }
                if ui.add_enabled(current_practice(&app.engine).is_some(), egui::Button::new("Export…")).clicked() {
                    actions.push(Action::ExportPractice);
                }
            });
            if let Some(p) = current_practice(&app.engine) {
                ui.label(format!("Playing the practice render: {:.0} %, {:+.0} semitones", p.tempo, p.semitones));
            } else if app.engine.wave.as_ref().is_some_and(|w| w.practice.is_some()) {
                ui.label("The file changed since the last render: Render again to practise it.");
            }
        });
    app.models.practice_open = open;
}

fn models_window(ctx: &egui::Context, app: &mut WaveApp, actions: &mut Vec<Action>) {
    let mut open = app.models.models_open;
    let busy = app.job_running();
    egui::Window::new("Models")
        .id(egui::Id::new("wave_models"))
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-24.0, 80.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if no_models(ui, app) {
                return;
            }
            ui.label("Downloaded only when you ask, checked by size and sha256.");
            egui::Grid::new("wave_models_grid").num_columns(5).striped(true).show(ui, |ui| {
                for m in MODELS {
                    ui.label(m.id);
                    ui.label(match m.kind {
                        ModelKind::Separation { stems } => format!("{stems} stems"),
                        ModelKind::Transcription => "lyrics".to_string(),
                    });
                    ui.label(mb(m.bytes));
                    ui.label(m.licence);
                    if installed(app, m.id) {
                        ui.label("installed");
                    } else if ui.add_enabled(!busy, egui::Button::new("Download")).clicked() {
                        actions.push(Action::Download(m.id));
                    }
                    ui.end_row();
                }
            });
        });
    app.models.models_open = open;
}

/// The open model windows, then what they asked for.
pub fn windows(ctx: &egui::Context, app: &mut WaveApp) {
    let mut actions = Vec::new();
    if app.models.stems_open {
        stems_window(ctx, app, &mut actions);
    }
    if app.models.transcribe_open {
        transcribe_window(ctx, app, &mut actions);
    }
    if app.models.practice_open {
        practice_window(ctx, app, &mut actions);
    }
    if app.models.models_open {
        models_window(ctx, app, &mut actions);
    }
    for a in actions {
        match a {
            Action::Separate(p) => app.separate_in_background(p),
            Action::Transcribe(p) => app.transcribe_in_background(p),
            Action::Practice(p) => app.practice_in_background(p),
            Action::PracticeOff => {
                app.run("wave.practice_off", json!({}));
            }
            Action::ExportPractice => {
                if let Some(path) = app.services.pick_save.as_ref().and_then(|pick| pick("Export practice", "practice.wav")) {
                    app.run("wave.practice_export", json!({"path": path}));
                }
            }
            Action::Download(id) => app.download_in_background(id),
        }
    }
}

/// Open the Practice window, its sliders on the render that plays (if any).
pub fn open_practice(app: &mut WaveApp) {
    if let Some(p) = current_practice(&app.engine) {
        app.models.tempo = p.tempo;
        app.models.semitones = p.semitones;
        app.models.gains = p.gains.clone();
    }
    app.models.practice_open = true;
}
