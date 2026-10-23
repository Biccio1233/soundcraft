//! WaveCraft's UI driven like a user: clicks and drags through the accessibility tree.

use egui::{Event, Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_ui_egui::Services;
use soundcraft_ui_wave::WaveApp;
use soundcraft_ui_wave::menus::WAVE_MENUS;

/// An engine with a 2 s mono 48 kHz noise file open.
fn opened(test: &str) -> Engine {
    let dir = std::env::temp_dir().join("wavecraft-ui-tests").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = 7u32;
    let ch: Vec<f32> = (0..96_000)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        })
        .collect();
    let path = dir.join("take.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    std::fs::write(&path, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![ch] }, &opts).unwrap()).unwrap();
    let mut e = Engine::default();
    e.execute("wave.open", &json!({"path": path.to_str().unwrap()})).unwrap();
    e
}

fn harness(engine: Engine) -> Harness<'static, WaveApp> {
    let app = WaveApp::new(engine, None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut WaveApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run();
    h
}

fn exec(h: &mut Harness<'static, WaveApp>, id: &str, p: Value) -> Value {
    h.state_mut().engine.execute(id, &p).unwrap()
}

#[test]
fn history_rows_click_jumps() {
    let mut e = opened("history_click");
    e.execute("wave.gain", &json!({"db": 1.0})).unwrap();
    e.execute("wave.gain", &json!({"db": 2.0})).unwrap();
    let mut h = harness(e);
    h.get_by_label("Open").click();
    h.run();
    let rows = exec(&mut h, "wave.history", json!({}));
    assert_eq!(rows["rows"][0]["state"], "current", "{rows}");
}

#[test]
fn drag_selects_a_range() {
    let mut h = harness(opened("drag_select"));
    let (x0, x1, y) = {
        let app = h.state();
        (app.x_for(24_000), app.x_for(72_000), app.timeline_rect().center().y)
    };
    let press =
        |pressed: bool, x: f32| Event::PointerButton { pos: pos2(x, y), button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
    h.input_mut().events.push(Event::PointerMoved(pos2(x0, y)));
    h.step();
    h.input_mut().events.push(press(true, x0));
    h.step();
    for i in 1..=10 {
        let x = x0 + (x1 - x0) * i as f32 / 10.0;
        h.input_mut().events.push(Event::PointerMoved(pos2(x, y)));
        h.step();
    }
    h.input_mut().events.push(press(false, x1));
    h.run();
    let info = exec(&mut h, "wave.info", json!({}));
    let spp = 96_000.0 / h.state().timeline_rect().width() as f64;
    let (start, end) = (info["selection"]["start"].as_f64().unwrap(), info["selection"]["end"].as_f64().unwrap());
    assert!((start - 24_000.0).abs() <= spp + 1.0 && (end - 72_000.0).abs() <= spp + 1.0, "{info}");
}

#[test]
fn rack_apply_button_runs_apply() {
    let mut e = opened("rack_apply_button");
    e.execute("wave.rack_add", &json!({"plugin": "gain", "params": {"gain": 3.0}})).unwrap();
    let mut h = harness(e);
    h.get_by_label("Apply").click();
    h.run();
    assert_eq!(h.state().engine.undo_label(), Some("Apply Rack (1 effect)"));
    assert!(exec(&mut h, "wave.rack_list", json!({}))["slots"].as_array().unwrap().is_empty());
}

#[test]
fn menus_map_to_existing_commands() {
    for item in WAVE_MENUS {
        assert!(soundcraft_engine::find_command(item.command).is_some(), "{} → {}", item.label, item.command);
        if !item.params.is_empty() {
            assert!(serde_json::from_str::<Value>(item.params).is_ok(), "{}: bad params JSON", item.label);
        }
    }
}

#[test]
fn every_wave_command_is_reachable() {
    let src = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
        .unwrap()
        .map(|f| std::fs::read_to_string(f.unwrap().path()).unwrap())
        .collect::<String>();
    let unreached: Vec<&str> = soundcraft_engine::command_specs()
        .iter()
        .filter(|c| c.id.starts_with("wave.") && c.journal && !c.view)
        .filter(|c| !WAVE_MENUS.iter().any(|m| m.command == c.id) && !src.contains(&format!("\"{}\"", c.id)))
        .map(|c| c.id)
        .collect();
    assert!(unreached.is_empty(), "wave commands with no menu or UI reference: {unreached:?}");
}
