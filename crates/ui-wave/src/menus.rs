//! WaveCraft's menus: each item is an engine command with fixed params (and, for files, a dialog
//! first). The engine's `wave.*` specs carry no menu path, so SoundCraft's menus never show them.

use crate::WaveApp;
use serde_json::{Value, json};

/// What an item asks for before running its command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    None,
    OpenFile,
    ExportFile,
    SavePreset,
    LoadPreset,
    SavePrint,
    LoadPrint,
}

#[derive(Debug, Clone, Copy)]
pub struct MenuItem {
    pub path: &'static [&'static str],
    pub label: &'static str,
    pub command: &'static str,
    /// JSON params ("" for none).
    pub params: &'static str,
    pub shortcut: Option<&'static str>,
    pub dialog: Dialog,
}

const fn item(
    path: &'static [&'static str],
    label: &'static str,
    command: &'static str,
    params: &'static str,
    shortcut: Option<&'static str>,
) -> MenuItem {
    MenuItem { path, label, command, params, shortcut, dialog: Dialog::None }
}

const fn file(path: &'static [&'static str], label: &'static str, command: &'static str, shortcut: Option<&'static str>, dialog: Dialog) -> MenuItem {
    MenuItem { path, label, command, params: "", shortcut, dialog }
}

pub const WAVE_MENUS: &[MenuItem] = &[
    file(&["File"], "Open…", "wave.open", Some("Cmd+O"), Dialog::OpenFile),
    item(&["File"], "Save", "wave.save", "", Some("Cmd+S")),
    file(&["File"], "Export…", "wave.export", Some("Cmd+Shift+E"), Dialog::ExportFile),
    item(&["Edit"], "Undo", "edit.undo", "", Some("Cmd+Z")),
    item(&["Edit"], "Redo", "edit.redo", "", Some("Cmd+Shift+Z")),
    item(&["Edit"], "Cut", "wave.cut", "", Some("Cmd+X")),
    item(&["Edit"], "Copy", "wave.copy", "", Some("Cmd+C")),
    item(&["Edit"], "Paste", "wave.paste", "", Some("Cmd+V")),
    item(&["Edit"], "Delete", "wave.delete", "", Some("Delete")),
    item(&["Edit"], "Insert 1 s of Silence", "wave.insert_silence", r#"{"seconds": 1}"#, None),
    item(&["Edit"], "Select All", "wave.select", r#"{"start": 0, "end": {"seconds": 86400}}"#, Some("Cmd+A")),
    item(&["Effects"], "Gain +3 dB", "wave.gain", r#"{"db": 3}"#, None),
    item(&["Effects"], "Gain −3 dB", "wave.gain", r#"{"db": -3}"#, None),
    item(&["Effects"], "Normalize to −0.1 dB", "wave.normalize", r#"{"db": -0.1}"#, None),
    item(&["Effects"], "Fade In", "wave.fade_in", "", None),
    item(&["Effects"], "Fade Out", "wave.fade_out", "", None),
    item(&["Effects"], "Invert", "wave.invert", "", None),
    item(&["Effects"], "Reverse", "wave.reverse", "", None),
    item(&["Effects"], "Silence", "wave.silence", "", None),
    item(&["Effects"], "Apply Rack", "wave.rack_apply", "", None),
    file(&["Effects"], "Load Rack Preset…", "wave.rack_preset_load", None, Dialog::LoadPreset),
    file(&["Effects"], "Save Rack Preset…", "wave.rack_preset_save", None, Dialog::SavePreset),
    item(RESTORE, "Capture Noise Print", "wave.noise_print_capture", "", Some("Shift+P")),
    item(RESTORE, "Noise Reduction", "wave.noise_reduction", "", Some("Cmd+Shift+P")),
    item(RESTORE, "Adaptive Denoise", "wave.adaptive_noise_reduction", "", None),
    item(RESTORE, "Hum Removal", "wave.dehum", "", None),
    item(RESTORE, "Hiss Reduction", "wave.hiss_reduction", "", None),
    item(RESTORE, "Clip Repair", "wave.declip", "", None),
    item(RESTORE, "Reverb Reduction", "wave.dereverb", "", None),
    file(RESTORE, "Save Noise Print…", "wave.noise_print_save", None, Dialog::SavePrint),
    file(RESTORE, "Load Noise Print…", "wave.noise_print_load", None, Dialog::LoadPrint),
    item(RESTORE, "Clear Noise Print", "wave.noise_print_clear", "", None),
    item(&["View"], "Spectral Display", "wave.view_spectral", "", Some("Cmd+Shift+D")),
];

/// The restoration submenu of Effects.
const RESTORE: &[&str] = &["Effects", "Noise Reduction / Restoration"];

/// The top-level menu names, in order.
fn roots() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for m in WAVE_MENUS {
        if let Some(r) = m.path.first()
            && !v.contains(r)
        {
            v.push(r);
        }
    }
    v
}

/// The menu bar.
pub fn bar(ui: &mut egui::Ui, app: &mut WaveApp) {
    let mut clicked = None;
    ui.horizontal(|ui| {
        for root in roots() {
            ui.menu_button(root, |ui| {
                for m in WAVE_MENUS.iter().filter(|m| m.path.first() == Some(&root)) {
                    let enabled = soundcraft_engine::find_command(m.command).is_some_and(|s| s.info(&app.engine).enabled);
                    let mut button = egui::Button::new(m.label);
                    if let Some(sc) = m.shortcut {
                        button = button.shortcut_text(sc);
                    }
                    if ui.add_enabled(enabled, button).clicked() {
                        clicked = Some(m);
                        ui.close();
                    }
                }
            });
        }
    });
    if let Some(m) = clicked {
        invoke(app, m);
    }
}

/// Run a menu item (asking for a file first when it needs one).
pub fn invoke(app: &mut WaveApp, m: &MenuItem) {
    let params: Value = if m.params.is_empty() { json!({}) } else { serde_json::from_str(m.params).unwrap_or_else(|_| json!({})) };
    match m.dialog {
        Dialog::None => {
            app.run(m.command, params);
        }
        Dialog::OpenFile => app.open_dialog(),
        Dialog::ExportFile => app.export_dialog(),
        Dialog::SavePreset => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|pick| pick("Save rack preset", "rack.json")) {
                app.run(m.command, json!({"path": path}));
            }
        }
        Dialog::LoadPreset => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick("Load rack preset", &["json"])) {
                app.run(m.command, json!({"path": path}));
            }
        }
        Dialog::SavePrint => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|pick| pick("Save noise print", "noise-print.json")) {
                app.run(m.command, json!({"path": path}));
            }
        }
        Dialog::LoadPrint => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick("Load noise print", &["json"])) {
                app.run(m.command, json!({"path": path}));
            }
        }
    }
}

/// Consume `shortcut` ("Cmd+Shift+Z", "Delete", …) if it was pressed this frame.
pub fn consume(input: &mut egui::InputState, shortcut: &str) -> bool {
    let mut mods = egui::Modifiers::NONE;
    let mut key = None;
    for part in shortcut.split('+') {
        match part {
            "Cmd" => mods |= egui::Modifiers::COMMAND,
            "Shift" => mods |= egui::Modifiers::SHIFT,
            "Alt" => mods |= egui::Modifiers::ALT,
            name => key = egui::Key::from_name(name),
        }
    }
    key.is_some_and(|k| input.consume_key(mods, k))
}
