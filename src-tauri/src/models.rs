//! The models Ligma runs on this device with ONNX Runtime: BiRefNet for
//! Remove background (see background.rs) and SAM 2.1 for Select object
//! (see objects.rs). Each is downloaded when it's first needed, once asked,
//! or from Settings, which can also remove it. Downloads are checked
//! against their SHA-256. ONNX Runtime is linked in on Windows. Macs with
//! Apple silicon download it along with the first model, as it needs
//! macOS 14

use serde::Serialize;
use tauri::{AppHandle, Manager, Webview};

/// A model as Settings shows it
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelState {
    /// This device can run it
    supported: bool,
    installed: bool,
    /// On disk once installed, otherwise to download
    bytes: u64,
    /// Percent downloaded, while it downloads
    downloading: Option<u8>,
}

/// Whether this device can run the models. ONNX Runtime needs macOS 14
pub fn supported() -> bool {
    cfg!(birefnet) && crate::background::available()
}

#[cfg(birefnet)]
pub use download::*;

/// The model Settings names: "background" or "objects"
#[cfg(birefnet)]
fn named(name: &str) -> Option<&'static Model> {
    [&BACKGROUND, &OBJECTS].into_iter().find(|model| model.name == name)
}

fn model_state(app: &AppHandle, name: &str) -> ModelState {
    #[cfg(birefnet)]
    if let Some(model) = named(name).filter(|_| supported()) {
        return model.state(app);
    }
    let _ = (app, name);
    ModelState::default()
}

/// Shows the model's state in the settings window, if open
#[cfg_attr(not(birefnet), allow(dead_code))]
fn show_state(app: &AppHandle, name: &str) {
    if let Some(window) = app.get_webview_window(crate::settings::WINDOW) {
        let state = serde_json::to_string(&model_state(app, name)).unwrap();
        let _ = window.eval(format!("window.applyModel?.({name:?}, {state})"));
    }
}

#[tauri::command]
pub async fn get_model(app: AppHandle, webview: Webview, name: String) -> Option<ModelState> {
    crate::settings::from_settings_window(&webview).then(|| model_state(&app, &name))
}

/// Downloads a model from Settings
#[tauri::command]
pub async fn install_model(app: AppHandle, webview: Webview, name: String) {
    if !crate::settings::from_settings_window(&webview) || !supported() {
        return;
    }
    #[cfg(birefnet)]
    if let Some(model) = named(&name) {
        model.install(&app);
    }
    let _ = (app, name);
}

/// Stops a model's download
#[tauri::command]
pub async fn cancel_model(webview: Webview, name: String) {
    if crate::settings::from_settings_window(&webview) {
        #[cfg(birefnet)]
        if let Some(model) = named(&name) {
            model.cancel();
        }
        let _ = name;
    }
}

/// Removes a model from Settings
#[tauri::command]
pub async fn uninstall_model(app: AppHandle, webview: Webview, name: String) {
    if !crate::settings::from_settings_window(&webview) {
        return;
    }
    #[cfg(birefnet)]
    if let Some(model) = named(&name) {
        model.uninstall(&app);
    }
    let _ = (app, name);
}

#[cfg(birefnet)]
mod download {
    use std::fs::File;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};

    use ort::environment::Environment;
    use sha2::{Digest, Sha256};
    use tauri::{AppHandle, Manager};

    use super::ModelState;

    /// A file a model needs, downloaded into the models folder as `file`,
    /// or as `entry` taken out of a .tgz. `size` and `sha256` are the
    /// download's
    pub struct Download {
        url: &'static str,
        sha256: &'static str,
        size: u64,
        pub file: &'static str,
        entry: Option<&'static str>,
    }

    /// Microsoft's build, which needs macOS 14
    #[cfg(target_os = "macos")]
    pub const RUNTIME: Download = Download {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-osx-arm64-1.31.1.tgz",
        sha256: "2e1344a1926ee50eb25b38153cb6992c3567515aef0e677a1a520a1798896d6f",
        size: 36_936_435,
        file: "libonnxruntime.dylib",
        entry: Some("onnxruntime-osx-arm64-1.31.1/lib/libonnxruntime.1.31.1.dylib"),
    };

    /// BiRefNet-lite (https://github.com/ZhengPeng7/BiRefNet, MIT), as ONNX
    /// from https://huggingface.co/onnx-community/BiRefNet_lite-ONNX, pinned
    /// to one revision
    pub const BIREFNET: Download = Download {
        url: "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx",
        sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
        size: 224_005_088,
        file: "birefnet-lite.onnx",
        entry: None,
    };

    /// SAM 2.1 Hiera Tiny (https://github.com/facebookresearch/sam2, Apache
    /// 2.0), as ONNX from https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX,
    /// pinned to one revision: the image encoder, quantized to 8 bits, and
    /// the prompt encoder and mask decoder. Their weights are in files of
    /// their own, which keep the names the models refer to them by
    macro_rules! sam {
        ($file:literal) => {
            concat!("https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/814a066640debee5a91e70aa401fb8e17e030503/onnx/", $file)
        };
    }
    pub const SAM_ENCODER: Download = Download {
        url: sam!("vision_encoder_int8.onnx"),
        sha256: "d848e08b747ba44ee59b31246988528d5ecc67d1e1c0a2f5c08395c86d92176f",
        size: 440_423,
        file: "sam2.1-tiny/vision_encoder_int8.onnx",
        entry: None,
    };
    const SAM_ENCODER_DATA: Download = Download {
        url: sam!("vision_encoder_int8.onnx_data"),
        sha256: "4359d45f8aafcbc69f945e714af7fb06b74b24c753b5af86b6a230ecd0e737bc",
        size: 52_573_088,
        file: "sam2.1-tiny/vision_encoder_int8.onnx_data",
        entry: None,
    };
    pub const SAM_DECODER: Download = Download {
        url: sam!("prompt_encoder_mask_decoder.onnx"),
        sha256: "874414704c5d686db7d206a35f6e15d26563d50c8c4468fccc6739bd7e491dcf",
        size: 213_114,
        file: "sam2.1-tiny/prompt_encoder_mask_decoder.onnx",
        entry: None,
    };
    const SAM_DECODER_DATA: Download = Download {
        url: sam!("prompt_encoder_mask_decoder.onnx_data"),
        sha256: "e9874d900dd4134ed60eab1e97910327c2419e0b2954485d8fd6e7f1a1470f47",
        size: 20_958_208,
        file: "sam2.1-tiny/prompt_encoder_mask_decoder.onnx_data",
        entry: None,
    };

    /// A model and what Ligma says about it
    pub struct Model {
        /// As Settings names it
        pub name: &'static str,
        files: &'static [&'static Download],
        /// What it's for, and the model, as the prompt to download it says
        doing: &'static str,
        model: &'static str,
        title: &'static str,
        /// Figma's toast while it downloads, and once it's done
        progress: &'static str,
        after: &'static str,
        /// Percent downloaded, while it downloads
        downloading: Mutex<Option<u8>>,
        /// Settings' Cancel
        cancel: AtomicBool,
        /// Jobs waiting on it, told how far its download has got: tab and
        /// job
        watchers: Mutex<Vec<(u32, u64)>>,
    }

    pub static BACKGROUND: Model = Model {
        name: "background",
        files: &[&BIREFNET],
        doing: "Removing backgrounds",
        model: "BiRefNet",
        title: "Download Background Model",
        progress: "Downloading the background model…",
        after: "Removing background…",
        downloading: Mutex::new(None),
        cancel: AtomicBool::new(false),
        watchers: Mutex::new(Vec::new()),
    };

    pub static OBJECTS: Model = Model {
        name: "objects",
        files: &[&SAM_ENCODER, &SAM_ENCODER_DATA, &SAM_DECODER, &SAM_DECODER_DATA],
        doing: "Selecting objects",
        model: "SAM 2.1",
        title: "Download Object Model",
        progress: "Downloading the object model…",
        after: "Reading the image…",
        downloading: Mutex::new(None),
        cancel: AtomicBool::new(false),
        watchers: Mutex::new(Vec::new()),
    };

    /// Why a model isn't there to use
    pub enum Missing {
        /// The person didn't want it downloaded, or cancelled it
        Cancelled,
        Failed,
    }

    /// Held while a model is looked for, downloaded, loaded, used or
    /// removed, so two images wait for one download and it isn't removed
    /// while in use
    static LOCK: Mutex<()> = Mutex::new(());

    pub fn lock() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn folder(app: &AppHandle) -> Result<PathBuf, String> {
        Ok(app.path().app_local_data_dir().map_err(|error| error.to_string())?.join("models"))
    }

    /// While a job waits on a model, it's told how far its download has got
    struct Watching(&'static Model, u32, u64);

    impl Drop for Watching {
        fn drop(&mut self) {
            self.0.watchers.lock().unwrap().retain(|&watcher| watcher != (self.1, self.2));
        }
    }

    impl Model {
        /// Everything it needs, ONNX Runtime first on macOS
        fn downloads(&self) -> impl Iterator<Item = &'static Download> + '_ {
            #[cfg(target_os = "macos")]
            let runtime = Some(&RUNTIME);
            #[cfg(windows)]
            let runtime: Option<&'static Download> = None;
            runtime.into_iter().chain(self.files.iter().copied())
        }

        fn installed(&self, folder: &Path) -> bool {
            self.downloads().all(|download| folder.join(download.file).is_file())
        }

        pub fn state(&self, app: &AppHandle) -> ModelState {
            let downloading = *self.downloading.lock().unwrap();
            let Ok(folder) = folder(app) else {
                return ModelState { supported: true, downloading, ..Default::default() };
            };
            let installed = self.installed(&folder);
            let bytes = if installed {
                self.downloads()
                    .filter_map(|download| std::fs::metadata(folder.join(download.file)).ok())
                    .map(|metadata| metadata.len())
                    .sum()
            } else {
                self.downloads().map(|download| download.size).sum()
            };
            ModelState { supported: true, installed, bytes, downloading }
        }

        fn tell_watchers(&self, app: &AppHandle, text: &str) {
            let watchers = self.watchers.lock().unwrap().clone();
            for (tab, job) in watchers {
                crate::handoff::progress(app, tab, job, text);
            }
        }

        /// The models folder, once the model is in it, for `job` in tab
        /// `tab`, which is told how its download goes. Asks first. Holds the
        /// lock while it's used
        pub fn ensure(&'static self, app: &AppHandle, tab: u32, job: u64) -> Result<(MutexGuard<'static, ()>, PathBuf), Missing> {
            self.watchers.lock().unwrap().push((tab, job));
            let _watching = Watching(self, tab, job);
            let held = lock();
            let folder = folder(app).map_err(|error| failed(self, error))?;
            if !self.installed(&folder) {
                if !self.ask(app, &folder) {
                    return Err(Missing::Cancelled);
                }
                match self.download(app, &folder) {
                    Ok(()) => {}
                    Err(None) => return Err(Missing::Cancelled),
                    Err(Some(error)) => {
                        self.download_failed(app, &error);
                        return Err(failed(self, error));
                    }
                }
            }
            Ok((held, folder))
        }

        /// Downloads it, unless it's there or on its way
        pub fn install(&'static self, app: &AppHandle) {
            let app = app.clone();
            std::thread::spawn(move || {
                let Ok(_held) = LOCK.try_lock() else {
                    return;
                };
                let result = folder(&app).map_err(Some).and_then(|folder| {
                    if self.installed(&folder) {
                        return Ok(());
                    }
                    self.download(&app, &folder)
                });
                if let Err(Some(error)) = result {
                    log::warn!("{}: {error}", self.model);
                    self.download_failed(&app, &error);
                }
            });
        }

        pub fn cancel(&self) {
            self.cancel.store(true, Ordering::SeqCst);
        }

        /// Removes it, once it's not in use. On macOS ONNX Runtime goes too,
        /// unless another model needs it
        pub fn uninstall(&'static self, app: &AppHandle) {
            let app = app.clone();
            std::thread::spawn(move || {
                if self.name == OBJECTS.name {
                    crate::objects::unload();
                }
                let _held = lock();
                if let Ok(folder) = folder(&app) {
                    let others = [&BACKGROUND, &OBJECTS].into_iter().filter(|other| other.name != self.name);
                    let shared = others.filter(|other| other.installed(&folder)).flat_map(|other| other.downloads()).collect::<Vec<_>>();
                    for download in self.downloads().filter(|download| !shared.iter().any(|kept| kept.file == download.file)) {
                        let path = folder.join(download.file);
                        if let Err(error) = std::fs::remove_file(&path) {
                            if error.kind() != std::io::ErrorKind::NotFound {
                                log::warn!("Couldn't remove {}: {error}", path.display());
                            }
                        }
                    }
                }
                for model in [&BACKGROUND, &OBJECTS] {
                    super::show_state(&app, model.name);
                }
            });
        }

        fn download_failed(&self, app: &AppHandle, error: &str) {
            let name = self.title.trim_start_matches("Download ").to_lowercase();
            crate::prompt::show(app, "Download Failed", format!("Couldn't download the {name}: {error}"), &["OK"], |_| {});
        }

        /// Whether the person agrees to download it, and ONNX Runtime with
        /// it on macOS if it's not there yet
        fn ask(&self, app: &AppHandle, folder: &Path) -> bool {
            let megabytes = |bytes: u64| (bytes as f64 / 1e6).round();
            let own = megabytes(self.files.iter().map(|download| download.size).sum());
            let missing: u64 =
                self.downloads().filter(|download| !folder.join(download.file).is_file()).map(|download| download.size).sum();
            let runtime = self.files.iter().map(|download| download.size).sum::<u64>() < missing;
            let (doing, model) = (self.doing, self.model);
            let message = if cfg!(windows) {
                format!("{doing} on this PC needs {model}, a {own} MB model that Ligma downloads once from Hugging Face. You can remove it in Settings.")
            } else if runtime {
                format!(
                    "{doing} with {model} needs the {own} MB model and ONNX Runtime to run it, {} MB in all, that Ligma downloads once from Hugging Face and GitHub. You can remove them in Settings.",
                    megabytes(missing)
                )
            } else {
                format!("{doing} with {model} needs a {own} MB model that Ligma downloads once from Hugging Face. You can remove it in Settings.")
            };
            let (send, receive) = mpsc::channel();
            crate::prompt::show(app, self.title, message, &["Cancel", "Download"], move |choice| {
                let _ = send.send(choice == Some(1));
            });
            receive.recv().unwrap_or(false)
        }

        /// Downloads what's missing into `folder`, telling Settings and the
        /// jobs waiting how far it's got. `Err(None)` when cancelled
        fn download(&self, app: &AppHandle, folder: &Path) -> Result<(), Option<String>> {
            self.cancel.store(false, Ordering::SeqCst);
            let missing: Vec<&Download> = self.downloads().filter(|download| !folder.join(download.file).is_file()).collect();
            let total = missing.iter().map(|download| download.size).sum::<u64>().max(1);
            let mut before = 0;
            let mut told = None;
            let mut report = |got: u64| {
                let percent = (got * 100 / total).min(100) as u8;
                if *self.downloading.lock().unwrap() != Some(percent) {
                    *self.downloading.lock().unwrap() = Some(percent);
                    super::show_state(app, self.name);
                }
                // In steps of 5%, so Figma's toast isn't redrawn too often
                if told != Some(percent / 5) {
                    told = Some(percent / 5);
                    self.tell_watchers(app, &format!("{} {}%", self.progress, percent / 5 * 5));
                }
            };
            report(0);
            let mut result = Ok(());
            for download in missing {
                let path = folder.join(download.file);
                result = std::fs::create_dir_all(path.parent().unwrap_or(folder)).map_err(|error| Some(error.to_string()));
                if result.is_ok() {
                    result = self.fetch(download, &path, &mut |got| report(before + got));
                }
                if result.is_err() {
                    break;
                }
                before += download.size;
            }
            *self.downloading.lock().unwrap() = None;
            // ONNX Runtime may now be there for the other model too
            for model in [&BACKGROUND, &OBJECTS] {
                super::show_state(app, model.name);
            }
            if result.is_ok() {
                self.tell_watchers(app, self.after);
            }
            result
        }

        /// Downloads `download` to `path`, through a `.part` file that's
        /// only kept once its hash matches. `Err(None)` when cancelled
        fn fetch(&self, download: &Download, path: &Path, report: &mut impl FnMut(u64)) -> Result<(), Option<String>> {
            let part = path.with_file_name(format!("{}.part", path.file_name().unwrap_or_default().to_string_lossy()));
            let result = tauri::async_runtime::block_on(async {
                let error = |error: &dyn std::fmt::Display| Some(error.to_string());
                let client = reqwest::Client::builder().tls_backend_native().build().map_err(|e| error(&e))?;
                let mut response = client
                    .get(download.url)
                    .send()
                    .await
                    .and_then(|response| response.error_for_status())
                    .map_err(|e| error(&e))?;
                let mut file = File::create(&part).map_err(|e| error(&e))?;
                let mut hash = Sha256::new();
                let mut got = 0u64;
                while let Some(chunk) = response.chunk().await.map_err(|e| error(&e))? {
                    if self.cancel.load(Ordering::SeqCst) {
                        return Err(None);
                    }
                    file.write_all(&chunk).map_err(|e| error(&e))?;
                    hash.update(&chunk);
                    got += chunk.len() as u64;
                    report(got.min(download.size));
                }
                file.sync_all().map_err(|e| error(&e))?;
                if format!("{:x}", hash.finalize()) != download.sha256 {
                    return Err(Some("the file didn't match".to_owned()));
                }
                Ok(())
            });
            let result = result.and_then(|()| match download.entry {
                Some(entry) => extract(&part, entry, path).map_err(Some),
                None => std::fs::rename(&part, path).map_err(|error| Some(error.to_string())),
            });
            let _ = std::fs::remove_file(&part);
            result
        }
    }

    fn failed(model: &Model, error: impl std::fmt::Display) -> Missing {
        log::warn!("{}: {error}", model.model);
        Missing::Failed
    }

    /// Takes `entry` out of the .tgz `archive`, into `path`
    #[cfg(target_os = "macos")]
    fn extract(archive: &Path, entry: &str, path: &Path) -> Result<(), String> {
        let temp = path.with_extension("tmp");
        let result = (|| {
            let archive = File::open(archive).map_err(|error| error.to_string())?;
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
            for file in archive.entries().map_err(|error| error.to_string())? {
                let mut file = file.map_err(|error| error.to_string())?;
                if file.path().map_err(|error| error.to_string())?.as_ref() == Path::new(entry) {
                    file.unpack(&temp).map_err(|error| error.to_string())?;
                    return std::fs::rename(&temp, path).map_err(|error| error.to_string());
                }
            }
            Err("ONNX Runtime wasn't in the download".to_owned())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    #[cfg(windows)]
    fn extract(_: &Path, _: &str, _: &Path) -> Result<(), String> {
        Err("nothing to extract on Windows".to_owned())
    }

    /// ONNX Runtime, loaded from `folder` on macOS. Once loaded it stays,
    /// even if the files are removed
    pub fn environment(folder: &Path) -> Result<&'static Environment, String> {
        static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();
        if let Some(environment) = ENVIRONMENT.get() {
            return Ok(environment);
        }
        #[cfg(target_os = "macos")]
        let builder = ort::init_from(folder.join(RUNTIME.file)).map_err(|error| error.to_string())?;
        #[cfg(windows)]
        let builder = {
            let _ = folder;
            ort::init()
        };
        let environment = builder.with_name("ligma").build().map_err(|error| error.to_string())?;
        Ok(ENVIRONMENT.get_or_init(|| environment))
    }
}
