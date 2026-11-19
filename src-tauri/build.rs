fn main() {
    // BiRefNet, for Remove background (src/background.rs), runs where ONNX
    // Runtime does: Windows, and Macs with Apple silicon
    println!("cargo::rustc-check-cfg=cfg(birefnet)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if os == "windows" || (os == "macos" && arch == "aarch64") {
        println!("cargo:rustc-cfg=birefnet");
    }
    // ONNX Runtime, linked in on Windows, links to DirectML, which isn't on
    // every Windows. It's loaded only once Upscale uses the graphics card,
    // after checking it's there (src/upscale.rs), so Ligma starts without it
    if os == "windows" && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:DirectML.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
    // The commit built, for dev traces
    if let Ok(out) = std::process::Command::new("git").args(["rev-parse", "--short", "HEAD"]).output() {
        if out.status.success() {
            println!("cargo:rustc-env=LIGMA_COMMIT={}", String::from_utf8_lossy(&out.stdout).trim());
            // Built again once the commit changes: HEAD's log moves with each
            if let Ok(log) = std::process::Command::new("git").args(["rev-parse", "--git-path", "logs/HEAD"]).output() {
                println!("cargo:rerun-if-changed={}", String::from_utf8_lossy(&log.stdout).trim());
            }
        }
    }
    tauri_build::build()
}
