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

#[test]
fn spectral_view_waits_for_a_drag_to_end() {
    let mut e = opened("spectral_drag");
    e.execute("wave.view_spectral", &json!({"on": true})).unwrap();
    let mut h = harness(e);
    let settled = h.state().spectral_renders();
    assert!(settled >= 1);
    // Drag the splitter (at a quarter of the timeline) down by 120 points over 12 frames.
    let tl = h.state().timeline_rect();
    let (x, y0) = (tl.center().x, tl.min.y + tl.height() * 0.25);
    let press =
        |pressed: bool, y: f32| Event::PointerButton { pos: pos2(x, y), button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
    h.input_mut().events.push(Event::PointerMoved(pos2(x, y0)));
    h.step();
    h.input_mut().events.push(press(true, y0));
    h.step();
    for i in 1..=12 {
        h.input_mut().events.push(Event::PointerMoved(pos2(x, y0 + 10.0 * i as f32)));
        h.step();
    }
    assert_eq!(h.state().spectral_renders(), settled, "no recompute while the drag lasts");
    h.input_mut().events.push(press(false, y0 + 120.0));
    h.run();
    assert_eq!(h.state().spectral_renders(), settled + 1, "one recompute once it ends");
}

fn other_file(test: &str) -> String {
    let dir = std::env::temp_dir().join("wavecraft-ui-tests").join(test);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("other.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    std::fs::write(&path, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 4_800]] }, &opts).unwrap())
        .unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn opening_another_file_asks_before_discarding_edits() {
    let frames = |app: &mut WaveApp| app.engine.execute("wave.info", &json!({})).unwrap()["frames"].as_u64();
    for (answer, expected) in [(false, Some(96_000)), (true, Some(4_800))] {
        let mut e = opened(&format!("ask_discard_{answer}"));
        e.execute("wave.gain", &json!({"db": 1.0})).unwrap();
        let mut app = WaveApp::new(e, None, Services::default()).with_confirm(Box::new(move |_question: &str| answer));
        app.open_path(&other_file(&format!("ask_discard_{answer}")));
        assert_eq!(frames(&mut app), expected, "answer {answer}");
    }
    // With nobody to ask, the open is refused and the edits stay.
    let mut e = opened("ask_discard_none");
    e.execute("wave.gain", &json!({"db": 1.0})).unwrap();
    let mut app = WaveApp::new(e, None, Services::default());
    app.open_path(&other_file("ask_discard_none"));
    assert_eq!(frames(&mut app), Some(96_000));
    assert!(app.engine.is_dirty());
}

#[test]
fn export_options_offer_depth_and_dither() {
    let dir = std::env::temp_dir().join("wavecraft-ui-tests").join("export_options");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.wav");
    let mut h = harness(opened("export_options_src"));
    h.state_mut().begin_export(out.to_str().unwrap());
    h.run();
    h.get_by_label("16-bit").click();
    h.run();
    h.get_by_label("Shaped strong").click();
    h.run();
    h.get_by_label("Export file").click();
    h.run();
    let info = soundcraft_audio_io::probe(&std::fs::read(&out).unwrap(), Some("wav")).unwrap();
    assert_eq!(info.sample_format, soundcraft_audio_io::SampleFormat::Int16);
    assert!(h.query_by_label("Export file").is_none(), "the options window closes");
}

#[test]
fn channel_toggles_set_the_enabled_channels() {
    let dir = std::env::temp_dir().join("wavecraft-ui-tests").join("channel_toggles");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stereo.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    std::fs::write(&path, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 4_800]; 2] }, &opts).unwrap())
        .unwrap();
    let mut e = Engine::default();
    e.execute("wave.open", &json!({"path": path.to_str().unwrap()})).unwrap();
    let mut h = harness(e);
    h.get_by_label("R").click();
    h.run();
    assert_eq!(exec(&mut h, "wave.info", json!({}))["channels_enabled"], json!([true, false]));
}

#[test]
fn effect_items_name_existing_plugins() {
    use soundcraft_ui_wave::menus::Dialog;
    let mut n = 0;
    for item in WAVE_MENUS {
        if let Dialog::Effect(plugin) = item.dialog {
            assert!(soundcraft_dsp::plugin_info(plugin).is_some(), "{} → {plugin}", item.label);
            n += 1;
        }
    }
    assert_eq!(n, 6);
}

#[test]
fn effect_dialog_process_adds_one_history_row() {
    let mut h = harness(opened("effect_dialog"));
    let n0 = exec(&mut h, "wave.history", json!({}))["rows"].as_array().unwrap().len();
    h.state_mut().open_effect("dehum", "wave.dehum");
    h.run();
    h.get_by_label("Process").click();
    h.run();
    let rows = exec(&mut h, "wave.history", json!({}))["rows"].as_array().cloned().unwrap();
    assert_eq!(rows.len(), n0 + 1);
    assert!(h.state().engine.undo_label().unwrap_or_default().starts_with("Hum Removal"), "{:?}", h.state().engine.undo_label());
}

#[test]
fn noise_reduction_dialog_captures_the_print() {
    let mut e = opened("nr_dialog");
    e.execute("wave.select", &json!({"start": 0, "end": 24_000})).unwrap();
    let mut h = harness(e);
    h.state_mut().open_effect("noise_reduction", "wave.noise_reduction");
    h.run();
    h.get_by_label("Capture Noise Print").click();
    h.run();
    assert_eq!(exec(&mut h, "wave.noise_print_info", json!({}))["captured"], json!(true));
}

#[test]
fn rack_hides_band_params() {
    let mut e = opened("rack_bands");
    e.execute("wave.noise_print_capture", &json!({"start": 0, "end": 24_000})).unwrap();
    e.execute("wave.rack_add", &json!({"plugin": "noise_reduction"})).unwrap();
    let h = harness(e);
    assert!(h.query_by_label("nb00").is_none());
    assert!(h.query_by_label_contains("Noise print").is_some());
}

#[test]
fn amplitude_statistics_window_shows_rms() {
    let mut h = harness(opened("stats_window"));
    h.state_mut().open_stats();
    h.run();
    assert!(h.query_by_label_contains("RMS").is_some());
}

/// An engine with a 45 s synthetic song open: intro, verse, chorus, verse, chorus (A minor, 96 BPM).
fn song_opened(test: &str) -> Engine {
    use soundcraft_wave_analysis::chords::{Chord, Quality};
    use soundcraft_wave_analysis::sections::SectionLabel;
    use soundcraft_wave_analysis::testsignal::{SongSpec, acceptance_song, song};
    let c = |root, quality| Chord { root, quality, bass: None };
    let (am, f, cm, g) = (c(9, Quality::Min), c(5, Quality::Maj), c(0, Quality::Maj), c(7, Quality::Maj));
    let spec = SongSpec {
        sections: vec![
            (SectionLabel::Intro, vec![am, am]),
            (SectionLabel::Verse, vec![am, f, cm, g]),
            (SectionLabel::Chorus, vec![f, g, cm, am]),
            (SectionLabel::Verse, vec![am, f, cm, g]),
            (SectionLabel::Chorus, vec![f, g, cm, am]),
        ],
        ..acceptance_song()
    };
    let (channels, _) = song(&spec);
    let dir = std::env::temp_dir().join("wavecraft-ui-tests").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("song.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    std::fs::write(&path, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: spec.sample_rate, channels }, &opts).unwrap()).unwrap();
    let mut e = Engine::default();
    e.execute("wave.open", &json!({"path": path.to_str().unwrap()})).unwrap();
    e
}

fn menu(h: &mut Harness<'static, WaveApp>, label: &str) {
    let item = WAVE_MENUS.iter().find(|m| m.label == label).unwrap_or_else(|| panic!("no menu item {label}"));
    soundcraft_ui_wave::menus::invoke(h.state_mut(), item);
    h.run();
}

#[test]
fn analysis_menu_items_reach_their_commands() {
    let ids: Vec<&str> = soundcraft_engine::command_specs().iter().map(|c| c.id).collect();
    let analysis: Vec<_> = WAVE_MENUS.iter().filter(|m| m.path.first() == Some(&"Analysis")).collect();
    for label in ["Analyze Song", "Chords & Lyrics…", "Chord Diagrams…", "Load Lyrics Text…", "Export ChordPro…", "Show Analysis Lanes"] {
        let m = analysis.iter().find(|m| m.label == label).unwrap_or_else(|| panic!("Analysis ▸ {label} missing"));
        assert!(ids.contains(&m.command), "{label}: {} is not a command", m.command);
    }
}

#[test]
fn analyze_song_shows_lanes() {
    let mut h = harness(song_opened("analysis_lanes"));
    assert!(h.state().analysis_lanes().is_none());
    menu(&mut h, "Analyze Song");
    let lanes = h.state().analysis_lanes().expect("lanes after analysis");
    assert!(lanes.chords.iter().any(|c| c.2 == "Am"), "{:?}", lanes.chords);
    assert!(lanes.sections.iter().any(|s| s.2 == "chorus"), "{:?}", lanes.sections);
    assert!(lanes.bars.len() >= 16 && lanes.beats.len() >= 64);
    menu(&mut h, "Show Analysis Lanes");
    assert!(h.state().analysis_lanes().is_none());
}

#[test]
fn chords_and_lyrics_window_shows_text() {
    let mut e = song_opened("analysis_lyrics_window");
    let words = json!([{"word": "chiusa", "start": 5.1, "end": 5.6}, {"word": "vive", "start": 5.7, "end": 6.2}, {"word": "dentro", "start": 7.6, "end": 8.0}]);
    e.execute("wave.lyrics_set", &json!({"words": words})).unwrap();
    let mut h = harness(e);
    menu(&mut h, "Chords & Lyrics…");
    assert!(h.query_all_by_label_contains("chiusa").next().is_some(), "no lyric line");
    assert!(h.query_all_by_label_contains("Am").next().is_some(), "no chord line");
    menu(&mut h, "Chord Diagrams…");
    assert!(h.query_all_by_label_contains("Capo").count() >= 2, "both windows show the capo");
}

#[test]
fn diagram_window_draws_every_quality() {
    use soundcraft_wave_analysis::chords::{Chord, QUALITIES};
    for q in QUALITIES {
        let chord = Chord { root: 4, quality: q, bass: None };
        let v = soundcraft_wave_analysis::voicing::voicings(&chord).into_iter().next().expect("a voicing");
        let d = json!({"label": chord.label(None), "frets": v.frets, "fingers": v.fingers, "base_fret": v.base_fret, "barre": v.barre});
        let mut h = Harness::builder().with_size(vec2(200.0, 200.0)).build_ui(move |ui| soundcraft_ui_wave::analysis::diagram(ui, &d));
        h.run();
    }
}
