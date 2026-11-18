fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../icons/icon.ico");
        let mut res = tauri_winres::WindowsResource::new();
        res.set_icon("../../icons/icon.ico");
        res.compile().expect("couldn't embed the installer window's icon");
    }
}
