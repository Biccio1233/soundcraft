//! Selection/View: start, end and duration of the selection (editable) and of the view.

use crate::{WaveApp, clock, parse_clock};
use serde_json::json;

/// Text being typed in the selection fields.
#[derive(Default)]
pub struct Edit {
    start: String,
    end: String,
}

pub fn show(ui: &mut egui::Ui, app: &mut WaveApp) {
    let sr = app.sample_rate();
    let sel = app.selection();
    let z = app.engine.session().edit.zoom;
    let view_end = z.scroll.saturating_add((z.samples_per_px * f64::from(app.timeline.width().max(0.0))) as i64);
    let mut commit = false;
    egui::Grid::new("wave_selection").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.strong("");
        ui.strong("Start");
        ui.strong("End");
        ui.strong("Duration");
        ui.end_row();
        ui.label("Selection");
        for (text, value) in [(&mut app.selection_edit.start, sel.start), (&mut app.selection_edit.end, sel.end)] {
            let r = ui.add_sized([92.0, 20.0], egui::TextEdit::singleline(text));
            if !r.has_focus() && !r.lost_focus() {
                *text = clock(value, sr);
            }
            commit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        }
        ui.label(clock(sel.len(), sr));
        ui.end_row();
        ui.label("View");
        ui.label(clock(z.scroll, sr));
        ui.label(clock(view_end, sr));
        ui.label(clock(view_end.saturating_sub(z.scroll), sr));
        ui.end_row();
    });
    if commit {
        match (parse_clock(&app.selection_edit.start, sr), parse_clock(&app.selection_edit.end, sr)) {
            (Some(start), Some(end)) => {
                app.run("wave.select", json!({"start": start, "end": end}));
            }
            _ => app.status = "type times as m:ss.mmm or seconds".into(),
        }
    }
}
