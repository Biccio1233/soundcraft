//! WaveCraft's effects rack: the track's inserts, previewed non-destructively and baked into the
//! file by `wave.rack_apply`.

use super::wave::{has_file, wave, wave_range, wave_track};
use super::wave_edit::replace_range;
use super::*;
use crate::{cmd, io};
use serde_json::json;

/// Version of the rack preset format.
const PRESET_VERSION: u64 = 1;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "wave.rack_add",
            "Add Effect",
            [],
            None,
            "{plugin, params?: {id: value}, slot?: 0..9} — put an effect in the rack (first free slot by default); the rack previews until applied",
            has_file,
            rack_add
        ),
        cmd!("wave.rack_remove", "Remove Effect", [], None, "{slot}", has_file, rack_remove),
        cmd!("wave.rack_bypass", "Bypass Effect", [], None, "{slot, on: bool}", has_file, rack_bypass),
        cmd!("wave.rack_param", "Set Effect Parameter", [], None, "{slot, param, value} — clamped to the parameter's range", has_file, rack_param),
        cmd!(
            query "wave.rack_list",
            "Effects Rack",
            [],
            None,
            "{} — the rack: slot, plugin, name, bypass, and each parameter's id, name, value, min, max, default, unit",
            has_file,
            rack_list
        ),
        cmd!(
            "wave.rack_apply",
            "Apply Rack",
            [],
            None,
            "{start?, end?} — render the selection (or the whole file) through the rack into the audio and empty the rack, as one step",
            has_file,
            rack_apply
        ),
        cmd!(query "wave.rack_preset_save", "Save Rack Preset", [], None, "{path} — write the rack as a JSON preset", has_file, preset_save),
        cmd!(
            "wave.rack_preset_load",
            "Load Rack Preset",
            [],
            None,
            "{path} — replace the rack with a JSON preset (an unknown plugin refuses the whole preset; values are clamped)",
            has_file,
            preset_load
        ),
    ]
}

/// Run another command's body inside this one, so it joins this command's single undo step.
fn call(e: &mut Engine, id: &str, p: Value) -> Result<Value> {
    let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
    (spec.run)(e, &p)
}

/// `p` with the wave track added as `track`.
fn on_track(e: &Engine, p: &Value) -> Result<Value> {
    let t = wave_track(e)?;
    let mut obj = p.as_object().cloned().unwrap_or_default();
    obj.insert("track".into(), json!(t.0));
    Ok(Value::Object(obj))
}

fn rack_json(e: &Engine) -> Result<Value> {
    let t = wave_track(e)?;
    let inserts = e.session().track(t).map(|tr| tr.mixer.inserts.clone()).unwrap_or_default();
    let slots: Vec<Value> = inserts
        .iter()
        .enumerate()
        .filter_map(|(slot, ins)| ins.as_ref().map(|ins| (slot, ins)))
        .map(|(slot, ins)| {
            let info = super::clap::plugin_info(&ins.plugin);
            let params: Vec<Value> = match info {
                Some(info) => info
                    .params
                    .iter()
                    .map(|pi| {
                        json!({
                            "id": pi.id,
                            "name": pi.name,
                            "value": ins.params.get(pi.id).copied().unwrap_or(pi.default),
                            "min": pi.min,
                            "max": pi.max,
                            "default": pi.default,
                            "unit": format!("{:?}", pi.unit).to_ascii_lowercase(),
                        })
                    })
                    .collect(),
                None => ins.params.iter().map(|(id, v)| json!({"id": id, "name": id, "value": v})).collect(),
            };
            json!({"slot": slot, "plugin": ins.plugin, "name": info.map_or(ins.plugin.as_str(), |i| i.name), "bypass": ins.bypass, "params": params})
        })
        .collect();
    Ok(json!({"slots": slots}))
}

fn rack_add(e: &mut Engine, p: &Value) -> Result<Value> {
    let plugin = str_param(p, "plugin").ok_or_else(|| bad("wave.rack_add", "`plugin` required"))?.to_string();
    let q = on_track(e, p)?;
    call(e, "mix.insert", q)?;
    let name = super::clap::plugin_info(&plugin).map_or(plugin.as_str(), |i| i.name).to_string();
    e.set_undo_label(format!("Add {name}"));
    rack_json(e)
}

fn rack_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let q = on_track(e, p)?;
    call(e, "mix.insert_remove", q)?;
    e.set_undo_label("Remove Effect");
    rack_json(e)
}

fn rack_bypass(e: &mut Engine, p: &Value) -> Result<Value> {
    let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("wave.rack_bypass", "`on` must be true or false"))?;
    let mut q = on_track(e, p)?;
    if let Some(obj) = q.as_object_mut() {
        obj.insert("value".into(), json!(on));
    }
    call(e, "mix.insert_bypass", q)?;
    e.set_undo_label(if on { "Bypass Effect" } else { "Enable Effect" });
    rack_json(e)
}

fn rack_param(e: &mut Engine, p: &Value) -> Result<Value> {
    let q = on_track(e, p)?;
    call(e, "mix.insert_param", q)?;
    let param = str_param(p, "param").unwrap_or("parameter").to_string();
    e.set_undo_label(format!("Set {param}"));
    rack_json(e)
}

fn rack_list(e: &mut Engine, _: &Value) -> Result<Value> {
    rack_json(e)
}

fn rack_apply(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.rack_apply";
    let t = wave_track(e)?;
    let channels = wave(e)?.channels;
    let r = wave_range(e, ID, p)?;
    if r.is_empty() {
        return Err(bad(ID, "select a range first"));
    }
    let inserts = e.session().track(t).map(|tr| tr.mixer.inserts.clone()).unwrap_or_default();
    let active = inserts.iter().flatten().filter(|i| !i.bypass && i.active).count();
    if active == 0 {
        return Err(bad(ID, "the rack has no active effect"));
    }
    let len = usize::try_from(r.len()).unwrap_or(0);
    let sample_rate = wave(e)?.sample_rate as f32;
    let built_in = inserts.iter().flatten().all(|i| soundcraft_dsp::plugin_info(&i.plugin).is_some());
    let mut out = if built_in {
        // Offline, in slot order, each plugin prepared and reset with its parameters set: no mixer
        // (so no pan law) and no parameter smoothing at the start of the range.
        let mut chans = soundcraft_mix::render_clips(e.session(), t, r);
        for ins in inserts.iter().flatten().filter(|i| !i.bypass && i.active) {
            let mut plugin = soundcraft_dsp::create(&ins.plugin).ok_or_else(|| bad(ID, format!("cannot create `{}`", ins.plugin)))?;
            for (k, v) in &ins.params {
                plugin.set_param(k, *v);
            }
            soundcraft_dsp::offline::apply_plugin(&mut chans, plugin.as_mut(), sample_rate);
        }
        chans
    } else {
        // Hosted plugins only run inside the mix engine. A mono track renders into a stereo main
        // (both sides equal, the pan law undone): keep one side.
        soundcraft_mix::render_track_pre_fader(e.session(), t, r)
    };
    out.truncate(channels);
    if out.len() != channels {
        return Err(EngineError::Internal(ID.into(), format!("the rack rendered {} channels for a {channels}-channel file", out.len())));
    }
    for ch in &mut out {
        ch.resize(len, 0.0);
    }
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.mixer.inserts.iter_mut().for_each(|i| *i = None);
    }
    let label = format!("Apply Rack ({active} effect{})", if active == 1 { "" } else { "s" });
    replace_range(e, r, out, label)
}

fn preset_save(e: &mut Engine, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad("wave.rack_preset_save", "`path` required"))?.to_string();
    let t = wave_track(e)?;
    let inserts = e.session().track(t).map(|tr| tr.mixer.inserts.clone()).unwrap_or_default();
    let slots: Vec<Value> = inserts
        .iter()
        .enumerate()
        .filter_map(|(slot, ins)| ins.as_ref().map(|ins| json!({"slot": slot, "plugin": ins.plugin, "bypass": ins.bypass, "params": ins.params})))
        .collect();
    let n = slots.len();
    let text = serde_json::to_string_pretty(&json!({"version": PRESET_VERSION, "slots": slots}))
        .map_err(|err| EngineError::Internal("wave.rack_preset_save".into(), err.to_string()))?;
    io::write_file(&path, text.as_bytes())?;
    Ok(json!({"path": path, "slots": n}))
}

fn preset_load(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.rack_preset_load";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(ID, "`path` required"))?.to_string();
    let t = wave_track(e)?;
    let text = io::read_text(&path)?;
    let preset: Value = serde_json::from_str(&text).map_err(|err| bad(ID, format!("{path} is not a rack preset: {err}")))?;
    if preset.get("version").and_then(Value::as_u64) != Some(PRESET_VERSION) {
        return Err(bad(ID, format!("{path}: unknown preset version (expected {PRESET_VERSION})")));
    }
    let slots = preset.get("slots").and_then(Value::as_array).ok_or_else(|| bad(ID, format!("{path}: `slots` must be a list")))?;
    // Check every slot before touching the rack.
    for s in slots {
        let plugin = s.get("plugin").and_then(Value::as_str).unwrap_or_default();
        if super::clap::plugin_info(plugin).is_none() {
            return Err(bad(ID, format!("{path}: unknown plugin `{plugin}`")));
        }
    }
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.mixer.inserts.iter_mut().for_each(|i| *i = None);
    }
    for s in slots {
        let mut q = json!({"track": t.0, "plugin": s.get("plugin").cloned().unwrap_or(Value::Null)});
        if let Some(obj) = q.as_object_mut() {
            for key in ["slot", "params"] {
                if let Some(v) = s.get(key) {
                    obj.insert(key.into(), v.clone());
                }
            }
        }
        let added = call(e, "mix.insert", q)?;
        if s.get("bypass").and_then(Value::as_bool) == Some(true) {
            call(e, "mix.insert_bypass", json!({"track": t.0, "slot": added.get("slot").cloned().unwrap_or(Value::Null), "value": true}))?;
        }
    }
    e.set_undo_label("Load Rack Preset");
    rack_json(e)
}

#[cfg(test)]
mod tests {
    use super::super::wave::render_file;
    use super::super::wave::tests::{noise, opened};
    use super::*;
    use serde_json::json;
    use soundcraft_audio_io::BitDepth;

    fn run(e: &mut Engine, id: &str, p: Value) -> Value {
        e.execute(id, &p).unwrap_or_else(|err| panic!("{id}: {err}"))
    }

    /// Built-in effects are applied offline with their parameters snapped: exact from sample 0.
    const SETTLE: usize = 0;

    fn doubled(now: &[f32], orig: &[f32]) -> bool {
        now.iter().zip(orig).all(|(a, b)| (a - 2.0 * b).abs() <= 1e-4)
    }

    #[test]
    fn apply_bakes_the_rack_and_clears_it() {
        let (mut e, _) = opened("rack_apply", &noise(48_000, 1, 48_000), BitDepth::Float32);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.rack_add", json!({"plugin": "gain", "params": {"gain": 6.0206}}));
        assert_eq!(render_file(&e).unwrap(), orig, "the rack is a preview until applied");
        run(&mut e, "wave.rack_apply", json!({}));
        let now = render_file(&e).unwrap();
        assert!(doubled(&now[0][SETTLE..], &orig[0][SETTLE..]));
        assert!(run(&mut e, "wave.rack_list", json!({}))["slots"].as_array().unwrap().is_empty());
        assert_eq!(e.undo_label(), Some("Apply Rack (1 effect)"));
        e.undo();
        assert_eq!(render_file(&e).unwrap(), orig);
        assert_eq!(run(&mut e, "wave.rack_list", json!({}))["slots"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn apply_on_a_range_only() {
        let (mut e, _) = opened("rack_range", &noise(48_000, 2, 96_000), BitDepth::Float32);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.rack_add", json!({"plugin": "gain", "params": {"gain": 6.0206}}));
        run(&mut e, "wave.rack_apply", json!({"start": 24_000, "end": 72_000}));
        let now = render_file(&e).unwrap();
        for c in 0..2 {
            assert_eq!(now[c][..24_000], orig[c][..24_000]);
            assert_eq!(now[c][72_000..], orig[c][72_000..]);
            assert!(doubled(&now[c][24_000 + SETTLE..72_000], &orig[c][24_000 + SETTLE..72_000]), "channel {c}");
        }
    }

    #[test]
    fn list_shows_plugins_and_param_ranges() {
        let (mut e, _) = opened("rack_list", &noise(48_000, 1, 4_800), BitDepth::Float32);
        run(&mut e, "wave.rack_add", json!({"plugin": "gain", "params": {"gain": 3.0}, "slot": 2}));
        run(&mut e, "wave.rack_bypass", json!({"slot": 2, "on": true}));
        let list = run(&mut e, "wave.rack_list", json!({}));
        let slot = &list["slots"][0];
        assert_eq!((slot["slot"].as_u64(), slot["plugin"].as_str(), slot["bypass"].as_bool()), (Some(2), Some("gain"), Some(true)));
        let p = &slot["params"][0];
        assert_eq!((p["id"].as_str(), p["value"].as_f64(), p["max"].as_f64()), (Some("gain"), Some(3.0), Some(24.0)));
        run(&mut e, "wave.rack_param", json!({"slot": 2, "param": "gain", "value": 30.0}));
        assert_eq!(run(&mut e, "wave.rack_list", json!({}))["slots"][0]["params"][0]["value"].as_f64(), Some(24.0));
        run(&mut e, "wave.rack_remove", json!({"slot": 2}));
        assert!(run(&mut e, "wave.rack_list", json!({}))["slots"].as_array().unwrap().is_empty());
    }

    #[test]
    fn preset_round_trip() {
        let (mut e, dir) = opened("rack_preset", &noise(48_000, 1, 4_800), BitDepth::Float32);
        run(&mut e, "wave.rack_add", json!({"plugin": "gain", "params": {"gain": -3.0}}));
        run(&mut e, "wave.rack_add", json!({"plugin": "dc_offset_removal", "slot": 3}));
        let before = run(&mut e, "wave.rack_list", json!({}));
        let preset = dir.join("rack.json");
        run(&mut e, "wave.rack_preset_save", json!({"path": preset.to_str().unwrap()}));
        run(&mut e, "wave.rack_remove", json!({"slot": 0}));
        run(&mut e, "wave.rack_remove", json!({"slot": 3}));
        let n = e.undo_history().len();
        run(&mut e, "wave.rack_preset_load", json!({"path": preset.to_str().unwrap()}));
        assert_eq!(run(&mut e, "wave.rack_list", json!({})), before);
        assert_eq!(e.undo_history().len(), n + 1);
    }

    #[test]
    fn preset_rejects_unknown_plugins_and_clamps() {
        let (mut e, dir) = opened("rack_preset_bad", &noise(48_000, 1, 4_800), BitDepth::Float32);
        run(&mut e, "wave.rack_add", json!({"plugin": "gain"}));
        let before = run(&mut e, "wave.rack_list", json!({}));
        let bad = dir.join("bad.json");
        std::fs::write(&bad, r#"{"version":1,"slots":[{"slot":0,"plugin":"gain","params":{}},{"slot":1,"plugin":"no_such_plugin","params":{}}]}"#)
            .unwrap();
        assert!(e.execute("wave.rack_preset_load", &json!({"path": bad.to_str().unwrap()})).is_err());
        assert_eq!(run(&mut e, "wave.rack_list", json!({})), before);
        let loud = dir.join("loud.json");
        std::fs::write(&loud, r#"{"version":1,"slots":[{"slot":0,"plugin":"gain","bypass":false,"params":{"gain":99.0}}]}"#).unwrap();
        run(&mut e, "wave.rack_preset_load", json!({"path": loud.to_str().unwrap()}));
        assert_eq!(run(&mut e, "wave.rack_list", json!({}))["slots"][0]["params"][0]["value"].as_f64(), Some(24.0));
        std::fs::write(&loud, "not json").unwrap();
        assert!(e.execute("wave.rack_preset_load", &json!({"path": loud.to_str().unwrap()})).is_err());
    }

    #[test]
    fn export_ignores_the_rack_until_applied() {
        let (mut e, dir) = opened("rack_export", &noise(48_000, 1, 4_800), BitDepth::Float32);
        let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
        run(&mut e, "wave.export", json!({"path": a.to_str().unwrap()}));
        run(&mut e, "wave.rack_add", json!({"plugin": "gain", "params": {"gain": 12.0}}));
        run(&mut e, "wave.export", json!({"path": b.to_str().unwrap()}));
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    }
}
