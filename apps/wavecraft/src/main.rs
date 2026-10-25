//! `wavecraft`: the WaveCraft window.
//!
//! ```text
//! wavecraft [FILE] [--control PORT] [--no-audio]
//!   FILE            an audio file to open (WAV, AIFF, FLAC, MP3, M4A, Ogg…)
//!   --control PORT  answer control requests on 127.0.0.1:PORT (0 picks a free port, printed as
//!                   WAVECRAFT_CONTROL_PORT=…); `wavecraft-cli mcp --connect PORT` wraps it for agents
//!   --no-audio      no audio output (playback off)
//!   --version       print the version
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod control_server;

use soundcraft_engine::Engine;
use soundcraft_ui_egui::Services;
use soundcraft_ui_wave::WaveApp;
use std::sync::Arc;

/// The usage block of the module doc comment.
fn help_text() -> String {
    include_str!("main.rs")
        .lines()
        .skip(3)
        .take_while(|l| l.trim_end() != "//! ```")
        .map(|l| l.trim_start_matches("//! "))
        .collect::<Vec<_>>()
        .join("\n")
}

struct App {
    wave: WaveApp,
    title: String,
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.wave.logic(ctx);
        let dropped: Vec<String> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_string_lossy().into_owned()).filter(|s| !s.is_empty()).collect());
        if let Some(path) = dropped.into_iter().next() {
            self.wave.open_path(&path);
        }
        if self.wave.quit_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && self.wave.engine.is_dirty() && !confirm_discard() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let name = self
            .wave
            .engine
            .wave
            .as_ref()
            .and_then(|w| w.path.as_deref())
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled");
        let title = format!("{name}{} — WaveCraft", if self.wave.engine.is_dirty() { " *" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.wave.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.wave.ui(ui);
    }

    fn on_exit(&mut self) {
        soundcraft_engine::shutdown_plugin_hosts();
    }
}

/// A yes/no question in a native dialog.
fn confirm(question: &str) -> bool {
    let answer = rfd::MessageDialog::new().set_title("WaveCraft").set_description(question).set_buttons(rfd::MessageButtons::YesNo).show();
    answer == rfd::MessageDialogResult::Yes
}

/// Closing with unsaved changes: ask before discarding them.
fn confirm_discard() -> bool {
    confirm("The file has unsaved changes. Close without saving?")
}

fn services() -> Services {
    Services {
        pick_open: Some(Box::new(|purpose: &str, exts: &[&str]| {
            let label = if exts.contains(&"json") { "Rack preset" } else { "Audio" };
            rfd::FileDialog::new().set_title(purpose).add_filter(label, exts).pick_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_save: Some(Box::new(|purpose: &str, default: &str| {
            let mut d = rfd::FileDialog::new().set_title(purpose);
            d = if default.ends_with(".json") {
                d.add_filter("Rack preset", &["json"])
            } else {
                d.add_filter("Audio", &["wav", "aif", "aiff", "flac"])
            };
            d.set_file_name(default).save_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_folder: Some(Box::new(|start: &str| {
            rfd::FileDialog::new().set_directory(start).pick_folder().map(|p| p.to_string_lossy().into_owned())
        })),
    }
}

fn app_icon() -> Option<egui::IconData> {
    let png: &[u8] = include_bytes!("../../../assets/app-icon/wavecraft-512.png");
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("WaveCraft {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", help_text());
        return Ok(());
    }
    let no_audio = args.iter().any(|a| a == "--no-audio");
    let control_port: Option<u16> = args
        .iter()
        .position(|a| a == "--control")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .or_else(|| std::env::var("WAVECRAFT_CONTROL_PORT").ok().and_then(|p| p.parse().ok()));
    let file = args
        .iter()
        .enumerate()
        .find(|(i, a)| !(a.starts_with("--") || (*i > 0 && args.get(i - 1).is_some_and(|p| p == "--control"))))
        .map(|(_, a)| a.clone());

    let mut engine = Engine::default();
    engine.wave_models = Some(std::sync::Arc::new(soundcraft_wave_ml::Backend::new()));
    if let Some(f) = &file
        && let Err(e) = engine.execute("wave.open", &serde_json::json!({"path": f}))
    {
        eprintln!("wavecraft: {f}: {e}");
    }
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("WaveCraft")
            .with_inner_size([1512.0, 898.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true)
            .with_app_id("ai.storyteller.wavecraft"),
        ..Default::default()
    };
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(icon);
    }
    eframe::run_native(
        "WaveCraft",
        options,
        Box::new(move |cc| {
            let player = (!no_audio).then(|| soundcraft_playback::Player::new(Arc::new(engine.session().clone())));
            let mut wave = WaveApp::new(engine, player, services()).with_confirm(Box::new(confirm));
            if let Some(port) = control_port {
                wave = wave.with_control(control_server::start(port, cc.egui_ctx.clone()));
            }
            Ok(Box::new(App { wave, title: String::new() }))
        }),
    )
}
