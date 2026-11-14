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
    tauri_build::build()
}
