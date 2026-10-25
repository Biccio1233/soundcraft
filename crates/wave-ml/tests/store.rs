use soundcraft_wave_analysis::models::{ModelInfo, ModelKind};
use soundcraft_wave_ml::store;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("wavecraft-ml-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Serve `body` (or a 404 for any path ending in "missing") to every request; returns the base URL.
fn serve(body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            if req.lines().next().is_some_and(|l| l.contains("missing")) {
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            } else {
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                let _ = s.write_all(&body);
            }
        }
    });
    format!("http://{addr}")
}

fn info(bytes: u64, sha256: &'static str) -> ModelInfo {
    ModelInfo { id: "test", file: "test.bin", url: "", sha256, bytes, licence: "MIT", source: "test", kind: ModelKind::Transcription }
}

/// A well-formed sha256 that matches nothing served here.
const WRONG_SHA: &str = "0e5f6a6d3b6ce8e2f27e1e1b1d6c4f1e21a0b5e4a87fd03e0a2b3f6f8f8f0c1d";

fn sha_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn download_checks_hash_and_size() {
    let body = b"hello model".to_vec();
    let sha: &'static str = Box::leak(sha_of(&body).into_boxed_str());
    let url = serve(body.clone());
    let d = dir("download_ok");
    let m = info(body.len() as u64, sha);
    let mut seen = 0;
    let path = store::download(&d, &m, &format!("{url}/test.bin"), store::STALL, &mut |done, total| {
        seen = done;
        assert_eq!(total, body.len() as u64);
        true
    })
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert_eq!(seen, body.len() as u64);
    assert_eq!(store::installed(&d, &m), Some(path));

    let d = dir("download_bad_hash");
    let bad = info(body.len() as u64, WRONG_SHA);
    let err = store::download(&d, &bad, &format!("{url}/test.bin"), store::STALL, &mut |_, _| true).unwrap_err();
    assert!(err.contains("checksum"), "{err}");
    assert!(std::fs::read_dir(&d).unwrap().next().is_none(), "left files behind");

    let d = dir("download_too_big");
    let small = info(5, sha);
    let err = store::download(&d, &small, &format!("{url}/test.bin"), store::STALL, &mut |_, _| true).unwrap_err();
    assert!(err.contains("size"), "{err}");
    assert!(std::fs::read_dir(&d).unwrap().next().is_none(), "left files behind");

    let d = dir("download_404");
    assert!(store::download(&d, &m, &format!("{url}/missing"), store::STALL, &mut |_, _| true).is_err());
    assert!(store::installed(&d, &m).is_none());
}

#[test]
fn installed_needs_the_right_size() {
    let d = dir("installed_size");
    let m = info(100, WRONG_SHA);
    std::fs::write(d.join("test.bin"), vec![0u8; 50]).unwrap();
    assert!(store::installed(&d, &m).is_none());
    std::fs::write(d.join("test.bin"), vec![0u8; 100]).unwrap();
    assert!(store::installed(&d, &m).is_some());
}

/// Same length as "the real model", other bytes.
const FAKE: &[u8] = b"the fake model";

fn touch_later(path: &std::path::Path) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10)).unwrap();
}

#[test]
fn damaged_model_is_refused_and_moved_aside() {
    let body = b"the real model".to_vec();
    let sha: &'static str = Box::leak(sha_of(&body).into_boxed_str());
    let d = dir("damaged");
    let m = info(body.len() as u64, sha);
    std::fs::write(d.join("test.bin"), FAKE).unwrap();
    assert!(store::installed(&d, &m).is_some(), "the right size: listed as installed");
    let err = store::verified(&d, &m).unwrap_err();
    assert!(err.contains("damaged") && err.contains("wave.model_download"), "{err}");
    assert!(store::installed(&d, &m).is_none(), "moved aside, so the model can be downloaded again");
    assert_eq!(std::fs::read(d.join("test.bin.damaged")).unwrap(), FAKE);

    std::fs::write(d.join("test.bin"), &body).unwrap();
    assert_eq!(store::verified(&d, &m).unwrap(), d.join("test.bin"));
    // A change after a good check is caught (the check is cached by length and modification time).
    std::fs::write(d.join("test.bin"), FAKE).unwrap();
    touch_later(&d.join("test.bin"));
    assert!(store::verified(&d, &m).is_err());
}

#[test]
fn ensure_downloads_again_over_a_damaged_file() {
    let body = b"the real model".to_vec();
    let sha: &'static str = Box::leak(sha_of(&body).into_boxed_str());
    let url = serve(body.clone());
    let d = dir("ensure_damaged");
    let m = info(body.len() as u64, sha);
    std::fs::write(d.join("test.bin"), FAKE).unwrap();
    let path = store::ensure(&d, &m, &format!("{url}/test.bin"), store::STALL, &mut |_, _| true).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert!(!d.join("test.bin.damaged").exists(), "the damaged copy is dropped once the good one is in place");
    // Good and in place: no second fetch (a dead URL would fail).
    assert_eq!(store::ensure(&d, &m, "http://127.0.0.1:9/test.bin", store::STALL, &mut |_, _| true).unwrap(), path);
}

/// A server that answers with a header and 10 of 100 bytes (`head`), or not at all, then hangs.
fn serve_stalling(head: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            if head {
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n0123456789");
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
    format!("http://{addr}")
}

#[test]
fn stalled_downloads_fail_instead_of_hanging() {
    let m = info(100, WRONG_SHA);
    for head in [false, true] {
        let d = dir(&format!("stall_{head}"));
        let t0 = std::time::Instant::now();
        let r = store::download(&d, &m, &format!("{}/test.bin", serve_stalling(head)), std::time::Duration::from_millis(300), &mut |_, _| true);
        assert!(r.is_err(), "head {head}: {r:?}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "head {head}: waited {:?}", t0.elapsed());
        assert!(std::fs::read_dir(&d).unwrap().next().is_none(), "left files behind");
    }
}

#[test]
fn a_download_can_be_cancelled() {
    let body = vec![7u8; 300_000];
    let sha: &'static str = Box::leak(sha_of(&body).into_boxed_str());
    let url = serve(body.clone());
    let d = dir("cancel");
    let m = info(body.len() as u64, sha);
    let err = store::download(&d, &m, &format!("{url}/test.bin"), store::STALL, &mut |_, _| false).unwrap_err();
    assert!(err.contains("cancelled"), "{err}");
    assert!(std::fs::read_dir(&d).unwrap().next().is_none(), "left files behind");
}
