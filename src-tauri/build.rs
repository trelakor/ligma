fn main() {
    // BiRefNet, for Remove background (src/background.rs), runs where ONNX
    // Runtime does: Windows, and Macs with Apple silicon
    println!("cargo::rustc-check-cfg=cfg(birefnet)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if os == "windows" || (os == "macos" && arch == "aarch64") {
        println!("cargo:rustc-cfg=birefnet");
    }
    tauri_build::build()
}
