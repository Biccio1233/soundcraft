//! ONNX Runtime's prebuilt CoreML objects call `__isPlatformVersionAtLeast` (behind `@available`),
//! which lives in clang's runtime library; Rust does not link it, so link it here on macOS.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let Ok(out) = std::process::Command::new("clang").arg("--print-resource-dir").output() else { return };
    let dir = format!("{}/lib/darwin", String::from_utf8_lossy(&out.stdout).trim());
    if std::path::Path::new(&dir).join("libclang_rt.osx.a").exists() {
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
}
