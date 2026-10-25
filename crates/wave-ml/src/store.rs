//! The models folder and checked downloads: a model is written to `<file>.part`, counted and hashed
//! as it arrives, and renamed into place only when its size and sha256 match the registry.

use sha2::{Digest, Sha256};
use soundcraft_wave_analysis::models::ModelInfo;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const CHUNK: usize = 1 << 16;

/// Where models live: `WAVECRAFT_MODELS`, else `~/Library/Application Support/WaveCraft/models` on
/// macOS and `$XDG_DATA_HOME/WaveCraft/models` (or `~/.local/share/WaveCraft/models`) elsewhere.
pub fn models_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("WAVECRAFT_MODELS").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support/WaveCraft/models");
    }
    match std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("WaveCraft/models"),
        None => home.join(".local/share/WaveCraft/models"),
    }
}

/// The model's file in `dir`, when present with the registry's size.
pub fn installed(dir: &Path, m: &ModelInfo) -> Option<PathBuf> {
    let path = dir.join(m.file);
    let len = std::fs::metadata(&path).ok()?.len();
    (len == m.bytes).then_some(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download `m` from `url` into `dir`; `progress(done, total)` in bytes. On any failure the partial
/// file is removed and nothing is installed.
pub fn download(dir: &Path, m: &ModelInfo, url: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dir.join(format!("{}.part", m.file));
    let result = fetch(&part, m, url, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
        return result.map(|_| part);
    }
    let path = dir.join(m.file);
    std::fs::rename(&part, &path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("{}: {e}", path.display())
    })?;
    Ok(path)
}

fn fetch(part: &Path, m: &ModelInfo, url: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    let response = ureq::get(url).call().map_err(|e| format!("{}: download failed: {e}", m.id))?;
    let mut body = response.into_body().into_reader();
    let mut file = std::fs::File::create(part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("{}: download interrupted: {e}", m.id))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or(&[]);
        done = done.saturating_add(n as u64);
        if done > m.bytes {
            return Err(format!("{}: wrong size: more than the {} bytes expected", m.id, m.bytes));
        }
        hasher.update(chunk);
        file.write_all(chunk).map_err(|e| format!("{}: {e}", part.display()))?;
        progress(done, m.bytes);
    }
    file.flush().map_err(|e| format!("{}: {e}", part.display()))?;
    if done != m.bytes {
        return Err(format!("{}: wrong size: {done} of {} bytes", m.id, m.bytes));
    }
    let got = hex(&hasher.finalize());
    if got != m.sha256 {
        return Err(format!("{}: checksum mismatch (sha256 {got}, expected {})", m.id, m.sha256));
    }
    Ok(())
}
