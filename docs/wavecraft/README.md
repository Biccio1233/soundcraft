# WaveCraft

> **Last updated:** 2026-10-23 · **Stage:** F0 (single-file editor) · **Design:** [`../superpowers/specs/2026-10-23-wavecraft-design.md`](../superpowers/specs/2026-10-23-wavecraft-design.md)

WaveCraft is a single-file audio editor built on SoundCraft's crates: open one file, edit it
destructively with a full history, preview an effects rack and bake it in, look at the waveform and
the spectrogram, export with dither. Restoration (F1) and music analysis (F2) come next. Everything
is a `wave.*` command, reachable from the menus, `wavecraft-cli`, the control channel and MCP.

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

## Commands (F0)

| Area | Commands |
|---|---|
| File | `wave.open`, `wave.info`, `wave.save`, `wave.export` |
| Selection and view | `wave.select`, `wave.view_spectral` |
| Edits (one undo step each) | `wave.cut`, `wave.copy`, `wave.paste`, `wave.delete`, `wave.insert_silence`, `wave.silence`, `wave.fade_in`, `wave.fade_out`, `wave.gain`, `wave.normalize`, `wave.invert`, `wave.reverse` |
| History | `wave.history`, `wave.history_jump`, `wave.history_budget`, `edit.undo`, `edit.redo` |
| Effects rack | `wave.rack_add`, `wave.rack_remove`, `wave.rack_bypass`, `wave.rack_param`, `wave.rack_list`, `wave.rack_apply`, `wave.rack_preset_save`, `wave.rack_preset_load` |

Export writes WAV, AIFF or FLAC at 16, 24 or 32 bits or 32-bit float (in the app, File › Export…
asks for the file, then for bit depth and dither). Dither defaults to TPDF at
16 bits and none above; `shaped_light` and `shaped_strong` move the noise to the highs. Samples
already on the target grid pass through unchanged, so untouched audio stays bit-exact. Save writes
back only WAV/AIFF/FLAC at 16 bits or more; MP3, M4A and 8-bit files go through Export.

## Quality gates

- `docker compose run --rm ci` — the full `cargo xtask ci` on Linux (software Vulkan for the UI tests).
- `scripts/wavecraft-ci-macos.sh` — the same steps on macOS, skipping the six tests that scan the
  system VST3 folder (a third-party plug-in that throws while loading aborts the test process).
- F0 acceptance: `cargo test -p wavecraft-cli --test acceptance_f0` (open, export, cut 0–2 s,
  normalize, export, undo twice, export: the third export equals the first byte for byte).

## Timings on the development Mac (Apple silicon, release build)

- Normalize + fade + 24-bit export of a 217 s stereo 44.1 kHz take: 0.6 s.
- Release build of both binaries from a warm cache: 1 min 40 s.

## Limits (F0)

Mono and stereo files only, up to 2^28 frames per channel (about 93 minutes of stereo at 48 kHz).
No noise reduction or restoration yet (F1), no music analysis (F2), no M4A export or batch (F3).
