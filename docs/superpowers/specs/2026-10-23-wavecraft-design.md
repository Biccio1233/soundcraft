# WaveCraft — design

> **Date:** 2026-10-23 · **Status:** approved scope (owner: Bix), technical calls by the implementing agent marked *(call)* · **Branch:** `wavecraft`

## Intent

WaveCraft is a single-file audio editor with music analysis, shipped as two new executables (`wavecraft`, `wavecraft-cli`) in this workspace. It reuses SoundCraft's crates instead of forking them. Its owner records song ideas, rehearsals and takes on modest gear: phones, a laptop, sometimes an 8-bit file by mistake. From the terminal or by asking an agent, he wants to:

- clean a take;
- try effects and step back through every change;
- get chords, key, tempo, sections and timed lyrics, and a ChordPro sheet he can study;
- split the parts into stems to practise against.

The songwriting project that consumes the output lives outside this repo. Its corpus scripts read ChordPro files.

The behaviour reference is an incumbent waveform editor for editing and restoration, and an incumbent music-practice app for analysis. Both are only **observed black-box** (menus, dialogs, parameters, screenshots by window id under the gitignored `plan/`). Nothing is read, disassembled or copied from their bundles: icons, presets, texts, models. Our UI uses our own words.

## Goals

- **F0 — single-file editor**
  - Open WAV/AIFF/MP3/M4A/FLAC (8/16/24/32-bit int, 32/64-bit float), with a waveform view and a spectral view (STFT, log frequency scale, our own colour map). Select by time.
  - Destructive edits with a full history: cut/copy/paste/delete, insert silence, fades, clip gain, normalize, invert, reverse. A History panel lists every step and can jump to any of them; the file changes only on Save/Export.
  - A non-destructive effects rack (the same built-in and hosted plugins as SoundCraft). *Apply* bakes it into the audio as one history step; rack presets can be saved.
  - Export with a choice of format and bit depth, and a dither profile: TPDF plus our own noise-shaping curves.
- **F1 — restoration**, parity with the incumbent's noise reduction/restoration family:
  - noise print capture;
  - spectral-subtraction noise reduction (Ephraim–Malah 1985 MMSE-STSA with decision-directed a-priori SNR), with output-noise-only;
  - adaptive noise reduction (minimum statistics, Martin 2001);
  - de-hum (notch comb on 50/60/auto Hz plus harmonics);
  - hiss reduction (band-wise expander above ~4 kHz);
  - de-clip (cubic interpolation of clipped runs);
  - de-reverb (late-decay suppression, declared approximate).

  Each one works both offline and as a rack plugin.
- **F2 — music analysis**, parity with the incumbent practice app:
  - tempo, beats and bars;
  - key (Krumhansl–Kessler profiles);
  - chords (phase A: CQT chroma + templates + Viterbi; phase B: a permissively licensed ONNX model, only if it measurably improves accuracy);
  - stems (HTDemucs, MIT weights, 4 or 6 stems);
  - timed lyrics (Whisper, MIT weights, it/en/auto);
  - chords over lyrics, exported as ChordPro, `.txt` and `.json`.

  **Extended by owner decision (2026-10-23):**
  - song sections with start times (intro/verse/chorus/bridge/instrumental/outro);
  - reference-tuning estimate (A4 in Hz), fed into key and chord analysis;
  - chord difficulty levels with guitar chord diagrams generated from our own voicing rules;
  - tempo change and transposition while listening.
- **F3 — polish**: batch processing of many files from the CLI, markers exported to CSV, recording from the microphone, M4A export (system encoder, isolated FFI crate, macOS only), documented shortcuts, optional Italian UI catalogue.

## Non-goals and anti-goals

- No multitrack: that is SoundCraft. No AAX.
- No MP3 export (the mature encoder is LGPL). No GPL/AGPL/LGPL code anywhere.
- No ML model inside the binary. Models are downloaded on first use, only after the owner says yes, with a pinned SHA-256 and a provenance/licence row each.
- No Python at runtime. The ML runtime is `ort` (ONNX Runtime, optional CoreML execution provider) behind an isolated crate.
- Anti-goal: anything that makes SoundCraft worse. WaveCraft must not change existing command behaviour, the `.scraft` format or SoundCraft's parity numbers. New behaviour lives in new modules and new crates.

## Constraints (repo rules, non-negotiable)

- Never crash: no panics or `unwrap`/`expect` in non-test code. `unsafe` only in isolated FFI crates (the new `wave-ml`, later the M4A encoder), each with `// SAFETY:` comments and a safe `Result` API. Input is hostile: `get()`, checked arithmetic, capped allocations. The audio callback never allocates or blocks.
- Everything is a command (id, label, menu path, params doc, enabled, run, tests), reachable from menus, the CLI, the control channel and MCP.
- `cargo xtask ci` green and `cargo xtask layers` respected on every commit; one topic per commit.
- Do not touch `/Applications/SoundCraft.app` or `~/.local/bin/soundcraft-cli`.

## Architecture *(calls)*

- **Document.**
  - A WaveCraft file is a `soundcraft_model::Session` created at the file's own sample rate (so nothing is resampled), with exactly one audio track (mono or stereo as the file).
  - It is built with `Session::new` + `add_track` + `engine::io::add_source` + `edit::place_clip`.
  - The playlist is a gap-free run of non-overlapping clips over sources (a piece table), so cuts and pastes share audio and undo stays cheap.
  - The edit mode is pinned to Shuffle, so cut/delete close the gap and paste/insert-silence make room.
  - SoundCraft crossfades (which overlap clips) are never used. WaveCraft fades are destructive envelopes over the selection.
- **Range edits.**
  - `audiosuite.process` works on whole clips. So every `wave.*` processing command first separates at the selection edges (`edit.separate`), then processes only the clips inside, then joins them into one new source for that range.
  - Optional per-channel enable: process only L or only R, like a channel-limited selection.
- **Pool hygiene and memory.**
  - The engine never drops superseded sources from the pool.
  - After each `wave.*` step, sources no longer referenced by the current document's clips are removed from *its* pool. Older snapshots keep their own `Arc`s.
  - A history byte budget (default 2 GiB, setting `wave.history_budget_mb`) counts unique source buffers across undo/redo snapshots (deduplicated by `Arc` pointer). It drops the oldest steps with a status message when exceeded.
  - This is additive engine API (`history_bytes`, `trim_history_to`). SoundCraft's behaviour is unchanged.
- **History panel.**
  - The engine's labelled undo/redo snapshots, plus two additive engine methods: `redo_history()` and `history_jump(index)`, the latter as one command, `wave.history_jump`.
  - Labels name the operation and its range ("Cut 0:00.000–0:02.000", "Noise Reduction (−16 dB)").
- **File render and export.**
  - `soundcraft_mix::render_clips` over the whole content gives the exact samples of the file: clip gain, fades and clip effects; no mixer, no pan law, mono stays mono.
  - `wave.export {path, format: wav|aiff|flac, bit_depth: 16|24|32|32f, dither: none|tpdf|shaped_light|shaped_strong}` renders that, applies our dither/noise shaping (new `dsp::dither` module, output snapped to the target grid so the encoder's conversion is exact), and encodes with `audio_io::encode` / `FileEncoder` (streamed, `dither: false` in the encoder).
  - `file.bounce_mix` is **not** used: it renders the main mix, writes mono as stereo and applies the pan law.
  - Save = export to the original path and format, with the original bit depth.
- **Rack.**
  - The track's inserts (10 slots, `mix.insert*` commands). *Apply* = `render_track_pre_fader` over the selection (or the file), then the range is replaced by one new clip and the inserts cleared, in a single undo step.
  - Rack presets are saved as JSON (plugin ids + params) under `~/Library/Application Support/WaveCraft/presets/`.
- **Playback.**
  - `soundcraft_playback::Player` on the one-track session. A mono track is centred with the −3 dB pan law, so WaveCraft sets the track fader to +3.01 dB for mono files: playback matches the file level.
  - Scrub stays silent (as in SoundCraft) for F0.
- **Commands.** `wave.*` ids in new modules under `crates/engine/src/cmd/` (`wave.rs`, `wave_restore.rs`, `wave_analysis.rs`…), each registered by one line in `cmd/mod.rs`. SoundCraft's own commands stay usable where they make sense (selection, transport, plugins).
- **New crates.**
  - `crates/wave-analysis` (`soundcraft-wave-analysis`, L1, pure Rust): tempo, key, chords phase A, sections, tuning, chord voicings and difficulty, lyrics alignment.
  - `crates/wave-ml` (`soundcraft-wave-ml`, the isolated FFI crate around `ort`): HTDemucs and Whisper, model download and verification.
  - `crates/ui-wave` (`soundcraft-ui-wave`, L6): the WaveCraft UI. It reuses `soundcraft-ui-egui` widgets where they are not tied to the multitrack app.
  - New DSP (restoration processors, STFT helpers, noise-shaped dither) goes into `crates/dsp` (L1). `dsp` has `rustfft` (MIT/Apache) and `spectrum::magnitude_db` but no STFT: add `dsp::stft`.
- **UI.**
  - `ui-wave` reuses the `ui-egui` items that are not tied to `SoundApp`: `waveform::draw`, `widgets::{meter, meter_scale, fader, icon_button, …}`, `theme`, `icons`, `fonts`, `edit_window::{x_of, sample_at}`.
  - It writes its own panels: files, effects rack with a generic parameter editor built from `PluginInfo.params`, history, editor (ruler, waveform and spectrogram, selection), levels, selection/view, transport.
  - The spectrogram is computed from `dsp::stft` into cached tiles keyed by document revision.
  - File dialogs come through the same injected `Services` closures (rfd in the app).
- **Apps.**
  - `apps/wavecraft` (egui window, `--control PORT`): its own `control_server` (copy of SoundCraft's small TCP transport) and its own control handler, because SoundCraft's is bound to `SoundApp`.
  - Its own names: config folder `WaveCraft`, `wavecraft.log`, app id `ai.storyteller.wavecraft`, our own icon with an `ATTRIBUTION.md` row.
  - `apps/wavecraft-cli` (`run`, `script`, `commands`, `describe`, `mcp [--connect PORT]`): the same shape and line format as `soundcraft-cli`, help text from its doc comment.
  - Both are added to `EXEMPT` in `xtask`; the new crates go into `LAYERS` (`wave-analysis` L1, `ui-wave` L6, `wave-ml` in the FFI exception list).
- **Agents.**
  - `crates/automation` gets an additive `Profile` (server name, instructions, URI scheme, product name, extra tools). `Server::new` keeps the SoundCraft profile, so its tests stay green; `wavecraft-cli mcp` uses `Server::with_profile(WAVECRAFT)`.
  - The servers are registered in Claude Code as `wavecraft` (headless) and `wavecraft-app` (connected to a running app). The skill folder is `~/.claude/skills/craft/apps/wavecraft/`.
- **Docker.** A Dockerfile for `wavecraft-cli` and `cargo xtask ci` on Linux (Rust ≥ 1.95, `libasound2-dev`, `pkg-config`, git for the `craft-libs` dependency, the `wasm32` target) plus a `docker-compose.yml` with `cli` and `ci` services. The desktop app stays native.
- **Parity docs.** `docs/target-app-parity.md` is already SoundCraft's assessment against its incumbent. WaveCraft's go in `docs/wavecraft/target-app-parity.md` (waveform editor) and `docs/wavecraft/moises-parity.md` (practice app) *(call: avoids overwriting SoundCraft's doc)*.

## Acceptance

- **F0**: a `wavecraft-cli` script imports a synthetic file, cuts 0–2 s, exports 24-bit, undoes two steps and exports again. The second export is bit-identical to the original rendered at 24-bit.
- **F1, synthetic**: 440 Hz at −20 dBFS plus white noise at −70 dBFS. Noise reduction leaves the tone within 1 dB and measurably lowers the noise floor.
- **F1, real**: on the owner's 8-bit take, the pause noise reaches ≤ −70 dB, while the 3 kHz band loses ≤ 1 dB against the source (source −41.6 dB, so not below −42.6 dB). The incumbent reached −74.7 dB in the pause but dropped to −43.3 dB at 3 kHz.
- **F2**: a synthetic song (known progression, tempo and key; TTS voice) gives:
  - BPM ±2 and the right key;
  - chords right on ≥ 85 % of bars;
  - ≥ 90 % of words aligned within 0.5 s;
  - sections detected at the right boundaries on a synthetic form;
  - tuning estimate within ±1 Hz on a detuned render.

  Stem separation on a synthetic 4-stem mix scores vocal SDR ≥ 6 dB. The ChordPro output is parsed by the songwriting corpus reader (`chordpro_corpus.py`). Timings on this Mac (CPU and CoreML) are stated in the README.
- **Delivery**:
  - binaries in `~/.local/bin/wavecraft` and `~/.local/bin/wavecraft-cli`;
  - both MCP servers registered;
  - skill folder with `STATO.md` and recipes (clean from a noise print, stems, chords + lyrics → ChordPro);
  - a Docker image for the headless CLI and CI on Linux.

## Settled after the builder check *(calls unless marked)*

- **F0 acceptance, exact script.** Open the synthetic file; export A (24-bit); cut 0–2 s; normalize; export B; undo twice; export C.
  - Assert C == A byte-for-byte and B ≠ A.
  - Opening shows as the first History row ("Open"), which cannot be undone, as in the incumbent; `replace_session` still clears the engine history.
  - Each `wave.*` command is exactly one undo step, even when it internally separates at the selection edges.
- **Save.**
  - `wave.save` writes back atomically (temp file + rename) only when the source format is writable: WAV/AIFF/FLAC, at the source bit depth, 8-bit excepted.
  - Otherwise it refuses with "use Export" (MP3, M4A, 8-bit).
  - Agent recipes never save over an original take: they always export to a new path. This is a rule in the skill, matching the owner's "originals are untouchable" practice.
- **Chord vocabulary** (from the owner's spec): maj, min, 7, maj7, min7, sus2, sus4, dim, aug, with slash bass.
  - Scoring of "≥ 85 % of bars": the majority label of each bar compared on root + quality, MIREX "sevenths" style, inversion ignored. Bass accuracy is reported separately.
  - Phase B (ONNX) is triggered only if phase A misses 85 %.
- **Noise print in the rack.**
  - Built-in plugins carry only parameters (a test enforces it), and that contract stays.
  - The offline `wave.noise_reduction` uses the full-resolution print stored in the WaveCraft document state.
  - The rack plugin receives the print folded into fixed per-band parameters (1/6-octave bands, ~60 params), which is enough for previewing.
  - Noise prints save and load as JSON files and travel inside rack presets as those band values.
- **`wave-ml` unsafe.** There is no FFI list in `xtask`: unsafe is allowed per crate by its own lints, as in `clap-host`/`vst3-host`/`au-host`. `wave-ml` gets a `LAYERS` entry (L1) and `unsafe_code = "deny"` with a single FFI module.
- **Deferred to the owner at the start of the phase they belong to** (the stop points of the spec):
  - **F1**: the measurement method for the real-take test (band filter for "3 kHz", RMS window, exact pause and music spans), proposed by calibrating against the incumbent's output in `11. AirCraft/SoundCraft/reclusa-nuova1/`, plus whether defaults must already hit the target.
  - **F2**:
    - the first Whisper model to download;
    - the provenance of the ONNX models (HTDemucs has no official ONNX: a one-off export by us at development time, hosted where the owner decides, or a third-party export);
    - free transcription vs alignment of lyrics the owner supplies (recommendation: both, with supplied text preferred for ChordPro);
    - real-time tempo/transpose vs a rendered preview (recommendation: rendered preview, no change to the shared player).

## Limits

- `render_clips` caps at `MAX_RENDER_SAMPLES` (2^29 samples across channels), and `offline` at `MAX_OFFLINE_SAMPLES` (2^28 per channel): about 93 min of stereo at 48 kHz. WaveCraft rejects longer files with a clear error in F0. Chunked processing is F3 work if ever needed.
- 8-bit sources open fine. Export offers 16/24/32-bit int and 32-bit float, because the encoder has no 8-bit writer and nobody needs one.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-23 | major | First design: scope from the owner's spec + extended F2, decisions of the spec review, architecture grounded in the code |
