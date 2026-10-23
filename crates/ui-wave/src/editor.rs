//! The Editor: an overview of the whole file, a time ruler, the waveform with the selection, and
//! the transport. Drag to select, click to place the cursor, scroll to move, Cmd+scroll to zoom.

use crate::{WaveApp, clock};
use egui::{Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;
use soundcraft_model::ClipContent;
use soundcraft_time::Samples;
use soundcraft_ui_egui::edit_window::{sample_at, x_of};
use soundcraft_ui_egui::theme::Tokens;

const OVERVIEW_H: f32 = 18.0;
const RULER_H: f32 = 20.0;
const TRANSPORT_H: f32 = 32.0;

pub fn show(ui: &mut egui::Ui, app: &mut WaveApp) {
    let t = Tokens::current();
    let full = ui.available_rect_before_wrap();
    let overview = Rect::from_min_size(full.min, vec2(full.width(), OVERVIEW_H));
    let ruler = Rect::from_min_size(pos2(full.min.x, overview.max.y + 2.0), vec2(full.width(), RULER_H));
    let timeline = Rect::from_min_max(pos2(full.min.x, ruler.max.y), pos2(full.max.x, (full.max.y - TRANSPORT_H).max(ruler.max.y + 10.0)));
    let transport = Rect::from_min_max(pos2(full.min.x, timeline.max.y), full.max);
    app.timeline = timeline;
    let len = app.file_len();
    fit_new_file(app, timeline);
    let painter = ui.painter_at(full);
    painter.rect_filled(full, 0.0, t.playlist_bg);
    if app.engine.wave.is_none() {
        painter.text(timeline.center(), egui::Align2::CENTER_CENTER, "Open a file (Cmd+O)", egui::FontId::proportional(16.0), t.text_dim);
        return;
    }
    draw_overview(&painter, app, overview, len, &t);
    draw_ruler(&painter, app, ruler, &t);
    draw_wave(ui, app, timeline, &t);
    interact(ui, app, overview, timeline, len);
    transport_bar(ui, app, transport);
}

/// Show a newly opened file whole.
fn fit_new_file(app: &mut WaveApp, timeline: Rect) {
    let path = app.engine.wave.as_ref().and_then(|w| w.path.clone());
    if path.is_some() && path != app.fitted_path && timeline.width() > 10.0 {
        let len = app.file_len().max(1);
        let z = app.engine.zoom_mut();
        z.samples_per_px = (len as f64 / f64::from(timeline.width())).max(0.01);
        z.scroll = 0;
        app.fitted_path = path;
    }
}

fn zoom_to(app: &mut WaveApp, timeline: Rect, samples_per_px: f64, anchor_x: f32) {
    let s = app.engine.session();
    let at = sample_at(s, timeline, anchor_x);
    let max = (app.file_len().max(1) as f64 / f64::from(timeline.width().max(1.0))).max(0.01) * 4.0;
    let spp = samples_per_px.clamp(0.01, max);
    let z = app.engine.zoom_mut();
    z.samples_per_px = spp;
    z.scroll = (at as f64 - f64::from(anchor_x - timeline.min.x) * spp).max(0.0) as Samples;
}

fn draw_overview(painter: &egui::Painter, app: &WaveApp, r: Rect, len: Samples, t: &Tokens) {
    painter.rect_filled(r, 2.0, t.ruler_bg);
    if len <= 0 {
        return;
    }
    let z = app.engine.session().edit.zoom;
    let frac = |s: Samples| (s as f64 / len as f64).clamp(0.0, 1.0) as f32;
    let view_end = z.scroll.saturating_add((z.samples_per_px * f64::from(app.timeline.width())) as Samples);
    let x0 = r.min.x + r.width() * frac(z.scroll);
    let x1 = r.min.x + r.width() * frac(view_end);
    painter.rect_stroke(
        Rect::from_min_max(pos2(x0, r.min.y + 1.0), pos2(x1.max(x0 + 2.0), r.max.y - 1.0)),
        2.0,
        Stroke::new(1.5, t.accent),
        egui::StrokeKind::Inside,
    );
    let sel = app.selection();
    if !sel.is_empty() {
        painter.rect_filled(
            Rect::from_min_max(pos2(r.min.x + r.width() * frac(sel.start), r.min.y + 4.0), pos2(r.min.x + r.width() * frac(sel.end), r.max.y - 4.0)),
            0.0,
            t.selection,
        );
    }
}

/// A tick every `step` seconds, with `step` the smallest that leaves room for a label.
fn draw_ruler(painter: &egui::Painter, app: &WaveApp, r: Rect, t: &Tokens) {
    painter.rect_filled(r, 0.0, t.ruler_bg);
    let sr = f64::from(app.sample_rate());
    let s = app.engine.session();
    let px_per_sec = sr / s.edit.zoom.samples_per_px.max(0.01);
    let steps = [0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0];
    let step = steps.iter().copied().find(|st| st * px_per_sec >= 80.0).unwrap_or(600.0);
    let first = (s.edit.zoom.scroll as f64 / sr / step).floor() as i64;
    for k in first.. {
        let at = (k as f64 * step * sr) as Samples;
        let x = x_of(s, app.timeline, at);
        if x > r.max.x {
            break;
        }
        if x < r.min.x {
            continue;
        }
        painter.line_segment([pos2(x, r.max.y - 6.0), pos2(x, r.max.y)], Stroke::new(1.0, t.ruler_tick));
        painter.text(pos2(x + 3.0, r.min.y + 2.0), egui::Align2::LEFT_TOP, clock(at, app.sample_rate()), egui::FontId::monospace(10.0), t.ruler_text);
    }
}

fn draw_wave(ui: &egui::Ui, app: &WaveApp, tl: Rect, t: &Tokens) {
    let painter = ui.painter_at(tl);
    let s = app.engine.session();
    let channels = app.engine.wave.as_ref().map_or(1, |w| w.channels);
    let sel = app.selection();
    if !sel.is_empty() {
        let (x0, x1) = (x_of(s, tl, sel.start), x_of(s, tl, sel.end));
        painter.rect_filled(Rect::from_min_max(pos2(x0, tl.min.y), pos2(x1, tl.max.y)), 0.0, t.selection);
    }
    let h = tl.height() / channels.max(1) as f32;
    for c in 1..channels {
        let y = tl.min.y + h * c as f32;
        painter.line_segment([pos2(tl.min.x, y), pos2(tl.max.x, y)], Stroke::new(1.0, t.border));
    }
    let wave = Color32::from_rgb(78, 201, 176);
    if let Some(track) = s.tracks.first() {
        for clip in track.clips() {
            let ClipContent::Audio { source, .. } = clip.content else { continue };
            let Some(audio) = s.pool.get(source) else { continue };
            let body =
                Rect::from_min_max(pos2(x_of(s, tl, clip.start), tl.min.y), pos2(x_of(s, tl, clip.start.saturating_add(clip.length)), tl.max.y));
            soundcraft_ui_egui::waveform::draw(&painter, s, tl, clip, audio, channels, body, wave);
        }
    }
    let (at, color) = match app.player.as_ref().filter(|p| p.is_playing()) {
        Some(p) => (p.position(), t.playhead),
        None => (sel.start, t.insertion),
    };
    let x = x_of(s, tl, at);
    painter.line_segment([pos2(x, tl.min.y), pos2(x, tl.max.y)], Stroke::new(1.0, color));
}

fn interact(ui: &mut egui::Ui, app: &mut WaveApp, overview: Rect, tl: Rect, len: Samples) {
    let clamp = |at: Samples| at.clamp(0, len.max(0));
    let r = ui.interact(tl, ui.id().with("wave_timeline"), Sense::click_and_drag());
    if r.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin())
    {
        app.drag_anchor = Some(clamp(sample_at(app.engine.session(), tl, p.x)));
    }
    if r.dragged()
        && let (Some(anchor), Some(p)) = (app.drag_anchor, r.interact_pointer_pos())
    {
        let at = clamp(sample_at(app.engine.session(), tl, p.x));
        app.run("wave.select", json!({"start": anchor.min(at), "end": anchor.max(at)}));
    }
    if r.drag_stopped() {
        app.drag_anchor = None;
    }
    if r.clicked()
        && let Some(p) = r.interact_pointer_pos()
    {
        let at = clamp(sample_at(app.engine.session(), tl, p.x));
        app.run("wave.select", json!({"start": at, "end": at}));
    }
    if r.hovered() {
        let (delta, zoom, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command, i.pointer.hover_pos()));
        let spp = app.engine.session().edit.zoom.samples_per_px;
        if zoom && delta.y != 0.0 {
            zoom_to(app, tl, spp * f64::from((-delta.y / 200.0).exp()), pointer.map_or(tl.center().x, |p| p.x));
        } else if delta != egui::Vec2::ZERO {
            let pan = -(if delta.x != 0.0 { delta.x } else { delta.y });
            let z = app.engine.zoom_mut();
            z.scroll = (z.scroll as f64 + f64::from(pan) * spp).clamp(0.0, len.max(0) as f64) as Samples;
        }
    }
    let o = ui.interact(overview, ui.id().with("wave_overview"), Sense::click_and_drag());
    if (o.clicked() || o.dragged())
        && let Some(p) = o.interact_pointer_pos()
        && len > 0
    {
        let at = (f64::from((p.x - overview.min.x) / overview.width().max(1.0)) * len as f64) as Samples;
        let z = app.engine.zoom_mut();
        let half = (z.samples_per_px * f64::from(tl.width()) / 2.0) as Samples;
        z.scroll = at.saturating_sub(half).max(0);
    }
}

fn transport_bar(ui: &mut egui::Ui, app: &mut WaveApp, r: Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    let playing = app.is_playing();
    if ui.button(if playing { "Stop" } else { "Play" }).clicked() {
        app.toggle_play();
    }
    ui.toggle_value(&mut app.looping, "Loop");
    let at = app.player.as_ref().filter(|p| p.is_playing()).map_or(app.selection().start, |p| p.position());
    ui.monospace(clock(at, app.sample_rate()));
    ui.separator();
    let tl = app.timeline;
    let spp = app.engine.session().edit.zoom.samples_per_px;
    if ui.button("−").on_hover_text("Zoom out").clicked() {
        zoom_to(app, tl, spp * 2.0, tl.center().x);
    }
    if ui.button("+").on_hover_text("Zoom in").clicked() {
        zoom_to(app, tl, spp / 2.0, tl.center().x);
    }
    if ui.button("Fit").on_hover_text("Show the whole file").clicked() {
        app.fitted_path = None;
    }
}
