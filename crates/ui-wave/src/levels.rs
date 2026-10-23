//! The Levels meter: the playback peak of each channel.

use crate::WaveApp;
use egui::{Rect, Sense, pos2, vec2};

pub fn show(ui: &mut egui::Ui, app: &WaveApp) {
    let peaks = app.player.as_ref().map_or([0.0; 2], |p| p.meters().main.peak);
    ui.vertical(|ui| {
        ui.strong("Levels");
        let (rect, _) = ui.allocate_exact_size(vec2(44.0, 70.0), Sense::hover());
        for (i, level) in peaks.iter().enumerate() {
            let x = rect.min.x + i as f32 * 22.0;
            let r = Rect::from_min_max(pos2(x, rect.min.y), pos2(x + 16.0, rect.max.y));
            soundcraft_ui_egui::widgets::meter(ui, r, *level, *level, *level >= 1.0);
        }
    });
}
