//! Render WaveCraft's UI offscreen (no window, no focus stealing) and save a PNG.
//!
//! `cargo run -p soundcraft-ui-wave --example wave_shot -- out.png [audio-file] [script.jsonl] [--size WxH]`
//!
//! Script lines are `{"command": "wave.gain", "params": {...}}` (run on the engine),
//! `{"menu": "Noise Reduction…"}` (invoke the menu item with that label), `{"shot": "path.png"}` or
//! `{"steps": n}`.

use serde_json::Value;
use soundcraft_ui_egui::Services;
use soundcraft_ui_wave::WaveApp;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "wavecraft.png".into());
    let file = args.get(1).filter(|a| !a.starts_with("--") && !a.ends_with(".jsonl")).cloned();
    let script = args.iter().skip(1).find(|a| a.ends_with(".jsonl")).cloned();
    let (w, h) = args
        .iter()
        .position(|a| a == "--size")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)))
        .unwrap_or((1512.0, 898.0));
    let mut engine = soundcraft_engine::Engine::default();
    if let Some(f) = &file
        && let Err(e) = engine.execute("wave.open", &serde_json::json!({"path": f}))
    {
        eprintln!("{f}: {e}");
    }
    let app = WaveApp::new(engine, None, Services::default());
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(w, h)).with_pixels_per_point(2.0).wgpu().build_ui_state(
        |ui, app: &mut WaveApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.input_mut().max_texture_side = Some(8192);
    for _ in 0..6 {
        harness.step();
    }
    let shoot = |h: &mut egui_kittest::Harness<WaveApp>, path: &str| match h.render() {
        Ok(img) => match img.save(path) {
            Ok(()) => println!("wrote {path}"),
            Err(e) => eprintln!("{path}: {e}"),
        },
        Err(e) => eprintln!("render failed: {e}"),
    };
    if let Some(script) = script {
        for line in std::fs::read_to_string(&script).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                eprintln!("bad line: {line}");
                continue;
            };
            if let Some(cmd) = v.get("command").and_then(Value::as_str) {
                let p = v.get("params").cloned().unwrap_or(Value::Null);
                if let Err(e) = harness.state_mut().engine.execute(cmd, &p) {
                    eprintln!("{cmd}: {e}");
                }
                harness.run();
            } else if let Some(label) = v.get("menu").and_then(Value::as_str) {
                match soundcraft_ui_wave::menus::WAVE_MENUS.iter().find(|m| m.label == label) {
                    Some(item) => soundcraft_ui_wave::menus::invoke(harness.state_mut(), item),
                    None => eprintln!("no menu item `{label}`"),
                }
                harness.run();
            } else if let Some(path) = v.get("shot").and_then(Value::as_str) {
                shoot(&mut harness, path);
            } else if let Some(n) = v.get("steps").and_then(Value::as_u64) {
                for _ in 0..n {
                    harness.step();
                }
            }
        }
    }
    harness.run();
    shoot(&mut harness, &out);
}
