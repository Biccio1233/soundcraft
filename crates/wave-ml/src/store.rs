//! The models folder and checked downloads: a model is written to `<file>.part`, counted and hashed
//! as it arrives, and renamed into place only when its size and sha256 match the registry. Before a
//! model reaches a native runtime its sha256 is checked again (once per process while the file is
//! unchanged): a damaged file is moved aside so it can be downloaded again.

use sha2::{Digest, Sha256};
use soundcraft_wave_analysis::models::ModelInfo;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

const CHUNK: usize = 1 << 16;

/// How long a download may wait for the server, or stall mid-file, before it fails.
pub const STALL: Duration = Duration::from_secs(60);

/// Files whose sha256 matched in this process: path, length, modification time.
static CHECKED: Mutex<Vec<(PathBuf, u64, SystemTime)>> = Mutex::new(Vec::new());

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

fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let md = std::fs::metadata(path).ok()?;
    Some((md.len(), md.modified().ok()?))
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(buf.get(..n).unwrap_or(&[]));
    }
}

fn damaged_path(dir: &Path, m: &ModelInfo) -> PathBuf {
    dir.join(format!("{}.damaged", m.file))
}

/// The model's file, when installed and its sha256 matches the registry. A damaged file is moved
/// aside to `<file>.damaged`, so the model shows as not installed and can be downloaded again.
pub fn verified(dir: &Path, m: &ModelInfo) -> Result<PathBuf, String> {
    let path = installed(dir, m).ok_or_else(|| crate::missing(m))?;
    let st = stamp(&path);
    let seen = |c: &[(PathBuf, u64, SystemTime)]| st.is_some_and(|(len, t)| c.iter().any(|(p, l, tt)| *p == path && *l == len && *tt == t));
    if seen(&CHECKED.lock().unwrap_or_else(PoisonError::into_inner)) {
        return Ok(path);
    }
    if sha256_of(&path)? == m.sha256 {
        if let Some((len, t)) = st {
            CHECKED.lock().unwrap_or_else(PoisonError::into_inner).push((path.clone(), len, t));
        }
        return Ok(path);
    }
    let aside = damaged_path(dir, m);
    let moved = std::fs::rename(&path, &aside).is_ok();
    Err(format!(
        "model {} ({} MB) is damaged (its sha256 does not match){}: run wave.model_download {{\"id\": \"{}\"}} to download it again",
        m.id,
        (m.bytes as f64 / 1e6).round(),
        if moved { format!(" and was moved aside to {}", aside.display()) } else { String::new() },
        m.id
    ))
}

/// The model's checked file: the installed one when its sha256 matches, else a fresh download.
pub fn ensure(dir: &Path, m: &ModelInfo, url: &str, stall: Duration, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<PathBuf, String> {
    if let Ok(path) = verified(dir, m) {
        return Ok(path);
    }
    let path = download(dir, m, url, stall, progress)?;
    let _ = std::fs::remove_file(damaged_path(dir, m));
    Ok(path)
}

/// Download `m` from `url` into `dir`; `progress(done, total)` in bytes returns false to cancel.
/// The server may take up to `stall` to answer or to send the next bytes. On any failure the
/// partial file is removed and nothing is installed.
pub fn download(dir: &Path, m: &ModelInfo, url: &str, stall: Duration, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dir.join(format!("{}.part", m.file));
    let result = fetch(&part, m, url, stall, progress);
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

fn fetch(part: &Path, m: &ModelInfo, url: &str, stall: Duration, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), String> {
    let config = ureq::Agent::config_builder().timeout_connect(Some(stall)).timeout_recv_response(Some(stall)).timeout_per_read(Some(stall)).build();
    let agent = ureq::Agent::new_with_config(config);
    let response = agent.get(url).call().map_err(|e| format!("{}: download failed: {e}", m.id))?;
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
        if !progress(done, m.bytes) {
            return Err(format!("{}: download cancelled", m.id));
        }
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
