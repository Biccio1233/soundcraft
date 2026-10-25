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
| Music analysis (never undo steps) | `wave.analyze`, `wave.analyze_tuning`, `wave.analyze_tempo`, `wave.analyze_key`, `wave.analyze_chords`, `wave.analyze_sections`, `wave.analysis_info`, `wave.chord_diagrams`, `wave.view_analysis` |
| Lyrics and ChordPro | `wave.lyrics_set`, `wave.lyrics`, `wave.chords_over_lyrics`, `wave.chordpro_export` |

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
| Hiss Reduction | `wave.hiss_reduction` | A downward expander per FFT bin above the cutoff (4 kHz), against a tracked noise floor. Steady tones above the cutoff count as hiss. |
| Clip Repair | `wave.declip` | Runs of clipped samples (up to 10 ms) redrawn by a cubic through their neighbours. Redrawn peaks can pass 0 dBFS: the command says so, and the audio needs normalizing before a 16- or 24-bit save or export, which would flatten them again. |
| Reverb Reduction | `wave.dereverb` | Late reverberation predicted from the power 50 ms earlier and the reverb time you set, subtracted per bin. Approximate: it does not measure the room. |

The usual workflow: select a stretch of noise alone, `wave.noise_print_capture` (it needs a
selection or explicit `start`/`end`, and keeps one print per channel), select all (or nothing),
`wave.noise_reduction`. Prints save and load as JSON (`wave.noise_print_save/load`).

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

## Music analysis (F2a)

Everything here is pure Rust (`crates/wave-analysis`), with no model to download. The audio is
mixed to mono and resampled to 22,050 Hz once. `wave.analyze` runs the whole chain on the
selection (or the whole file) and keeps the result until the audio changes: selecting, zooming or
moving the playhead keeps it, an edit makes it stale (`wave.analysis_info`) and an undo makes it
fresh again. The other commands reuse it as it is, with its range and grid, unless they name
`start`/`end` or `grid`.

| Analysis | How it works |
|---|---|
| Tuning (`wave.analyze_tuning`) | Spectral peaks between 80 Hz and 2 kHz against the equal-tempered grid: the reference A4 in Hz, which then centres the chroma. |
| Tempo, beats, bars (`wave.analyze_tempo`) | No tempo when nothing has an onset (a held note, a drone). Otherwise onsets from log-spectral flux over 40 bands, an autocorrelation tempo with a preference around 120 BPM, and beats by dynamic programming (Ellis 2007). An octave check doubles a slow estimate whose half-beats are as strong as a snare. Bars start where the chords change and the bass hits; 4/4 unless `beats_per_bar`. |
| Key (`wave.analyze_key`) | Krumhansl–Kessler profiles. When the relative major/minor scores almost the same, the tonic chord the song dwells on decides. |
| Chords (`wave.analyze_chords {grid: beat\|bar\|free, level: 1..3, capo: 0..7}`) | Constant-Q chroma against harmonic templates of maj, m, 7, maj7, m7, sus2, sus4, dim and aug in all keys, smoothed by Viterbi; the bass decides inversions (`C/E`). Where no chord fits well (drums alone, speech, noise) there is no chord, never a guessed one. Level 1 = major and minor only, 2 = adds sevenths and sus, 3 = everything. With a capo the labels are the shapes to play. |
| Sections (`wave.analyze_sections`) | Bar-by-bar self-similarity of harmony and timbre; boundaries from checkerboard novelty; repeats share a letter; names (intro, verse, chorus, bridge, outro) from repetition and position, as a best effort. |
| Guitar diagrams (`wave.chord_diagrams {level, capo: "auto"\|0..7}`) | Our own search over fretboard shapes in standard tuning, scored for fingers, barre, stretch, position and open strings. `auto` suggests a capo only when it makes the song clearly easier. |

Lyrics: `wave.lyrics_set {words: [{word, start, end}]}` sets timed words (F2b's transcription will
fill them). Then `wave.lyrics_set {text}` or `{path}` aligns the real lyrics to them word by word,
misheard words included; pasted tags like `[Chorus]` are dropped and every time stays inside the
file. `wave.chords_over_lyrics` puts the chords on the words (inside a held word when the chord
changes on it); where nobody sings, it writes chord-only lines of four bars. `wave.chordpro_export` writes ChordPro (with
`{define}` diagrams, `{soc}`/`{eoc}` choruses), plain text with chords above the words, or JSON.

In the app: Analysis › Analyze Song draws beat, chord and section lanes under the ruler.
Chords & Lyrics… shows the song with the line under the playhead highlighted, Chord Diagrams…
shows the shapes, and both have Level and Capo pickers.

**Synthetic acceptance** (A minor, 96 BPM, A4 = 436 Hz, 8 sections over 52 bars):

| Target | Measured |
|---|---|
| Tuning ± 1 Hz | 435.93 Hz |
| Tempo ± 2 BPM | 96.00 |
| Key | A minor |
| Chords right on ≥ 85 % of bars | 100 % |
| Section boundaries within a beat | 8 of 8 |
| ChordPro read by the songwriting corpus reader | 44 of 44 chords, same order |

**Two real covers, report only** (no ground truth for these versions; the corpus gives the
original songs' chords). One cover of a song on G B C Cm reads G major, 90.9 BPM, and at level 1
96 % of its time on those four chords. One on A Bm D reads A major, 137.7 BPM, with 92 % of its time
on A, Bm and D. Section boundaries on real recordings are much less reliable than on the
synthetic song.

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
- F2a acceptance: `cargo test -p wavecraft-cli --test acceptance_f2` (the synthetic song above).
  The ChordPro check against the corpus reader is ignored by default:
  `cargo test -p wavecraft-cli --test acceptance_f2 -- --ignored` (`WAVECRAFT_CORPUS_READER=path`).

## Timings on the development Mac (Apple silicon, release build)

- Normalize + fade + 24-bit export of a 217 s stereo 44.1 kHz take: 0.6 s.
- Open + noise print + noise reduction (FFT 4096) of the same take: 1.7 s.
- Open + `wave.analyze` of a 4.5-minute MP3: about 5 s on a loaded machine (load average 13–20); the analysis alone about 3 s.
- Release build of both binaries from a warm cache: 1 min 40 s (21 min after the F2a engine changes, with the machine loaded).

## Limits

Mono and stereo files only, up to 2^28 frames per channel (about 93 minutes of stereo at 48 kHz).
Restoration has no learned sound removal, click/pop or phase tools yet (see the parity doc). Music
analysis has no stems, transcription or tempo/transpose preview yet (F2b); no M4A export or batch
(F3).
