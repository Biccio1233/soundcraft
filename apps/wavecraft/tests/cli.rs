//! The wavecraft binary's command line: help and version never open a window.

use std::process::Command;

#[test]
fn help_does_not_open_a_window() {
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft")).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().next().unwrap_or_default().starts_with("wavecraft [FILE]"), "{text}");
}

#[test]
fn version_prints_and_exits() {
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft")).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("WaveCraft "));
}
