# WaveCraft parity — music analysis (F2)

> **Last updated:** 2026-10-25 · **Change:** minor (F2b: stems, transcription, practice preview) · **Target:** the incumbent practice app for musicians, observed black-box (its screens and menus, kept under the gitignored `plan/`)

What the incumbent offers to study a song, and where WaveCraft stands. Wording is ours; only
feature names are shared. F2a covers everything that needs no machine-learning model; F2b adds
stems, transcription and the tempo/transpose preview, with models downloaded only on request.

| Feature | WaveCraft | Command(s) | Test |
|---|---|---|---|
| Tempo (BPM) | ✅ onset autocorrelation + dynamic-programming beats, octave check; 60–196 BPM exact on a plain drum pattern | `wave.analyze_tempo` | `finds_tempo_of_a_drum_pattern`, `tempi_away_from_the_prior` |
| Beats and bars on the timeline | ✅ beat ticks, bar lines and numbers in a lane under the ruler; 4/4 unless `beats_per_bar` | `wave.analyze_tempo`, `wave.view_analysis` | `downbeats_follow_chord_changes`, `analyze_song_shows_lanes` |
| Key | ✅ Krumhansl–Kessler profiles, relative major/minor settled by the tonic chord | `wave.analyze_key` | `a_minor_progression`, `settle_key_prefers_the_tonic_chord` |
| Tuning reference (A4) | ✅ estimated, and used to centre the chroma | `wave.analyze_tuning` | `tuning_finds_a_detuned_reference` |
| Chords on a beat, bar or free grid | ✅ constant-Q chroma, harmonic templates, Viterbi; maj, m, 7, maj7, m7, sus2, sus4, dim, aug, inversions | `wave.analyze_chords` | `each_quality_is_recognised`, `slash_bass`, `progression_bars` |
| Simplified chords (difficulty levels) | ✅ three levels: triads, + sevenths and sus, everything | `wave.analyze_chords {level}` | `levels_simplify`, `chords_levels_and_capo` |
| Guitar chord diagrams | ✅ our own voicing search (standard tuning), easiest first | `wave.chord_diagrams` | `open_chords_first`, `every_chord_has_a_voicing`, `diagram_window_draws_every_quality` |
| Capo | ✅ shapes for any capo, or a suggested one when it saves real effort | `wave.analyze_chords {capo}`, `wave.chord_diagrams {capo: "auto"}` | `capo_suggestion` |
| Song sections (verse, chorus…) | ✅ boundaries from self-similarity; names from repetition and position (best effort) | `wave.analyze_sections` | `acceptance_form_boundaries`, `chorus_and_verse_named` |
| Lyrics synced to the music | ✅ transcribed with word times (from the vocals stem when there is one); supplied text is aligned to them | `wave.transcribe`, `wave.lyrics_set`, `wave.lyrics` | `transcribe_prefers_the_vocals_stem_and_sets_words`, `models_meet_the_f2_targets` |
| Chords over lyrics, export | ✅ ChordPro (with chord definitions), plain text, JSON | `wave.chords_over_lyrics`, `wave.chordpro_export` | `chordpro_places_chords_on_words`, `chordpro_is_read_by_the_corpus_reader` |
| Highlighted line while playing | ✅ in the Chords & Lyrics window | UI | `chords_and_lyrics_window_shows_text` |
| Stem separation (vocals, drums, bass, other, guitar, piano) | ✅ HTDemucs 4 or 6 stems, WAV files beside the song | `wave.separate_stems`, `wave.stems` | `separate_writes_stems_beside_the_file`, `models_meet_the_f2_targets` |
| Lyrics transcription | ✅ whisper.cpp, Italian, English or detected | `wave.transcribe` | `whisper_hears_italian`, `models_meet_the_f2_targets` |
| Tempo change and transpose while listening | 🟡 a rendered preview (re-rendered on each change), not live | `wave.practice`, `wave.practice_off`, `wave.practice_export` | `practice_preview_on_the_synthetic_song`, `practice_maps_positions` |
| Mute or solo instruments while practising | ✅ a gain per stem in the practice render | `wave.practice {stems}` | `practice_mixes_stems_with_gains` |
| Model management | ✅ list, sizes, licences, checked downloads | `wave.models`, `wave.model_download` | `download_checks_hash_and_size`, `models_window_lists_sizes` |
| Metronome, count-in, loop by section | ❌ not planned in F2 | — | — |

## Acceptance (synthetic songs)

A minor, 96 BPM, A4 = 436 Hz, 48 kHz, eight sections over 52 bars (`testsignal::acceptance_song`),
through the CLI: A4 435.93 Hz, 96.00 BPM, A minor, chords right on 100 % of the bars, all 8 section
boundaries on the bar. The exported ChordPro is read back by the songwriting corpus reader with the
same 44 chords in the same order.

With the models (a synthetic 4-stem song, 41 s: drums, bass, pad and an Italian TTS voice with its
words placed at known times): vocal SDR 15.15 dB (target ≥ 6; drums 19.1, bass 15.2, other 11.6),
27 of 27 words transcribed from the vocals stem, in order, starting within 0.5 s (target ≥ 90 %).
The practice render of the acceptance song at 80 % and +2 semitones lasts 1.2500 × as long and
reads 76.80 BPM and B minor.
