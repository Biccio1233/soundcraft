# WaveCraft parity — music analysis (F2)

> **Last updated:** 2026-10-25 · **Change:** major (first version, F2a) · **Target:** the incumbent practice app for musicians, observed black-box (its screens and menus, kept under the gitignored `plan/`)

What the incumbent offers to study a song, and where WaveCraft stands. Wording is ours; only
feature names are shared. F2a covers everything that needs no machine-learning model; stems,
transcription and the tempo/transpose preview come with F2b.

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
| Lyrics synced to the music | 🟡 timed words come from F2b's transcription or from an agent; supplied text is aligned to them | `wave.lyrics_set`, `wave.lyrics` | `align_transfers_times_and_fills_gaps`, `lyrics_words_then_text` |
| Chords over lyrics, export | ✅ ChordPro (with chord definitions), plain text, JSON | `wave.chords_over_lyrics`, `wave.chordpro_export` | `chordpro_places_chords_on_words`, `chordpro_is_read_by_the_corpus_reader` |
| Highlighted line while playing | ✅ in the Chords & Lyrics window | UI | `chords_and_lyrics_window_shows_text` |
| Stem separation (vocals, drums, bass, other, guitar, piano) | ❌ F2b | — | — |
| Lyrics transcription | ❌ F2b | — | — |
| Tempo change and transpose while listening | ❌ F2b (rendered preview) | — | — |
| Metronome, count-in, loop by section | ❌ not planned in F2 | — | — |

## Acceptance (synthetic song)

A minor, 96 BPM, A4 = 436 Hz, 48 kHz, eight sections over 52 bars (`testsignal::acceptance_song`),
through the CLI: A4 435.93 Hz, 96.00 BPM, A minor, chords right on 100 % of the bars, all 8 section
boundaries on the bar. The exported ChordPro is read back by the songwriting corpus reader with the
same 44 chords in the same order.
