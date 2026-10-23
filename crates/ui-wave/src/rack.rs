//! The Effects Rack: up to ten effects previewed on playback; Apply bakes them into the audio.

use crate::WaveApp;
use serde_json::{Value, json};

pub fn show(ui: &mut egui::Ui, app: &mut WaveApp) {
    ui.strong("Effects Rack");
    let slots = app.engine.execute("wave.rack_list", &json!({})).ok().and_then(|v| v["slots"].as_array().cloned()).unwrap_or_default();
    let mut actions: Vec<(&'static str, Value)> = Vec::new();
    let mut merged: Option<(Value, String)> = None;
    let mut released = false;
    for s in &slots {
        let slot = s["slot"].as_u64().unwrap_or(0);
        ui.horizontal(|ui| {
            let mut on = !s["bypass"].as_bool().unwrap_or(false);
            if ui.checkbox(&mut on, s["name"].as_str().unwrap_or("Effect")).changed() {
                actions.push(("wave.rack_bypass", json!({"slot": slot, "on": !on})));
            }
            if ui.small_button("✕").on_hover_text("Remove").clicked() {
                actions.push(("wave.rack_remove", json!({"slot": slot})));
            }
        });
        let params = s["params"].as_array().map(Vec::as_slice).unwrap_or_default();
        if params.iter().any(|p| p["id"].as_str().is_some_and(crate::restore::is_band_param)) {
            let captured =
                params.iter().any(|p| p["id"].as_str().is_some_and(crate::restore::is_band_param) && p["value"].as_f64().is_some_and(|v| v > -159.5));
            ui.label(if captured { "Noise print: from the capture" } else { "Noise print: none (capture one, then add the effect again)" });
        }
        for p in params {
            let (Some(id), Some(min), Some(max)) = (p["id"].as_str(), p["min"].as_f64(), p["max"].as_f64()) else { continue };
            if crate::restore::is_band_param(id) {
                continue;
            }
            let mut v = p["value"].as_f64().unwrap_or(min);
            let name = p["name"].as_str().unwrap_or(id);
            let info = s["plugin"].as_str().and_then(soundcraft_dsp::plugin_info).and_then(|i| i.param(id));
            // Choices and switches change in one click: a step of their own, no merging.
            match info.map(|i| (i.unit, i.choices)) {
                Some((soundcraft_dsp::Unit::Toggle, _)) => {
                    let mut on = v >= 0.5;
                    if ui.checkbox(&mut on, name).changed() {
                        actions.push(("wave.rack_param", json!({"slot": slot, "param": id, "value": if on { 1.0 } else { 0.0 }})));
                    }
                    continue;
                }
                Some((soundcraft_dsp::Unit::Choice, choices)) => {
                    let mut pick = v.round().max(0.0) as usize;
                    let current = choices.get(pick).copied().unwrap_or_default();
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(("wave_rack_choice", slot, id)).selected_text(current).show_ui(ui, |ui| {
                            for (i, c) in choices.iter().enumerate() {
                                ui.selectable_value(&mut pick, i, *c);
                            }
                        });
                        ui.label(name);
                    });
                    if pick != v.round().max(0.0) as usize {
                        actions.push(("wave.rack_param", json!({"slot": slot, "param": id, "value": pick})));
                    }
                    continue;
                }
                _ => {}
            }
            let r = ui.add(egui::Slider::new(&mut v, min..=max).text(name));
            if r.changed() {
                merged = Some((json!({"slot": slot, "param": id, "value": v}), format!("rack:{slot}:{id}")));
            }
            released |= r.drag_stopped();
        }
    }
    if slots.is_empty() {
        ui.label("Empty: add an effect to preview it");
    }
    ui.horizontal(|ui| {
        let current = soundcraft_dsp::plugin_info(&app.add_effect).map_or("Effect", |p| p.name);
        egui::ComboBox::from_id_salt("wave_add_effect").selected_text(current).show_ui(ui, |ui| {
            for p in soundcraft_dsp::plugins().iter().filter(|p| !p.is_instrument && !soundcraft_dsp::is_generator(p.id)) {
                ui.selectable_value(&mut app.add_effect, p.id.to_string(), p.name);
            }
        });
        if ui.button("Add").clicked() {
            actions.push(("wave.rack_add", json!({"plugin": app.add_effect})));
        }
        if ui
            .add_enabled(!slots.is_empty(), egui::Button::new("Apply"))
            .on_hover_text("Bake the rack into the selection (or the whole file)")
            .clicked()
        {
            actions.push(("wave.rack_apply", json!({})));
        }
    });
    if let Some((p, key)) = merged
        && let Err(err) = app.engine.execute_merged("wave.rack_param", &p, &key)
    {
        app.status = err.to_string();
    }
    if released {
        app.engine.end_merge();
    }
    for (id, p) in actions {
        app.run(id, p);
    }
}
