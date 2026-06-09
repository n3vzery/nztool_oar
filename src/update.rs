use log::warn;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

pub struct UpdateState {
    pub has_update: bool,
    pub new_version: String,
    pub download_url: String,
    pub skipped: bool,
    pub in_progress: bool,
    pub status: String,
    pub body: Option<String>,
}

pub static UPDATE_STATE: OnceLock<Arc<Mutex<UpdateState>>> = OnceLock::new();

#[derive(serde::Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub body: Option<String>,
    pub assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
}

pub fn is_version_newer(new: &str, current: &str) -> bool {
    let new_parts: Vec<&str> = new.split('.').collect();
    let cur_parts: Vec<&str> = current.split('.').collect();
    for i in 0..std::cmp::max(new_parts.len(), cur_parts.len()) {
        let n = new_parts
            .get(i)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let c = cur_parts
            .get(i)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        if n > c {
            return true;
        } else if n < c {
            return false;
        }
    }
    false
}

pub fn init_state() -> Arc<Mutex<UpdateState>> {
    UPDATE_STATE.get_or_init(|| {
        Arc::new(Mutex::new(UpdateState {
            has_update: false,
            new_version: String::new(),
            download_url: String::new(),
            skipped: false,
            in_progress: false,
            status: String::new(),
            body: None,
        }))
    }).clone()
}

pub fn check_for_updates() {
    let state = init_state();
    thread::spawn(move || {
        let response = ureq::get("https://api.github.com/repos/n3vzery/nztool_oar/releases/latest")
            .set("User-Agent", "nztool_oar")
            .call();

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to check for updates: {:?}", e);
                return;
            }
        };

        let release: GithubRelease = match response.into_json() {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to parse update release JSON: {:?}", e);
                return;
            }
        };

        let clean_tag = release.tag_name.trim_start_matches('v');
        let current_version = "2.4.1";

        if is_version_newer(clean_tag, current_version) {
            if let Some(asset) = release.assets.iter().find(|a| a.name == "nztool_oar.exe") {
                if let Ok(mut s) = state.lock() {
                    s.has_update = true;
                    s.new_version = release.tag_name.clone();
                    s.download_url = asset.browser_download_url.clone();
                    s.status = format!("New version {} is available!", release.tag_name);
                    s.body = release.body.clone();
                }
            }
        }
    });
}

pub fn start_update() {
    let state = init_state();
    thread::spawn(move || {
        let download_url = {
            let mut s = state.lock().unwrap();
            s.in_progress = true;
            s.status = "Downloading update...".to_string();
            s.download_url.clone()
        };

        let response = match ureq::get(&download_url).call() {
            Ok(r) => r,
            Err(e) => {
                let mut s = state.lock().unwrap();
                s.status = format!("Download failed: {:?}", e);
                s.in_progress = false;
                return;
            }
        };

        let mut bytes = Vec::new();
        use std::io::Read;
        if let Err(e) = response.into_reader().read_to_end(&mut bytes) {
            let mut s = state.lock().unwrap();
            s.status = format!("Failed to read data: {:?}", e);
            s.in_progress = false;
            return;
        }

        let exe_path = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                let mut s = state.lock().unwrap();
                s.status = format!("Failed to get current exe path: {:?}", e);
                s.in_progress = false;
                return;
            }
        };

        let mut old_path = exe_path.clone();
        old_path.set_extension("exe.old");

        if old_path.exists() {
            let _ = std::fs::remove_file(&old_path);
        }

        if let Err(e) = std::fs::rename(&exe_path, &old_path) {
            let mut s = state.lock().unwrap();
            s.status = format!("Failed to rename current exe: {:?}", e);
            s.in_progress = false;
            return;
        }

        if let Err(e) = std::fs::write(&exe_path, bytes) {
            let _ = std::fs::rename(&old_path, &exe_path);
            let mut s = state.lock().unwrap();
            s.status = format!("Failed to write new exe: {:?}", e);
            s.in_progress = false;
            return;
        }

        let _ = std::process::Command::new(&exe_path).spawn();
        std::process::exit(0);
    });
}
