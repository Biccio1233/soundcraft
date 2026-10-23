//! The History panel: every step since the file was opened; click one to go back (or forward) to it.

use crate::WaveApp;
use serde_json::json;

pub fn show(ui: &mut egui::Ui, app: &mut WaveApp) {
    ui.strong("History");
    let Ok(h) = app.engine.execute("wave.history", &json!({})) else {
        ui.label("No file open");
        return;
    };
    let t = soundcraft_ui_egui::theme::Tokens::current();
    let mut jump = None;
    egui::ScrollArea::vertical().id_salt("wave_history").max_height(240.0).show(ui, |ui| {
        for row in h["rows"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let label = row["label"].as_str().unwrap_or_default();
            let state = row["state"].as_str().unwrap_or_default();
            let mut text = egui::RichText::new(label);
            if state == "undone" {
                text = text.italics().color(t.text_dim);
            }
            if ui.selectable_label(state == "current", text).clicked() {
                jump = row["index"].as_u64();
            }
        }
    });
    let mut mb = h["budget_mb"].as_u64().unwrap_or(2048);
    let used = h["bytes"].as_u64().unwrap_or(0) >> 20;
    ui.horizontal(|ui| {
        ui.label(format!("Memory {used} MB of"));
        if ui.add(egui::DragValue::new(&mut mb).range(64..=65_536).suffix(" MB")).changed() {
            app.run("wave.history_budget", json!({"mb": mb}));
        }
    });
    if let Some(index) = jump {
        app.run("wave.history_jump", json!({"index": index}));
    }
}
