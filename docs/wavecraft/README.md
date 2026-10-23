# WaveCraft

> **Last updated:** 2026-10-24 · **Stage:** F1 (restoration) · **Design:** [`../superpowers/specs/2026-10-23-wavecraft-design.md`](../superpowers/specs/2026-10-23-wavecraft-design.md)

WaveCraft is a single-file audio editor built on SoundCraft's crates: open one file, edit it
destructively with a full history, preview an effects rack and bake it in, look at the waveform and
the spectrogram, export with dither, and clean it up: noise reduction from a noise print, adaptive
noise reduction, hum, hiss, clipping and reverb. Music analysis (F2) comes next. Everything is a
`wave.*` command, reachable from the menus, `wavecraft-cli`, the control channel and MCP.

## Build and run

```sh
cargo build --release -p wavecraft -p wavecraft-cli
target/release/wavecraft take.wav                 # the window
target/release/wavecraft take.wav --control 0     # plus the control channel (prints WAVECRAFT_CONTROL_PORT=…)
target/release/wavecraft-cli run --in take.wav --cmd wave.info
target/release/wavecraft-cli script edit.script --in take.wav   # one `ID {json}` per line, `#` comments
target/release/wavecraft-cli commands wave.        # WaveCraft's commands with their params
```

A script, for example (cut the first two seconds, normalize, export 24-bit):

```text
wave.cut {"start": 0, "end": {"seconds": 2}}
wave.normalize {"db": -1}
wave.export {"path": "take-edit.wav", "bit_depth": 24, "dither": "shaped_light"}
```

Positions are samples, `{"seconds": x}` or time strings like `"0:12.500"`. Edits act on the
selection (`wave.select {start, end, channels?}`) or on explicit `start`/`end`. Processing commands
(gain, normalize, fades, Apply Rack…) with no selection act on the whole file; cut, copy and delete
need a range, so a cursor alone never removes the audio.

`wave.open` refuses when the open file has unsaved changes unless it gets `discard: true` (the app
asks first), and it never replaces a SoundCraft multitrack session.

## Agents (MCP)

- Headless: `wavecraft-cli mcp` — the same generic tools as SoundCraft (`list_commands`, `execute`,
  `batch`, …) under the server name `wavecraft`.
- A running app: start it with `--control PORT`, then `wavecraft-cli mcp --connect PORT`. The app's
  control channel answers SoundCraft's methods and shapes (`engine.execute`, `session.inspect`,
  `ui.inspect`, `ui.click`, `ui.drag`, `ui.key`, `ui.text`, `ui.screenshot`, `app.quit`; see
  [`../control-protocol.md`](../control-protocol.md)).
- Register in Claude Code: `claude mcp add --scope user wavecraft -- wavecraft-cli mcp` and
  `claude mcp add --scope user wavecraft-app -- wavecraft-cli mcp --connect PORT`.

## Commands

| Area | Commands |
|---|---|
| File | `wave.open`, `wave.info`, `wave.save`, `wave.export` |
| Selection and view | `wave.select`, `wave.view_spectral` |
| Edits (one undo step each) | `wave.cut`, `wave.copy`, `wave.paste`, `wave.delete`, `wave.insert_silence`, `wave.silence`, `wave.fade_in`, `wave.fade_out`, `wave.gain`, `wave.normalize`, `wave.invert`, `wave.reverse` |
| History | `wave.history`, `wave.history_jump`, `wave.history_budget`, `edit.undo`, `edit.redo` |
| Effects rack | `wave.rack_add`, `wave.rack_remove`, `wave.rack_bypass`, `wave.rack_param`, `wave.rack_list`, `wave.rack_apply`, `wave.rack_preset_save`, `wave.rack_preset_load` |
| Noise print | `wave.noise_print_capture`, `wave.noise_print_info`, `wave.noise_print_save`, `wave.noise_print_load`, `wave.noise_print_clear` |
| Restoration (one undo step each) | `wave.noise_reduction`, `wave.adaptive_noise_reduction`, `wave.dehum`, `wave.hiss_reduction`, `wave.declip`, `wave.dereverb` |
| Measurement | `wave.stats {start?, end?, filter?: bandpass\|highpass\|lowpass, hz?, q?}` (peak and RMS) |

Export writes WAV, AIFF or FLAC at 16, 24 or 32 bits or 32-bit float (in the app, File › Export…
asks for the file, then for bit depth and dither). Dither defaults to TPDF at
16 bits and none above; `shaped_light` and `shaped_strong` move the noise to the highs. Samples
already on the target grid pass through unchanged, so untouched audio stays bit-exact. Save writes
back only WAV/AIFF/FLAC at 16 bits or more; MP3, M4A and 8-bit files go through Export.

## Restoration (F1)

Each restoration effect is a built-in plugin, so it works three ways: as a `wave.*` command that
processes the selection (or the whole file) as one undo step, as a dialog in Effects › Noise
Reduction / Restoration (*Process*, or *Add to Rack* to hear it live), and as a rack effect.

| Effect | Command | How it works |
|---|---|---|
| Noise Reduction | `wave.noise_reduction` | Removes the captured noise print: Ephraim–Malah (1985) log-spectral amplitude gain with the decision-directed a-priori SNR. Defaults: reduce by up to 40 dB, noise offset +2 dB, smoothing 99.5 %, FFT 4096. In the rack the print travels as 62 sixth-octave band levels. |
| Adaptive Denoise | `wave.adaptive_noise_reduction` | No print: the noise is tracked by minimum statistics (after Martin 2001). Steady tones count as noise; a rising noise is followed within about two tracking windows. |
| Hum Removal | `wave.dehum` | Finite-depth notches on 50 or 60 Hz and harmonics (Auto picks the family from the selection). |
| Hiss Reduction | `wave.hiss_reduction` | A downward expander per FFT bin above the cutoff (4 kHz), against a tracked noise floor. |
| Clip Repair | `wave.declip` | Runs of clipped samples (up to 10 ms) redrawn by a cubic through their neighbours. |
| Reverb Reduction | `wave.dereverb` | Late reverberation predicted from the power 50 ms earlier and the reverb time you set, subtracted per bin. Approximate: it does not measure the room. |

The usual workflow: select a stretch of noise alone, `wave.noise_print_capture`, select all (or
nothing), `wave.noise_reduction`. Prints save and load as JSON (`wave.noise_print_save/load`).

**Real-take acceptance** (the owner's 8-bit 44.1 kHz stereo take; method in the design, "Settled at
the start of F1": pause = RMS over 211.4–212.1 s, "3 kHz" = RMS over 30–90 s after an RBJ
band-pass at 3000 Hz, Q 0.5). Capture on the pause, defaults on the whole file:

| | Pause | 3 kHz | > 8 kHz on the music (30–60 s) |
|---|---|---|---|
| Source | −44.96 dB | −41.52 dB | −55.2 dB |
| WaveCraft | **−71.20 dB** (limit −70) | **−42.34 dB** (−0.83, limit −1) | −65.8 dB |
| Incumbent, 22/10 | −74.7 dB | −43.3 dB (−1.7) | −64.7 dB |

ffmpeg on WaveCraft's 24-bit export reads the same pause and 3 kHz figures to 0.01 dB. The 3 kHz
margin is small (0.17 dB) on this take: with brighter noise, lower the reduction or the offset.

## Quality gates

- `docker compose run --rm ci` — the full `cargo xtask ci` on Linux (software Vulkan for the UI tests).
- `scripts/wavecraft-ci-macos.sh` — the same steps on macOS, skipping the six tests that scan the
  system VST3 folder (a third-party plug-in that throws while loading aborts the test process).
- F0 acceptance: `cargo test -p wavecraft-cli --test acceptance_f0` (open, export, cut 0–2 s,
  normalize, export, undo twice, export: the third export equals the first byte for byte).
- F1 acceptance: `cargo test -p wavecraft-cli --test acceptance_f1` (synthetic: 440 Hz at −20 dBFS
  over −70 dBFS noise keeps the tone within 1 dB and lowers the floor ≥ 20 dB; measured +0.00 dB and
  −28.8 dB). The real take: `cargo test --release -p wavecraft-cli --test acceptance_f1 -- --ignored`
  (`WAVECRAFT_REAL_TAKE=path` if it is elsewhere).

## Timings on the development Mac (Apple silicon, release build)

- Normalize + fade + 24-bit export of a 217 s stereo 44.1 kHz take: 0.6 s.
- Open + noise print + noise reduction (FFT 4096) of the same take: 1.7 s.
- Release build of both binaries from a warm cache: 1 min 40 s.

## Limits

Mono and stereo files only, up to 2^28 frames per channel (about 93 minutes of stereo at 48 kHz).
Restoration has no learned sound removal, click/pop or phase tools yet (see the parity doc). No
music analysis yet (F2), no M4A export or batch (F3).
