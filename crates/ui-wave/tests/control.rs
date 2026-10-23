//! WaveCraft's control channel: the requests an agent sends to a running wavecraft.

use egui::vec2;
use egui_kittest::Harness;
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_ui_egui::{ControlRequest, Services};
use soundcraft_ui_wave::WaveApp;
use std::sync::mpsc::{Receiver, Sender, channel};

fn fixture(test: &str) -> String {
    let dir = std::env::temp_dir().join("wavecraft-ui-tests").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("take.wav");
    let ch: Vec<f32> = (0..96_000).map(|i| ((i % 100) as f32 / 100.0) - 0.5).collect();
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    std::fs::write(&path, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![ch] }, &opts).unwrap()).unwrap();
    path.to_str().unwrap().to_string()
}

fn harness() -> (Harness<'static, WaveApp>, Sender<ControlRequest>) {
    let (tx, rx) = channel();
    let app = WaveApp::new(Engine::default(), None, Services::default()).with_control(rx);
    let h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut WaveApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    (h, tx)
}

/// One frame, with the app's synthetic input merged in (what eframe's raw-input hook does).
fn step(h: &mut Harness<'static, WaveApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

/// Send a request and run frames until it is answered.
fn call(h: &mut Harness<'static, WaveApp>, tx: &Sender<ControlRequest>, method: &str, params: Value) -> Value {
    let (req, rx): (ControlRequest, Receiver<Value>) = ControlRequest::new(method, params);
    tx.send(req).unwrap();
    for _ in 0..60 {
        step(h);
        if let Ok(v) = rx.try_recv() {
            return v;
        }
    }
    panic!("{method}: no reply");
}

#[test]
fn control_execute_and_inspect() {
    let (mut h, tx) = harness();
    let path = fixture("control_exec");
    let r = call(&mut h, &tx, "engine.execute", json!({"command": "wave.open", "params": {"path": path}}));
    assert_eq!(r["ok"], true, "{r}");
    let r = call(&mut h, &tx, "command", json!({"command": "wave.info"}));
    assert_eq!(r["result"]["frames"], 96_000, "{r}");
    let r = call(&mut h, &tx, "session.inspect", json!({}));
    assert_eq!(r["result"]["wave"]["frames"], 96_000, "{r}");
    assert!(r["result"]["history"]["rows"].is_array(), "{r}");
    let r = call(&mut h, &tx, "ui.inspect", json!({}));
    assert!(r["result"]["timeline"].as_array().is_some_and(|a| a.len() == 4), "{r}");
    let r = call(&mut h, &tx, "engine.execute", json!({"command": "wave.gain", "params": {"db": 200}}));
    assert_eq!(r["ok"], false, "{r}");
    let r = call(&mut h, &tx, "no.such.method", json!({}));
    assert_eq!(r["ok"], false, "{r}");
}

#[test]
fn control_drag_selects_through_synthetic_input() {
    let (mut h, tx) = harness();
    let path = fixture("control_drag");
    call(&mut h, &tx, "engine.execute", json!({"command": "wave.open", "params": {"path": path}}));
    for _ in 0..3 {
        step(&mut h);
    }
    let (x0, x1, y) = {
        let app = h.state();
        (app.x_for(24_000), app.x_for(72_000), app.timeline_rect().center().y)
    };
    let r = call(&mut h, &tx, "ui.drag", json!({"x": x0, "y": y, "to_x": x1, "to_y": y, "steps": 10}));
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..20 {
        step(&mut h);
    }
    let info = call(&mut h, &tx, "command", json!({"command": "wave.info"}));
    let spp = 96_000.0 / h.state().timeline_rect().width() as f64;
    let start = info["result"]["selection"]["start"].as_f64().unwrap();
    let end = info["result"]["selection"]["end"].as_f64().unwrap();
    assert!((start - 24_000.0).abs() <= spp + 1.0 && (end - 72_000.0).abs() <= spp + 1.0, "{info}");
}

#[test]
fn app_quit_is_requested() {
    let (mut h, tx) = harness();
    assert!(!h.state().quit_requested());
    let r = call(&mut h, &tx, "app.quit", json!({}));
    assert_eq!(r["ok"], true);
    assert!(h.state().quit_requested());
}
