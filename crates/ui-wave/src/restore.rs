//! The restoration dialogs (one window per effect, built from the plugin's parameters) and the
//! Amplitude Statistics window. *Process* runs the effect's `wave.*` command on the selection (or
//! the whole file); *Add to Rack* puts it in the rack, which plays it live as a preview.

use crate::WaveApp;
use serde_json::{Map, Value, json};
use soundcraft_dsp::{ParamInfo, Taper, Unit};

/// An open effect dialog.
pub struct EffectDialog {
    pub plugin: &'static str,
    pub command: &'static str,
    values: Vec<(&'static ParamInfo, f32)>,
}

/// The noise print's band parameters, which the dialog and the rack don't show as sliders.
pub fn is_band_param(id: &str) -> bool {
    id.len() == 4 && id.starts_with("nb") && id[2..].chars().all(|c| c.is_ascii_digit())
}

impl EffectDialog {
    pub fn new(plugin: &'static str, command: &'static str) -> Option<Self> {
        let info = soundcraft_dsp::plugin_info(plugin)?;
        let values = info.params.iter().filter(|p| !is_band_param(p.id)).map(|p| (p, p.default)).collect();
        Some(Self { plugin, command, values })
    }

    /// The command's params: choices as their labels, toggles as booleans.
    fn command_params(&self) -> Value {
        let mut m = Map::new();
        for (p, v) in &self.values {
            let value = match p.unit {
                Unit::Choice => json!(p.choices.get(v.round().max(0.0) as usize).copied().unwrap_or_default()),
                Unit::Toggle => json!(*v >= 0.5),
                _ => json!(v),
            };
            m.insert(p.id.to_string(), value);
        }
        Value::Object(m)
    }

    /// The rack insert's params: raw values.
    fn rack_params(&self) -> Value {
        Value::Object(self.values.iter().map(|(p, v)| (p.id.to_string(), json!(v))).collect())
    }
}

impl WaveApp {
    /// Open the dialog of the restoration effect `plugin` (processed by `command`).
    pub fn open_effect(&mut self, plugin: &'static str, command: &'static str) {
        self.effect = EffectDialog::new(plugin, command);
    }

    /// Open the Amplitude Statistics window on the selection (or the whole file).
    pub fn open_stats(&mut self) {
        self.stats = self.run("wave.stats", json!({})).or(Some(Value::Null));
    }
}

fn param_row(ui: &mut egui::Ui, p: &ParamInfo, v: &mut f32) {
    ui.horizontal(|ui| match p.unit {
        Unit::Choice => {
            let current = p.choices.get(v.round().max(0.0) as usize).copied().unwrap_or_default();
            egui::ComboBox::from_label(p.name).selected_text(current).show_ui(ui, |ui| {
                for (i, c) in p.choices.iter().enumerate() {
                    ui.selectable_value(v, i as f32, *c);
                }
            });
        }
        Unit::Toggle => {
            let mut on = *v >= 0.5;
            if ui.checkbox(&mut on, p.name).changed() {
                *v = if on { 1.0 } else { 0.0 };
            }
        }
        _ => {
            ui.add(
                egui::Slider::new(v, p.min..=p.max)
                    .logarithmic(p.taper == Taper::Log && p.min > 0.0)
                    .text(p.name)
                    .custom_formatter(|x, _| p.format(x as f32)),
            );
        }
    });
}

/// The noise print as a curve: 62 sixth-octave bands, −140 to −20 dB.
fn print_curve(ui: &mut egui::Ui, bands: &[f64]) {
    let t = soundcraft_ui_egui::theme::Tokens::current();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(420.0), 90.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, t.panel_bg2);
    if bands.len() < 2 {
        return;
    }
    let (lo, hi) = (-140.0f64, -20.0f64);
    let points: Vec<egui::Pos2> = bands
        .iter()
        .enumerate()
        .map(|(i, db)| {
            let x = rect.left() + rect.width() * i as f32 / (bands.len() - 1) as f32;
            let y = rect.bottom() - rect.height() * ((db.clamp(lo, hi) - lo) / (hi - lo)) as f32;
            egui::pos2(x, y)
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, t.accent)));
}

/// The open effect dialog, if any.
pub fn effect_window(ctx: &egui::Context, app: &mut WaveApp) {
    let Some(mut dlg) = app.effect.take() else { return };
    let name = soundcraft_dsp::plugin_info(dlg.plugin).map_or(dlg.plugin, |i| i.name);
    let mut open = true;
    let mut actions: Vec<(&'static str, Value)> = Vec::new();
    let mut print_dialog: Option<bool> = None;
    let mut close = false;
    egui::Window::new(name)
        .id(egui::Id::new("wave_effect"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, 80.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if dlg.plugin == "noise_reduction" {
                let info = app.engine.execute("wave.noise_print_info", &json!({})).unwrap_or(Value::Null);
                if info["captured"].as_bool().unwrap_or(false) {
                    ui.label(format!("Noise print: {:.2} s, {}-point", info["seconds"].as_f64().unwrap_or(0.0), info["fft"].as_u64().unwrap_or(0)));
                    let bands: Vec<f64> = info["bands"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    print_curve(ui, &bands);
                } else {
                    ui.label("No noise print: select noise alone, then Capture Noise Print.");
                }
                ui.horizontal(|ui| {
                    if ui.button("Capture Noise Print").clicked() {
                        actions.push(("wave.noise_print_capture", json!({})));
                    }
                    if ui.button("Save…").clicked() {
                        print_dialog = Some(true);
                    }
                    if ui.button("Load…").clicked() {
                        print_dialog = Some(false);
                    }
                    if ui.button("Select Entire File").clicked() {
                        actions.push(("wave.select", json!({"start": 0, "end": {"seconds": 86_400}})));
                    }
                });
                ui.separator();
            }
            for (p, v) in &mut dlg.values {
                param_row(ui, p, v);
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Process").on_hover_text("Process the selection (or the whole file)").clicked() {
                    actions.push((dlg.command, dlg.command_params()));
                    close = true;
                }
                if ui.button("Add to Rack").on_hover_text("Preview it live in the rack").clicked() {
                    actions.push(("wave.rack_add", json!({"plugin": dlg.plugin, "params": dlg.rack_params()})));
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    let mut processed_ok = true;
    for (id, p) in actions {
        let ok = app.run(id, p).is_some();
        if id == dlg.command {
            processed_ok = ok;
        }
    }
    match print_dialog {
        Some(true) => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|pick| pick("Save noise print", "noise-print.json")) {
                app.run("wave.noise_print_save", json!({"path": path}));
            }
        }
        Some(false) => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick("Load noise print", &["json"])) {
                app.run("wave.noise_print_load", json!({"path": path}));
            }
        }
        None => {}
    }
    // A failed Process keeps the dialog open so the message can be read and the settings fixed.
    if open && !(close && processed_ok) {
        app.effect = Some(dlg);
    }
}

/// The Amplitude Statistics window, if open.
pub fn stats_window(ctx: &egui::Context, app: &mut WaveApp) {
    let Some(stats) = app.stats.take() else { return };
    let mut open = true;
    let mut refresh = false;
    egui::Window::new("Amplitude Statistics")
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-24.0, 80.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if stats.is_null() {
                ui.label("No audio to measure");
            } else {
                let f = |k: &str| stats[k].as_f64().map_or_else(|| "—".to_string(), |v| format!("{v:.2} dB"));
                egui::Grid::new("wave_stats").num_columns(2).show(ui, |ui| {
                    ui.label("Peak");
                    ui.label(f("peak_db"));
                    ui.end_row();
                    ui.label("RMS");
                    ui.label(f("rms_db"));
                    ui.end_row();
                    ui.label("Samples");
                    ui.label(stats["frames"].as_u64().map_or_else(String::new, |n| n.to_string()));
                    ui.end_row();
                });
            }
            if ui.button("Measure Selection").clicked() {
                refresh = true;
            }
        });
    if refresh {
        app.open_stats();
    } else if open {
        app.stats = Some(stats);
    }
}
