//! The music analysis on screen: beat, chord and section lanes under the ruler, the Chords & Lyrics
//! window (chords above the words, the sung line highlighted) and the Chord Diagrams window. The
//! level and capo picked here apply to all three.

use crate::WaveApp;
use egui::{Color32, Rect, Stroke, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::cmd::wave_analysis::Analysis;
use soundcraft_ui_egui::edit_window::x_of;
use soundcraft_ui_egui::theme::Tokens;
use soundcraft_wave_analysis::chords::Grid;
use soundcraft_wave_analysis::key::Key;

/// Height of the lanes: chords on top, sections under them.
pub const LANES_H: f32 = 34.0;
const CHORD_LANE_H: f32 = 18.0;
const LEVELS: [(u8, &str); 3] = [(1, "Basic"), (2, "Intermediate"), (3, "Full")];
const DIAGRAM_W: f32 = 78.0;
const DIAGRAM_H: f32 = 104.0;

/// What the lanes show, in file seconds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lanes {
    /// Bar starts and their numbers (from 1).
    pub bars: Vec<(f64, usize)>,
    pub beats: Vec<f64>,
    /// Chord spans with their labels at the chosen level and capo (no "no chord" spans).
    pub chords: Vec<(f64, f64, String)>,
    /// Section spans: label and letter.
    pub sections: Vec<(f64, f64, String, char)>,
}

type CacheKey = (u64, u64, i64, i64, usize, usize, usize, u8, Option<u8>);

/// What the windows show, computed once per analysis, lyrics, level and capo.
struct Cache {
    key: CacheKey,
    capo: u8,
    txt: Vec<String>,
    /// Lyric lines: start (s) and the words joined by spaces, in order.
    lines: Vec<(f64, String)>,
    diagrams: Vec<Value>,
}

/// The level and capo the user picked, which windows are open, and what they show.
pub struct View {
    pub level: u8,
    /// `None`: the suggested capo.
    pub capo: Option<u8>,
    pub lyrics_open: bool,
    pub diagrams_open: bool,
    cache: Option<Cache>,
}

impl Default for View {
    fn default() -> Self {
        View { level: 3, capo: Some(0), lyrics_open: false, diagrams_open: false, cache: None }
    }
}

/// The analysis the screen can show: present, not stale.
fn current(app: &WaveApp) -> Option<&Analysis> {
    let a = app.engine.wave.as_ref()?.analysis.as_ref()?;
    (a.revision == app.engine.revision).then_some(a)
}

fn grid_name(g: Grid) -> &'static str {
    match g {
        Grid::Beat => "beat",
        Grid::Bar => "bar",
        Grid::Free => "free",
    }
}

/// Params that make the engine reuse the analysis on screen (same range, grid, bar length).
fn same_analysis(a: &Analysis) -> Value {
    let mut p = json!({"start": a.range.start, "end": a.range.end, "grid": grid_name(a.grid)});
    if let Some(b) = a.beats_per_bar {
        p["beats_per_bar"] = json!(b);
    }
    p
}

fn with_view(mut p: Value, view: &View) -> Value {
    p["level"] = json!(view.level);
    p["capo"] = view.capo.map_or(json!("auto"), |c| json!(c));
    p
}

/// The params of `wave.chordpro_export` for `path`, as the windows show the song.
pub fn export_params(app: &WaveApp, path: &str) -> Value {
    let mut p = current(app).map_or_else(|| json!({}), same_analysis);
    p = with_view(p, &app.analysis);
    p["path"] = json!(path);
    p
}

fn refresh(app: &mut WaveApp) {
    let Some(a) = current(app) else {
        app.analysis.cache = None;
        return;
    };
    let lyrics = app.engine.wave.as_ref().map_or((0, 0), |w| (w.lyrics.heard.len(), w.lyrics.lines.len()));
    let key: CacheKey =
        (app.engine.revision, a.revision, a.range.start, a.range.end, a.chords.len(), lyrics.0, lyrics.1, app.analysis.level, app.analysis.capo);
    if app.analysis.cache.as_ref().is_some_and(|c| c.key == key) {
        return;
    }
    let p = with_view(same_analysis(a), &app.analysis);
    let col = app.engine.execute("wave.chords_over_lyrics", &p).unwrap_or(Value::Null);
    let diagrams = app.engine.execute("wave.chord_diagrams", &p).unwrap_or(Value::Null);
    let lyrics = app.engine.execute("wave.lyrics", &json!({})).unwrap_or(Value::Null);
    let lines = lyrics["lines"]
        .as_array()
        .map(|ls| {
            ls.iter()
                .map(|l| {
                    let words: Vec<&str> = l["words"].as_array().map(|ws| ws.iter().filter_map(|w| w["text"].as_str()).collect()).unwrap_or_default();
                    (l["words"][0]["start"].as_f64().unwrap_or(0.0), words.join(" "))
                })
                .collect()
        })
        .unwrap_or_default();
    app.analysis.cache = Some(Cache {
        key,
        capo: col["capo"].as_u64().map_or(0, |c| c.min(7) as u8),
        txt: col["txt"].as_str().unwrap_or_default().lines().map(str::to_string).collect(),
        lines,
        diagrams: diagrams["chords"].as_array().cloned().unwrap_or_default(),
    });
}

/// The lanes to draw, when an analysis is shown.
pub fn lanes(app: &WaveApp) -> Option<Lanes> {
    if !app.engine.wave.as_ref()?.show_analysis {
        return None;
    }
    let a = current(app)?;
    let at = |t: f32| a.offset_s + f64::from(t);
    let capo = app.analysis.cache.as_ref().map_or(app.analysis.capo.unwrap_or(0), |c| c.capo);
    let key = a.key.as_ref().map(|k| Key { tonic: ((i16::from(k.tonic) - i16::from(capo)).rem_euclid(12)) as u8, ..k.clone() });
    let level = app.analysis.level;
    Some(Lanes {
        bars: a.tempo.downbeats.iter().enumerate().map(|(i, t)| (at(*t), i + 1)).collect(),
        beats: a.tempo.beats.iter().map(|t| at(*t)).collect(),
        chords: a
            .chords
            .iter()
            .filter_map(|s| s.chord.map(|c| (at(s.start), at(s.end), c.simplify(level).transpose(-(capo as i8)).label(key.as_ref()))))
            .collect(),
        sections: a.sections.iter().map(|s| (at(s.start), at(s.end), s.label.name().to_string(), s.letter)).collect(),
    })
}

/// Draw `lanes` in `r` (chords on top, sections under them) on the editor's time axis.
pub fn draw_lanes(painter: &egui::Painter, app: &WaveApp, lanes: &Lanes, r: Rect, t: &Tokens) {
    let s = app.engine.session();
    let sr = f64::from(app.sample_rate().max(1));
    let x = |secs: f64| x_of(s, app.timeline, (secs * sr).round() as i64);
    let chord_r = Rect::from_min_size(r.min, vec2(r.width(), CHORD_LANE_H));
    let section_r = Rect::from_min_max(pos2(r.min.x, chord_r.max.y), r.max);
    painter.rect_filled(chord_r, 0.0, t.ruler_bg);
    painter.rect_filled(section_r, 0.0, t.playlist_lane);
    for (start, end, label, letter) in &lanes.sections {
        let (x0, x1) = (x(*start).max(r.min.x), x(*end).min(r.max.x));
        if x1 <= x0 {
            continue;
        }
        let fill = if label == "chorus" {
            t.accent.gamma_multiply(0.45)
        } else if (*letter as u32).is_multiple_of(2) {
            t.panel_bg2
        } else {
            t.playlist_alt
        };
        let band = Rect::from_min_max(pos2(x0, section_r.min.y + 1.0), pos2(x1, section_r.max.y - 1.0));
        painter.rect_filled(band, 2.0, fill);
        painter.rect_stroke(band, 2.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        painter.text(
            pos2(x0 + 4.0, band.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{label} {letter}"),
            egui::FontId::proportional(10.0),
            t.text,
        );
    }
    let px_per_bar = lanes.bars.windows(2).next().map_or(0.0, |w| x(w[1].0) - x(w[0].0));
    for beat in &lanes.beats {
        let bx = x(*beat);
        if (r.min.x..=r.max.x).contains(&bx) {
            painter.line_segment([pos2(bx, chord_r.max.y - 3.0), pos2(bx, chord_r.max.y)], Stroke::new(1.0, t.ruler_tick));
        }
    }
    for (time, n) in &lanes.bars {
        let bx = x(*time);
        if !(r.min.x..=r.max.x).contains(&bx) {
            continue;
        }
        painter.line_segment([pos2(bx, chord_r.min.y), pos2(bx, chord_r.max.y)], Stroke::new(1.0, t.bar_line));
        // Section labels already mark the bars where a section starts.
        let section_start = lanes.sections.iter().any(|s| (s.0 - time).abs() < 0.2);
        if !section_start && (px_per_bar >= 28.0 || n % 4 == 1) {
            painter.text(pos2(bx + 2.0, section_r.max.y - 1.0), egui::Align2::LEFT_BOTTOM, n.to_string(), egui::FontId::monospace(8.0), t.text_dim);
        }
    }
    for (start, _, label) in &lanes.chords {
        let cx = x(*start);
        if cx > r.max.x || cx < r.min.x - 40.0 {
            continue;
        }
        painter.text(pos2(cx.max(r.min.x) + 3.0, chord_r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::monospace(11.0), t.text);
    }
}

/// Open a window, analysing the song first when there is nothing (or only stale results) to show.
pub fn open(app: &mut WaveApp, lyrics: bool) {
    if current(app).is_none() {
        app.run("wave.analyze", json!({}));
    }
    if lyrics {
        app.analysis.lyrics_open = true;
    } else {
        app.analysis.diagrams_open = true;
    }
}

fn controls(ui: &mut egui::Ui, view: &mut View, resolved_capo: u8) {
    ui.horizontal(|ui| {
        let level_name = LEVELS.iter().find(|l| l.0 == view.level).map_or("Full", |l| l.1);
        ui.label("Level");
        egui::ComboBox::from_id_salt("analysis_level").selected_text(level_name).show_ui(ui, |ui| {
            for (n, name) in LEVELS {
                ui.selectable_value(&mut view.level, n, name);
            }
        });
        let capo_text = match view.capo {
            None => format!("Auto ({resolved_capo})"),
            Some(0) => "None".to_string(),
            Some(c) => c.to_string(),
        };
        ui.label("Capo");
        egui::ComboBox::from_id_salt("analysis_capo").selected_text(capo_text).show_ui(ui, |ui| {
            ui.selectable_value(&mut view.capo, None, "Auto");
            ui.selectable_value(&mut view.capo, Some(0), "None");
            for c in 1..=7u8 {
                ui.selectable_value(&mut view.capo, Some(c), c.to_string());
            }
        });
    });
}

/// The txt line to highlight: the lyric line under the playhead (and its chord line above).
fn sung(cache: &Cache, now_s: f64) -> Vec<usize> {
    let Some(j) = cache.lines.iter().rposition(|(start, _)| *start <= now_s) else { return Vec::new() };
    let mut k = 0;
    for (i, line) in cache.txt.iter().enumerate() {
        if cache.lines.get(k).is_some_and(|(_, text)| text == line) {
            if k == j {
                let above = i
                    .checked_sub(1)
                    .filter(|p| cache.txt.get(*p).is_some_and(|l| !l.is_empty() && !l.ends_with(':') && !cache.lines.iter().any(|x| &x.1 == l)));
                return above.into_iter().chain(std::iter::once(i)).collect();
            }
            k += 1;
        }
    }
    Vec::new()
}

fn now_s(app: &WaveApp) -> f64 {
    let sr = f64::from(app.sample_rate().max(1));
    let at = app.player.as_ref().filter(|p| p.is_playing()).map_or(app.selection().start, |p| p.position());
    at as f64 / sr
}

/// The Chords & Lyrics and Chord Diagrams windows, when open.
pub fn windows(ctx: &egui::Context, app: &mut WaveApp) {
    if !app.analysis.lyrics_open && !app.analysis.diagrams_open {
        return;
    }
    refresh(app);
    let t = Tokens::current();
    let resolved = app.analysis.cache.as_ref().map_or(0, |c| c.capo);
    let analysed = app.analysis.cache.is_some();
    let mut run_analysis = false;
    let mut export = false;
    if app.analysis.lyrics_open {
        let mut open = true;
        let highlight = app.analysis.cache.as_ref().map(|c| sung(c, now_s(app))).unwrap_or_default();
        egui::Window::new("Chords & Lyrics")
            .id(egui::Id::new("wave_chords_lyrics"))
            .default_size(vec2(520.0, 420.0))
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(ctx.content_rect().center_top() + vec2(0.0, 80.0))
            .open(&mut open)
            .show(ctx, |ui| {
                if !analysed {
                    ui.label("Nothing analysed yet.");
                    run_analysis = ui.button("Analyze Song").clicked();
                    return;
                }
                controls(ui, &mut app.analysis, resolved);
                ui.separator();
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    if let Some(c) = &app.analysis.cache {
                        for (i, line) in c.txt.iter().enumerate() {
                            let mut text = egui::RichText::new(line).monospace();
                            if highlight.contains(&i) {
                                text = text.color(t.accent).strong();
                            }
                            ui.label(text);
                        }
                    }
                });
                ui.separator();
                export = ui.button("Export…").clicked();
            });
        app.analysis.lyrics_open = open;
    }
    if app.analysis.diagrams_open {
        let mut open = true;
        egui::Window::new("Chord Diagrams")
            .id(egui::Id::new("wave_chord_diagrams"))
            .default_size(vec2(440.0, 300.0))
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(ctx.content_rect().right_top() + vec2(-24.0, 80.0))
            .open(&mut open)
            .show(ctx, |ui| {
                if !analysed {
                    ui.label("Nothing analysed yet.");
                    run_analysis = ui.button("Analyze Song").clicked();
                    return;
                }
                controls(ui, &mut app.analysis, resolved);
                ui.label(if resolved > 0 { format!("Capo {resolved}: shapes to play") } else { "Capo: none".to_string() });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if let Some(c) = &app.analysis.cache {
                            for d in &c.diagrams {
                                diagram(ui, d);
                            }
                        }
                    });
                });
            });
        app.analysis.diagrams_open = open;
    }
    if run_analysis {
        app.run("wave.analyze", json!({}));
    }
    if export {
        let stem = app
            .engine
            .wave
            .as_ref()
            .and_then(|w| w.path.as_deref())
            .and_then(|p| std::path::Path::new(p).file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("song")
            .to_string();
        if let Some(path) = app.services.pick_save.as_ref().and_then(|pick| pick("Export ChordPro", &format!("{stem}.cho"))) {
            let p = export_params(app, &path);
            app.run("wave.chordpro_export", p);
        }
    }
}

/// One chord diagram from a `wave.chord_diagrams` entry: six strings, five frets from the base
/// fret, dots with finger numbers, the barre, and x/o above the nut.
pub fn diagram(ui: &mut egui::Ui, d: &Value) {
    let t = Tokens::current();
    let (rect, _) = ui.allocate_exact_size(vec2(DIAGRAM_W, DIAGRAM_H), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let label = d["label"].as_str().unwrap_or_default();
    painter.text(pos2(rect.center().x, rect.min.y + 2.0), egui::Align2::CENTER_TOP, label, egui::FontId::proportional(13.0), t.text);
    let frets: Vec<i64> = d["frets"].as_array().map(|a| a.iter().map(|v| v.as_i64().unwrap_or(-1)).collect()).unwrap_or_default();
    if frets.len() != 6 {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "—", egui::FontId::proportional(12.0), t.text_dim);
        return;
    }
    let fingers: Vec<i64> = d["fingers"].as_array().map(|a| a.iter().map(|v| v.as_i64().unwrap_or(0)).collect()).unwrap_or_default();
    let base = d["base_fret"].as_i64().unwrap_or(1).max(1);
    let grid = Rect::from_min_max(pos2(rect.min.x + 14.0, rect.min.y + 30.0), pos2(rect.max.x - 8.0, rect.max.y - 6.0));
    let dx = grid.width() / 5.0;
    let dy = grid.height() / 5.0;
    let string_x = |s: usize| grid.min.x + s as f32 * dx;
    let line = Stroke::new(1.0, t.text_dim);
    for s in 0..6 {
        painter.line_segment([pos2(string_x(s), grid.min.y), pos2(string_x(s), grid.max.y)], line);
    }
    for f in 0..=5 {
        let y = grid.min.y + f as f32 * dy;
        painter.line_segment([pos2(grid.min.x, y), pos2(grid.max.x, y)], if f == 0 && base == 1 { Stroke::new(3.0, t.text) } else { line });
    }
    if base > 1 {
        painter.text(
            pos2(grid.min.x - 3.0, grid.min.y + dy / 2.0),
            egui::Align2::RIGHT_CENTER,
            base.to_string(),
            egui::FontId::proportional(9.0),
            t.text_dim,
        );
    }
    let dot = Color32::from_gray(235);
    if let Some(b) = d["barre"].as_i64() {
        let on: Vec<usize> = (0..6).filter(|s| frets[*s] == b).collect();
        if let (Some(&a), Some(&z)) = (on.first(), on.last())
            && z > a
        {
            let y = grid.min.y + ((b - base) as f32 + 0.5) * dy;
            painter.rect_filled(Rect::from_min_max(pos2(string_x(a) - 4.0, y - 4.0), pos2(string_x(z) + 4.0, y + 4.0)), 4.0, dot);
        }
    }
    for (s, f) in frets.iter().enumerate() {
        let x = string_x(s);
        match *f {
            f if f < 0 => {
                painter.text(pos2(x, grid.min.y - 3.0), egui::Align2::CENTER_BOTTOM, "x", egui::FontId::proportional(10.0), t.text_dim);
            }
            0 => {
                painter.circle_stroke(pos2(x, grid.min.y - 7.0), 3.0, Stroke::new(1.0, t.text));
            }
            f => {
                let y = grid.min.y + ((f - base) as f32 + 0.5) * dy;
                painter.circle_filled(pos2(x, y), 5.0, dot);
                if let Some(n) = fingers.get(s).filter(|n| **n > 0) {
                    painter.text(pos2(x, y), egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(8.0), Color32::from_gray(20));
                }
            }
        }
    }
}
