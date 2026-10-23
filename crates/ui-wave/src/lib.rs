//! WaveCraft's user interface: one audio file in an editor (waveform and spectral view), with an
//! effects rack, the history, levels and the selection. Every action goes through
//! `engine.execute`, so menus, shortcuts, the CLI, the control channel and MCP do the same thing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod editor;
mod files;
mod history;
mod levels;
pub mod menus;
mod rack;
mod selection;

use egui::Rect;
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_playback::Player;
use soundcraft_time::{Range, Samples};
use soundcraft_ui_egui::Services;

/// The WaveCraft window's state. The document lives in `engine`; this only holds view state.
pub struct WaveApp {
    pub engine: Engine,
    player: Option<Player>,
    services: Services,
    /// Engine revision last handed to the player.
    synced_revision: u64,
    /// The file the view was last fitted to (a new file is shown whole).
    fitted_path: Option<String>,
    /// The waveform area of the last frame, in points.
    timeline: Rect,
    /// Where a selection drag started.
    drag_anchor: Option<Samples>,
    /// Loop playback of the selection.
    looping: bool,
    /// Theme and fonts installed.
    styled: bool,
    /// Last error or message, shown at the bottom.
    status: String,
    /// The effect picked in the rack's "add" list.
    add_effect: String,
    /// Text being edited in the Selection/View fields.
    selection_edit: selection::Edit,
}

impl WaveApp {
    pub fn new(engine: Engine, player: Option<Player>, services: Services) -> Self {
        WaveApp {
            synced_revision: engine.revision.wrapping_sub(1),
            engine,
            player,
            services,
            fitted_path: None,
            timeline: Rect::NOTHING,
            drag_anchor: None,
            looping: false,
            styled: false,
            status: String::new(),
            add_effect: "gain".into(),
            selection_edit: selection::Edit::default(),
        }
    }

    /// Run a command; an error goes to the status line. Returns the result when it worked.
    pub fn run(&mut self, id: &str, params: Value) -> Option<Value> {
        match self.engine.execute(id, &params) {
            Ok(v) => {
                if let Some(m) = self.engine.messages.pop() {
                    self.status = m;
                    self.engine.messages.clear();
                } else {
                    self.status.clear();
                }
                Some(v)
            }
            Err(err) => {
                self.status = err.to_string();
                None
            }
        }
    }

    /// The waveform area of the last frame, in points.
    pub fn timeline_rect(&self) -> Rect {
        self.timeline
    }

    /// Where position `at` (samples) is drawn in the waveform area, in points.
    pub fn x_for(&self, at: Samples) -> f32 {
        soundcraft_ui_egui::edit_window::x_of(self.engine.session(), self.timeline, at)
    }

    /// The open file's sample rate (48 kHz when nothing is open).
    fn sample_rate(&self) -> u32 {
        self.engine.wave.as_ref().map_or(48_000, |w| w.sample_rate)
    }

    /// The file's length in frames.
    fn file_len(&self) -> Samples {
        soundcraft_engine::cmd::wave::file_len(&self.engine)
    }

    fn selection(&self) -> Range {
        self.engine.session().edit.selection
    }

    /// Per-frame work that doesn't draw: theme, the player's copy of the document, transport.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if !self.styled {
            soundcraft_ui_egui::fonts::install(ctx);
            soundcraft_ui_egui::theme::apply(ctx, false);
            self.styled = true;
        }
        if let Some(p) = &self.player
            && self.engine.revision != self.synced_revision
            && self.drag_anchor.is_none()
        {
            p.update_session(self.engine.session_arc());
            self.synced_revision = self.engine.revision;
        }
        for req in std::mem::take(&mut self.engine.transport_requests) {
            use soundcraft_engine::TransportRequest as T;
            match req {
                T::Play | T::PlaySelection => self.play(),
                T::Stop | T::Pause | T::ResetSession => self.stop(),
                T::TogglePlay => self.toggle_play(),
                T::Locate(at) => {
                    if let Some(p) = &self.player {
                        p.locate(at);
                    }
                }
                _ => {}
            }
        }
        if self.player.as_ref().is_some_and(Player::is_playing) {
            ctx.request_repaint();
        }
    }

    fn is_playing(&self) -> bool {
        self.player.as_ref().is_some_and(Player::is_playing)
    }

    /// Play the selection (looped when looping is on), or from the cursor to the end.
    fn play(&mut self) {
        let sel = self.selection();
        if let Some(p) = &self.player {
            if self.engine.revision != self.synced_revision {
                p.update_session(self.engine.session_arc());
                self.synced_revision = self.engine.revision;
            }
            if sel.is_empty() {
                p.play(sel.start, Some(self.file_len()), None);
            } else {
                p.play(sel.start, Some(sel.end), self.looping.then_some(sel));
            }
        } else {
            self.status = "no audio output".into();
        }
    }

    fn stop(&mut self) {
        if let Some(p) = &self.player {
            p.stop();
        }
    }

    fn toggle_play(&mut self) {
        if self.is_playing() { self.stop() } else { self.play() }
    }

    /// Ask for a file to open, then open it.
    fn open_dialog(&mut self) {
        let picked =
            self.services.pick_open.as_ref().and_then(|pick| pick("Open audio file", &["wav", "wave", "aif", "aiff", "flac", "mp3", "m4a", "ogg"]));
        if let Some(path) = picked {
            self.stop();
            self.run("wave.open", json!({"path": path}));
        }
    }

    /// Ask where to export, then export with WaveCraft's defaults (24-bit unless the name says otherwise).
    fn export_dialog(&mut self) {
        let name = self
            .engine
            .wave
            .as_ref()
            .and_then(|w| w.path.as_deref())
            .and_then(|p| std::path::Path::new(p).file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled");
        let suggested = format!("{name} (edit).wav");
        if let Some(path) = self.services.pick_save.as_ref().and_then(|pick| pick("Export", &suggested)) {
            self.run("wave.export", json!({"path": path}));
        }
    }

    /// The whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.shortcuts(ui);
        egui::Panel::top("wave_menu").show(ui, |ui| menus::bar(ui, self));
        egui::Panel::bottom("wave_status").show(ui, |ui| {
            let t = soundcraft_ui_egui::theme::Tokens::current();
            let text = if self.status.is_empty() {
                self.engine.wave.as_ref().map_or_else(|| "No file open".to_string(), files::summary)
            } else {
                self.status.clone()
            };
            ui.label(egui::RichText::new(text).color(t.text_dim));
        });
        egui::Panel::bottom("wave_bottom").resizable(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                levels::show(ui, self);
                ui.separator();
                selection::show(ui, self);
            });
        });
        egui::Panel::left("wave_left").resizable(true).default_size(300.0).show(ui, |ui| {
            files::show(ui, self);
            ui.separator();
            rack::show(ui, self);
            ui.separator();
            history::show(ui, self);
        });
        egui::CentralPanel::default().show(ui, |ui| editor::show(ui, self));
    }

    /// Keyboard shortcuts from the menus, plus Space for play/stop.
    fn shortcuts(&mut self, ui: &egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_play();
        }
        let fired: Vec<&menus::MenuItem> =
            menus::WAVE_MENUS.iter().filter(|m| m.shortcut.is_some_and(|s| ui.input_mut(|i| menus::consume(i, s)))).collect();
        for item in fired {
            menus::invoke(self, item);
        }
    }
}

/// `m:ss.mmm` of a position.
pub fn clock(pos: Samples, sample_rate: u32) -> String {
    let secs = pos.max(0) as f64 / f64::from(sample_rate.max(1));
    let m = (secs / 60.0).floor();
    format!("{m}:{:06.3}", secs - m * 60.0)
}

/// A position typed as `m:ss.mmm`, `ss.mmm` or plain seconds, in samples.
pub fn parse_clock(text: &str, sample_rate: u32) -> Option<Samples> {
    let text = text.trim();
    let secs = match text.split_once(':') {
        Some((m, s)) => m.trim().parse::<f64>().ok()? * 60.0 + s.trim().parse::<f64>().ok()?,
        None => text.parse::<f64>().ok()?,
    };
    (secs.is_finite() && secs >= 0.0).then(|| (secs * f64::from(sample_rate)).round() as Samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_round_trips() {
        assert_eq!(clock(96_000, 48_000), "0:02.000");
        assert_eq!(clock(48_000 * 75 + 24_000, 48_000), "1:15.500");
        assert_eq!(parse_clock("1:15.500", 48_000), Some(48_000 * 75 + 24_000));
        assert_eq!(parse_clock("2", 48_000), Some(96_000));
        assert_eq!(parse_clock("-1", 48_000), None);
        assert_eq!(parse_clock("abc", 48_000), None);
    }
}
