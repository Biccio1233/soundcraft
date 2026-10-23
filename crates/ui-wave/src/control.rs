//! WaveCraft's control channel: the JSON requests a running wavecraft answers (same method names and
//! shapes as SoundCraft's, see docs/control-protocol.md), so `soundcraft_automation::Remote` and
//! `wavecraft-cli mcp --connect` drive it unchanged.

use crate::WaveApp;
use egui::{Event, Modifiers, PointerButton, pos2};
use serde_json::{Value, json};
use soundcraft_ui_egui::ControlRequest;
use soundcraft_ui_egui::control::{base64, png_bytes};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};

/// Requests answered per frame at most (the rest wait for the next frame).
const PER_FRAME: usize = 64;
/// Frames a screenshot may wait for the window to present one.
const SHOT_DEADLINE_FRAMES: u32 = 120;

/// The control channel's state inside the app.
#[derive(Default)]
pub struct Control {
    rx: Option<Receiver<ControlRequest>>,
    /// Synthetic input, one batch of events per frame.
    input: VecDeque<Vec<Event>>,
    shots: Vec<Shot>,
    next_token: u64,
    quit: bool,
}

struct Shot {
    token: u64,
    reply: Sender<Value>,
    path: Option<String>,
    frames: u32,
    sent: bool,
}

fn ok(v: Value) -> Value {
    json!({"ok": true, "result": v})
}

fn err(e: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": e.to_string()})
}

impl WaveApp {
    /// Answer control requests arriving on `rx` (see `control_server` in the app).
    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control.rx = Some(rx);
        self
    }

    /// `app.quit` was requested: the host closes the window.
    pub fn quit_requested(&self) -> bool {
        self.control.quit
    }

    /// Merge this frame's synthetic input into the raw input (call from eframe's raw-input hook).
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        if let Some(batch) = self.control.input.pop_front() {
            raw.events.extend(batch);
        }
    }
}

fn mods(p: &Value) -> Modifiers {
    let on = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mut m = Modifiers::NONE;
    if on("cmd") {
        m |= Modifiers::COMMAND;
    }
    if on("shift") {
        m |= Modifiers::SHIFT;
    }
    if on("alt") {
        m |= Modifiers::ALT;
    }
    if on("ctrl") {
        m |= Modifiers::CTRL;
    }
    m
}

/// Answer what arrived since the last frame, queue synthetic input and collect screenshots.
pub fn drain(app: &mut WaveApp, ctx: &egui::Context) {
    if let Some(rx) = app.control.rx.take() {
        for _ in 0..PER_FRAME {
            let Ok(req) = rx.try_recv() else { break };
            if let Some(reply) = handle(app, &req) {
                let _ = req.reply.send(reply);
            }
        }
        app.control.rx = Some(rx);
    }
    collect_screenshots(app, ctx);
    if !app.control.input.is_empty() || !app.control.shots.is_empty() {
        ctx.request_repaint();
    }
}

/// One request; `None` when the reply comes later (screenshots).
fn handle(app: &mut WaveApp, req: &ControlRequest) -> Option<Value> {
    let p = &req.params;
    let f = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let at = pos2(f("x"), f("y"));
    Some(match req.method.as_str() {
        "engine.execute" | "command" => {
            let id = p.get("command").or_else(|| p.get("id")).and_then(Value::as_str).unwrap_or("");
            let params = p.get("params").cloned().unwrap_or(json!({}));
            match app.engine.execute(id, &params) {
                Ok(v) => ok(v),
                Err(e) => err(e),
            }
        }
        "engine.commands" => match app.engine.execute("engine.commands", p) {
            Ok(v) => ok(v),
            Err(e) => err(e),
        },
        "session.inspect" | "document.inspect" => {
            let mut q = |id: &str| app.engine.execute(id, &json!({})).unwrap_or(Value::Null);
            let (wave, history, rack) = (q("wave.info"), q("wave.history"), q("wave.rack_list"));
            let full = p.get("detail").and_then(Value::as_str) == Some("full");
            ok(json!({"wave": wave, "history": history, "rack": rack, "session": soundcraft_engine::inspect::session(&app.engine, full)}))
        }
        "ui.inspect" => {
            let t = app.timeline;
            let z = app.engine.session().edit.zoom;
            ok(json!({
                "timeline": [t.min.x, t.min.y, t.max.x, t.max.y],
                "samples_per_px": z.samples_per_px,
                "scroll": z.scroll,
                "spectral": app.engine.wave.as_ref().is_some_and(|w| w.spectral),
                "playing": app.is_playing(),
                "status": app.status,
            }))
        }
        "ui.move" => {
            app.control.input.push_back(vec![Event::PointerMoved(at)]);
            ok(json!({}))
        }
        "ui.click" => {
            let button = match p.get("button").and_then(Value::as_str) {
                Some("right") | Some("secondary") => PointerButton::Secondary,
                _ => PointerButton::Primary,
            };
            let m = mods(p);
            app.control.input.push_back(vec![Event::PointerMoved(at)]);
            for _ in 0..p.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 3) {
                app.control.input.push_back(vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: m }]);
                app.control.input.push_back(vec![Event::PointerButton { pos: at, button, pressed: false, modifiers: m }]);
            }
            ok(json!({}))
        }
        "ui.drag" => {
            let to = pos2(f("to_x"), f("to_y"));
            let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(8).clamp(1, 200);
            let m = mods(p);
            app.control.input.push_back(vec![Event::PointerMoved(at)]);
            app.control.input.push_back(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: m }]);
            for i in 1..=steps {
                app.control.input.push_back(vec![Event::PointerMoved(at + (to - at) * (i as f32 / steps as f32))]);
            }
            app.control.input.push_back(vec![Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: m }]);
            ok(json!({}))
        }
        "ui.key" => {
            let Some(key) = p.get("key").and_then(Value::as_str).and_then(egui::Key::from_name) else {
                return Some(err("unknown `key` (Space, Enter, A, F7, =…)"));
            };
            let m = mods(p);
            app.control.input.push_back(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m }]);
            app.control.input.push_back(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m }]);
            ok(json!({}))
        }
        "ui.text" => {
            let text = p.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
            app.control.input.push_back(vec![Event::Text(text)]);
            ok(json!({}))
        }
        "ui.screenshot" => {
            app.control.next_token = app.control.next_token.wrapping_add(1);
            let path = p.get("path").and_then(Value::as_str).map(str::to_string);
            app.control.shots.push(Shot { token: app.control.next_token, reply: req.reply.clone(), path, frames: 0, sent: false });
            return None;
        }
        "app.quit" => {
            app.control.quit = true;
            ok(json!({}))
        }
        other => err(format!("unknown method `{other}`")),
    })
}

/// Ask the window for screenshots and answer the requests waiting for them (as SoundCraft does).
fn collect_screenshots(app: &mut WaveApp, ctx: &egui::Context) {
    if app.control.shots.is_empty() {
        return;
    }
    let images: Vec<(u64, std::sync::Arc<egui::ColorImage>)> = ctx.input(|i| {
        i.raw
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Screenshot { user_data, image, .. } => {
                    user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).map(|t| (*t, image.clone()))
                }
                _ => None,
            })
            .collect()
    });
    let mut keep = Vec::new();
    for mut s in std::mem::take(&mut app.control.shots) {
        if let Some((_, img)) = images.iter().find(|(t, _)| *t == s.token) {
            let _ = s.reply.send(encode(img, s.path.as_deref()));
            continue;
        }
        s.frames += 1;
        if !s.sent && s.frames >= 2 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(s.token)));
            s.sent = true;
        }
        if s.frames > SHOT_DEADLINE_FRAMES {
            let _ = s.reply.send(err("no frame was presented (is the window visible?)"));
            continue;
        }
        keep.push(s);
    }
    app.control.shots = keep;
}

fn encode(img: &egui::ColorImage, path: Option<&str>) -> Value {
    let bytes = match png_bytes(img) {
        Ok(b) => b,
        Err(e) => return err(e),
    };
    match path {
        Some(p) => match std::fs::write(p, &bytes) {
            Ok(()) => ok(json!({"path": p, "width": img.size[0], "height": img.size[1]})),
            Err(e) => err(format!("{p}: {e}")),
        },
        None => ok(json!({"png_base64": base64(&bytes), "width": img.size[0], "height": img.size[1]})),
    }
}
