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
        copy_msvc_runtime(&arch);
    }
    tauri_build::build()
}

// ONNX Runtime's C++ needs the MSVC C++ runtime DLLs, which a fresh Windows
// doesn't have. They're copied here from Visual Studio's redist folder and
// installed next to ligma.exe (tauri.windows.conf.json), as Microsoft allows
const MSVC_RUNTIME: [&str; 4] = [
    "msvcp140.dll",
    "msvcp140_1.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
];

fn copy_msvc_runtime(arch: &str) {
    use std::path::PathBuf;
    let dest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("gen/msvc-runtime");
    for dll in MSVC_RUNTIME {
        println!("cargo:rerun-if-changed={}", dest.join(dll).display());
    }
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => panic!("no MSVC runtime for {other}"),
    };
    let src = find_msvc_redist(arch).unwrap_or_else(|| {
        panic!("MSVC runtime for {arch} not found, install Visual Studio's C++ build tools")
    });
    std::fs::create_dir_all(&dest).unwrap();
    for dll in MSVC_RUNTIME {
        let from = src.join(dll);
        std::fs::copy(&from, dest.join(dll))
            .unwrap_or_else(|e| panic!("copying {}: {e}", from.display()));
    }
}

// The newest <redist>\MSVC\<version>\<arch>\Microsoft.VC*.CRT, from the
// Developer Command Prompt's VCToolsRedistDir or else vswhere
fn find_msvc_redist(arch: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let crt_in = |version: &Path| -> Option<PathBuf> {
        std::fs::read_dir(version.join(arch)).ok()?.flatten().map(|e| e.path()).find(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            name.starts_with("Microsoft.VC") && name.ends_with(".CRT") && p.join("msvcp140_1.dll").is_file()
        })
    };
    println!("cargo:rerun-if-env-changed=VCToolsRedistDir");
    if let Some(dir) = std::env::var_os("VCToolsRedistDir") {
        if let Some(crt) = crt_in(Path::new(&dir)) {
            return Some(crt);
        }
    }
    let vswhere = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?)
        .join("Microsoft Visual Studio/Installer/vswhere.exe");
    let out = std::process::Command::new(vswhere)
        .args(["-latest", "-products", "*", "-property", "installationPath"])
        .output()
        .ok()?;
    let install = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut versions: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(Path::new(&install).join("VC/Redist/MSVC"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let parts: Option<Vec<u32>> = name.split('.').map(|n| n.parse().ok()).collect();
            Some((parts?, e.path()))
        })
        .collect();
    versions.sort();
    versions.iter().rev().find_map(|(_, path)| crt_in(path))
}
