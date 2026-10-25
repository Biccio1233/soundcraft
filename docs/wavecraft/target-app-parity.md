# WaveCraft parity — waveform editor (F0)

> **Last updated:** 2026-10-24 · **Change:** minor (restoration section, F1) · **Target:** the incumbent waveform editor, observed black-box (menus, dialogs, screenshots by window id, kept under the gitignored `plan/`)

What the incumbent's waveform view offers for one file, and where WaveCraft stands. Wording is ours;
only feature names are shared. Music analysis (F2) has its own document: `practice-app-parity.md`.

| Feature | WaveCraft | Command(s) | Test |
|---|---|---|---|
| Open common audio formats, 8–32 bit | ✅ WAV/AIFF/FLAC/MP3/M4A/Ogg, mono and stereo | `wave.open` | `open_keeps_rate_channels_and_samples` |
| Waveform and spectral display (log frequency) | ✅ | `wave.view_spectral` | `spectral_toggle_is_a_view_command`, `tile_for_a_440_tone_peaks_in_the_right_row` |
| Time selection, per-channel editing | ✅ | `wave.select {channels}` | `channel_mask_limits_processing` |
| Cut, copy, paste, delete, insert silence | ✅ | `wave.cut/copy/paste/delete/insert_silence` | `cut_closes_the_gap_and_is_one_step`, `paste_inserts_and_shifts` |
| Fades with shapes, gain, normalize, invert, reverse, silence | ✅ | `wave.fade_in/fade_out/gain/normalize/invert/reverse/silence` | `fades_follow_their_shape`, `gain_and_normalize_touch_only_the_range` |
| History list with jump to any state, undone steps shown | ✅ plus a memory budget | `wave.history`, `wave.history_jump`, `wave.history_budget` | `history_rows_mark_current_and_undone`, `history_rows_click_jumps` |
| Undo/redo labels naming the operation | ✅ e.g. "Cut 0:00.000–0:02.000" | `edit.undo`, `edit.redo` | `cut_closes_the_gap_and_is_one_step` |
| Non-destructive effects rack (10 slots), Apply into the audio | ✅ built-in and hosted plugins | `wave.rack_*`, `wave.rack_apply` | `apply_bakes_the_rack_and_clears_it`, `apply_on_a_range_only` |
| Rack presets | ✅ JSON | `wave.rack_preset_save/load` | `preset_round_trip` |
| Save / Save As with format, sample type and format settings | ✅ WAV/AIFF/FLAC; 🟡 no MP3/AAC writer | `wave.save`, `wave.export` | `export_formats_follow_the_extension`, `save_refuses_mp3_m4a_and_8_bit` |
| Dither on bit-depth reduction (types, noise shaping) | ✅ TPDF + two noise-shaping curves of our own | `wave.export {dither}` | `shaped_strong_pushes_error_up`, `on_grid_input_passes_through` |
| Sample-rate and channel conversion on save | ❌ not yet (export keeps rate and channels) | — | — |
| Levels meter, selection/view fields | ✅ | UI | `drag_selects_a_range` |
| Markers, batch processing, recording | ❌ F3 | — | — |

## Restoration (F1)

The incumbent's Effects › Noise Reduction / Restoration menu, item by item (our names and wording).

| Feature | WaveCraft | Command(s) | Test |
|---|---|---|---|
| Capture a noise print, save/load it | ✅ JSON prints | `wave.noise_print_capture/save/load/clear/info` | `print_save_load_round_trip`, `capture_then_noise_reduction_is_one_undo_step` |
| Noise reduction from the print (amount, spectral smoothing, FFT size, output noise only) | ✅ MMSE log-spectral amplitude; print curve in the dialog, not editable | `wave.noise_reduction` | `nr_synthetic_tone_kept_noise_lowered`, `real_take_meets_the_f1_target` |
| Adaptive noise reduction (no print) | ✅ minimum statistics | `wave.adaptive_noise_reduction` | `adaptive_lowers_stationary_noise_without_print` |
| Simple denoise with an amount slider | 🟡 covered by Adaptive Denoise | `wave.adaptive_noise_reduction` | — |
| Hum removal (frequency, Q, harmonics, output hum only) | ✅ 50/60/Auto | `wave.dehum` | `dehum_removes_50hz_family`, `dehum_auto_finds_60` |
| Hiss reduction (noise floor, reduce by, output hiss only) | ✅ cutoff adjustable | `wave.hiss_reduction` | `hiss_lowers_highs_keeps_lows` |
| Reverb removal (amount, auto gain) | ✅ approximate (needs the reverb time) | `wave.dereverb` | `dereverb_shortens_the_tail` |
| Clipping restoration | ✅ cubic redraw of runs up to 10 ms | `wave.declip` | `declip_restores_a_clipped_sine` |
| Preview while adjusting | ✅ through the rack (Add to Rack) | `wave.rack_add` | `effect_dialog_process_adds_one_history_row` |
| Amplitude statistics | 🟡 peak and RMS, optional band filter; no histogram, no loudness | `wave.stats` | `stats_matches_hand_computation` |
| Learned sound removal (capture a sound model, remove it) | ❌ | — | — |
| Click/pop eliminator, automatic click removal | ❌ | — | — |
| Automatic phase correction | ❌ | — | — |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-23 | major | First assessment at the end of F0 |
| 2026-10-24 | minor | Restoration section at the end of F1 |
