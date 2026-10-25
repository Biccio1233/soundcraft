//! The Editor: an overview of the whole file, a time ruler, the waveform with the selection, and
//! the transport. Drag to select, click to place the cursor, scroll to move, Cmd+scroll to zoom.

use crate::{WaveApp, analysis, clock, spectral};
use egui::{Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;
use soundcraft_model::ClipContent;
use soundcraft_time::Samples;
use soundcraft_ui_egui::edit_window::{sample_at, x_of};
use soundcraft_ui_egui::theme::Tokens;
use std::hash::{Hash, Hasher};

const OVERVIEW_H: f32 = 18.0;
const RULER_H: f32 = 20.0;
const TRANSPORT_H: f32 = 32.0;

pub fn show(ui: &mut egui::Ui, app: &mut WaveApp) {
    let t = Tokens::current();
    let full = ui.available_rect_before_wrap();
    let overview = Rect::from_min_size(full.min, vec2(full.width(), OVERVIEW_H));
    let ruler = Rect::from_min_size(pos2(full.min.x, overview.max.y + 2.0), vec2(full.width(), RULER_H));
    let lanes = analysis::lanes(app);
    let lanes_r = Rect::from_min_size(pos2(full.min.x, ruler.max.y), vec2(full.width(), if lanes.is_some() { analysis::LANES_H } else { 0.0 }));
    let timeline = Rect::from_min_max(pos2(full.min.x, lanes_r.max.y), pos2(full.max.x, (full.max.y - TRANSPORT_H).max(lanes_r.max.y + 10.0)));
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
    if let Some(l) = &lanes {
        analysis::draw_lanes(&painter, app, l, lanes_r, &t);
    }
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

/// The waveform area and, when the spectral view is on, the spectral area under it.
fn split(app: &WaveApp, tl: Rect) -> (Rect, Option<Rect>) {
    if !app.engine.wave.as_ref().is_some_and(|w| w.spectral) {
        return (tl, None);
    }
    let y = tl.min.y + tl.height() * app.spectral_split;
    (Rect::from_min_max(tl.min, pos2(tl.max.x, y)), Some(Rect::from_min_max(pos2(tl.min.x, y + 2.0), tl.max)))
}

/// A key of everything the spectral texture depends on: the clips, the view, the area.
fn spectral_key(app: &WaveApp, r: Rect) -> u64 {
    let s = app.engine.session();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(track) = s.tracks.first() {
        for c in track.clips() {
            (c.id.0, c.start, c.length, c.gain_db.to_bits(), c.fade_in.len, c.fade_out.len).hash(&mut h);
            if let ClipContent::Audio { source, offset } = c.content {
                (source.0, offset).hash(&mut h);
            }
        }
    }
    (s.edit.zoom.scroll, s.edit.zoom.samples_per_px.to_bits(), r.width().to_bits(), r.height().to_bits(), app.sample_rate()).hash(&mut h);
    h.finish()
}

/// Draw the spectral view of the visible audio, one band per channel, recomputed only when its key changes.
fn draw_spectral(ui: &egui::Ui, app: &mut WaveApp, r: Rect, t: &Tokens) {
    let painter = ui.painter_at(r);
    painter.rect_filled(r, 0.0, Color32::from_rgb(8, 4, 20));
    let key = spectral_key(app, r);
    // While a drag or a scroll is under way the old texture stays (stretched): recomputing every
    // frame of a gesture would stall the UI.
    let busy = ui.input(|i| i.pointer.any_down() || i.smooth_scroll_delta != egui::Vec2::ZERO);
    let stale = app.spectral_cache.as_ref().is_none_or(|(k, _)| *k != key);
    if stale && (app.spectral_cache.is_none() || !busy) {
        let s = app.engine.session();
        let channels = app.engine.wave.as_ref().map_or(1, |w| w.channels).max(1);
        let columns = (r.width().round().max(1.0) as usize).min(2048);
        let rows = ((r.height() / channels as f32).round().max(8.0) as usize).min(256);
        let hop = s.edit.zoom.samples_per_px.round().max(1.0) as usize;
        let start = s.edit.zoom.scroll;
        let track = s.tracks.first().map(|t| t.id);
        let render = |at: Samples, len: usize| track.map(|t| render_audio(s, t, at, len as Samples)).unwrap_or_default();
        let tiles = spectral::tiles_from(render, start, hop, columns, rows, app.sample_rate(), channels);
        let pixels: Vec<Color32> = tiles.iter().flat_map(|t| t.image().pixels).collect();
        if pixels.len() == columns * rows * channels {
            let image = egui::ColorImage::new([columns, rows * channels], pixels);
            let tex = ui.ctx().load_texture("wave_spectral", image, egui::TextureOptions::LINEAR);
            app.spectral_cache = Some((key, tex));
            app.spectral_renders = app.spectral_renders.wrapping_add(1);
        }
    }
    if let Some((_, tex)) = &app.spectral_cache {
        painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let channels = app.engine.wave.as_ref().map_or(1, |w| w.channels).max(1);
    let band_h = r.height() / channels as f32;
    for c in 0..channels {
        let bottom = r.min.y + band_h * (c + 1) as f32;
        for (hz, label) in [(100.0, "100 Hz"), (1_000.0, "1 kHz"), (10_000.0, "10 kHz")] {
            let y = bottom - spectral::axis_fraction(hz, app.sample_rate()) * band_h;
            painter.line_segment([pos2(r.max.x - 8.0, y), pos2(r.max.x, y)], Stroke::new(1.0, t.ruler_tick));
            painter.text(pos2(r.max.x - 10.0, y), egui::Align2::RIGHT_CENTER, label, egui::FontId::monospace(10.0), t.ruler_text);
        }
        if c > 0 {
            painter.line_segment([pos2(r.min.x, bottom - band_h), pos2(r.max.x, bottom - band_h)], Stroke::new(1.0, t.border));
        }
    }
}

/// The file's samples over `start..start + len` (clips only, no mixer).
fn render_audio(s: &soundcraft_model::Session, track: soundcraft_model::TrackId, start: Samples, len: Samples) -> Vec<Vec<f32>> {
    soundcraft_mix::render_clips(s, track, soundcraft_time::Range::new(start, start.saturating_add(len)))
}

fn draw_wave(ui: &egui::Ui, app: &mut WaveApp, full: Rect, t: &Tokens) {
    let (tl, spec) = split(app, full);
    if let Some(r) = spec {
        draw_spectral(ui, app, r, t);
    }
    let painter = ui.painter_at(full);
    let s = app.engine.session();
    let channels = app.engine.wave.as_ref().map_or(1, |w| w.channels);
    let sel = app.selection();
    if !sel.is_empty() {
        let (x0, x1) = (x_of(s, full, sel.start), x_of(s, full, sel.end));
        painter.rect_filled(Rect::from_min_max(pos2(x0, full.min.y), pos2(x1, full.max.y)), 0.0, t.selection);
    }
    let painter = ui.painter_at(tl);
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
        Some(p) => (app.file_position(p.position()), t.playhead),
        None => (sel.start, t.insertion),
    };
    let x = x_of(s, full, at);
    ui.painter_at(full).line_segment([pos2(x, full.min.y), pos2(x, full.max.y)], Stroke::new(1.0, color));
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
    // The splitter between waveform and spectral view (added after the timeline, so it wins).
    if let (_, Some(spec)) = split(app, tl) {
        let handle = Rect::from_min_max(pos2(tl.min.x, spec.min.y - 5.0), pos2(tl.max.x, spec.min.y + 3.0));
        let h = ui.interact(handle, ui.id().with("wave_splitter"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
        if h.dragged()
            && let Some(p) = h.interact_pointer_pos()
        {
            app.spectral_split = ((p.y - tl.min.y) / tl.height().max(1.0)).clamp(0.1, 0.85);
        }
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
