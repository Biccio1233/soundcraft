//! F0 acceptance: a script opens a file, exports it at 24 bits, cuts 0–2 s, normalizes, exports
//! again, undoes both steps and exports a third time. The third export is the first, byte for
//! byte; the second differs.

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use std::process::Command;

fn noise(channels: usize, frames: usize) -> AudioBuffer {
    let mut s = 0x1234_5678u32;
    let ch = (0..channels)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
                })
                .collect()
        })
        .collect();
    AudioBuffer { sample_rate: 48_000, channels: ch }
}

#[test]
fn script_cut_normalize_undo_reexport_is_identical() {
    let dir = std::env::temp_dir().join("wavecraft-cli-tests").join("acceptance_f0");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("take.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None };
    std::fs::write(&src, soundcraft_audio_io::encode(&noise(2, 144_000), &opts).unwrap()).unwrap();
    let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
    let script = format!(
        "# F0 acceptance\n\
         wave.open {{\"path\": \"{src}\"}}\n\
         wave.export {{\"path\": \"{a}\", \"bit_depth\": 24}}\n\
         wave.cut {{\"start\": 0, \"end\": {{\"seconds\": 2}}}}\n\
         wave.normalize {{}}\n\
         wave.export {{\"path\": \"{b}\", \"bit_depth\": 24}}\n\
         edit.undo {{}}\n\
         edit.undo {{}}\n\
         wave.export {{\"path\": \"{c}\", \"bit_depth\": 24}}\n",
        src = src.to_str().unwrap(),
        a = p("a.wav"),
        b = p("b.wav"),
        c = p("c.wav"),
    );
    let script_path = dir.join("f0.wavecraft");
    std::fs::write(&script_path, script).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft-cli")).arg("script").arg(&script_path).output().unwrap();
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let read = |n: &str| std::fs::read(dir.join(n)).unwrap();
    assert_eq!(read("c.wav"), read("a.wav"));
    assert_ne!(read("b.wav"), read("a.wav"));
}

#[test]
fn run_with_in_opens_the_file_first() {
    let dir = std::env::temp_dir().join("wavecraft-cli-tests").join("run_in");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("take.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    std::fs::write(&src, soundcraft_audio_io::encode(&noise(1, 4_800), &opts).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft-cli")).args(["run", "--in", src.to_str().unwrap(), "--cmd", "wave.info"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"frames\":4800"), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn help_names_the_tool_and_never_opens_a_window() {
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft-cli")).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().next().unwrap_or_default().starts_with("wavecraft-cli run"), "{text}");
    assert!(text.contains("wavecraft-cli mcp"), "{text}");
}
