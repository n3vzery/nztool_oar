use crate::features::{
    BindKey, ConfigKey, DoubleClickButton, Feature, FeatureId, SerializableFeature,
};
use log::{error, warn};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// global state
pub struct GlobalInputState {
    pub lmb_down: AtomicBool,
    pub space_down: AtomicBool,
    pub autoclicker_active: AtomicBool,
    pub bhop_active: AtomicBool,
    pub ctrl_down: AtomicBool,
    pub shift_down: AtomicBool,
    pub alt_down: AtomicBool,
    pub capslock_down: AtomicBool,
    pub mouse_hook_thread_id: AtomicU32,
    pub keyboard_hook_thread_id: AtomicU32,
    pub last_focus_check: AtomicU64,
    pub cached_focus_value: AtomicBool,
    pub rdev_shutdown: AtomicBool,
    pub lmb_hold_active: AtomicBool,
    pub click_debug_enabled: AtomicBool,
    pub all_macros_disabled: AtomicBool,
}

impl GlobalInputState {
    pub const fn new() -> Self {
        Self {
            lmb_down: AtomicBool::new(false),
            space_down: AtomicBool::new(false),
            autoclicker_active: AtomicBool::new(false),
            bhop_active: AtomicBool::new(false),
            ctrl_down: AtomicBool::new(false),
            shift_down: AtomicBool::new(false),
            alt_down: AtomicBool::new(false),
            capslock_down: AtomicBool::new(false),
            mouse_hook_thread_id: AtomicU32::new(0),
            keyboard_hook_thread_id: AtomicU32::new(0),
            last_focus_check: AtomicU64::new(0),
            cached_focus_value: AtomicBool::new(false),
            rdev_shutdown: AtomicBool::new(false),
            lmb_hold_active: AtomicBool::new(false),
            click_debug_enabled: AtomicBool::new(false),
            all_macros_disabled: AtomicBool::new(false),
        }
    }
}

pub static GLOBAL_STATE: GlobalInputState = GlobalInputState::new();

#[derive(Clone)]
pub struct InputState;

impl InputState {
    pub fn is_lmb_down(&self) -> bool {
        GLOBAL_STATE.lmb_down.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn set_lmb_down(&self, down: bool) {
        GLOBAL_STATE.lmb_down.store(down, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn set_space_down(&self, down: bool) {
        GLOBAL_STATE.space_down.store(down, Ordering::SeqCst);
    }

    pub fn is_space_down(&self) -> bool {
        GLOBAL_STATE.space_down.load(Ordering::SeqCst)
    }

    pub fn toggle_autoclicker(&self) -> bool {
        let current = GLOBAL_STATE.autoclicker_active.load(Ordering::SeqCst);
        GLOBAL_STATE
            .autoclicker_active
            .store(!current, Ordering::SeqCst);
        !current
    }

    pub fn is_autoclicker_active(&self) -> bool {
        GLOBAL_STATE.autoclicker_active.load(Ordering::SeqCst)
    }

    pub fn toggle_bhop(&self) -> bool {
        let current = GLOBAL_STATE.bhop_active.load(Ordering::SeqCst);
        GLOBAL_STATE.bhop_active.store(!current, Ordering::SeqCst);
        !current
    }

    pub fn is_bhop_active(&self) -> bool {
        GLOBAL_STATE.bhop_active.load(Ordering::SeqCst)
    }

    pub fn toggle_lmb_hold(&self) -> bool {
        let current = GLOBAL_STATE.lmb_hold_active.load(Ordering::SeqCst);
        GLOBAL_STATE
            .lmb_hold_active
            .store(!current, Ordering::SeqCst);
        !current
    }

    pub fn is_lmb_hold_active(&self) -> bool {
        GLOBAL_STATE.lmb_hold_active.load(Ordering::SeqCst)
    }

    pub fn set_lmb_hold_active(&self, active: bool) {
        GLOBAL_STATE.lmb_hold_active.store(active, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn is_ctrl_down(&self) -> bool {
        GLOBAL_STATE.ctrl_down.load(Ordering::SeqCst)
    }

    pub fn is_shift_down(&self) -> bool {
        GLOBAL_STATE.shift_down.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn is_alt_down(&self) -> bool {
        GLOBAL_STATE.alt_down.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn is_capslock_down(&self) -> bool {
        GLOBAL_STATE.capslock_down.load(Ordering::SeqCst)
    }

    pub fn set_mouse_hook_thread_id(&self, id: u32) {
        GLOBAL_STATE
            .mouse_hook_thread_id
            .store(id, Ordering::SeqCst);
    }

    pub fn get_mouse_hook_thread_id(&self) -> u32 {
        GLOBAL_STATE.mouse_hook_thread_id.load(Ordering::SeqCst)
    }

    pub fn set_keyboard_hook_thread_id(&self, id: u32) {
        GLOBAL_STATE
            .keyboard_hook_thread_id
            .store(id, Ordering::SeqCst);
    }

    pub fn get_keyboard_hook_thread_id(&self) -> u32 {
        GLOBAL_STATE.keyboard_hook_thread_id.load(Ordering::SeqCst)
    }

    pub fn toggle_all_macros_disabled(&self) -> bool {
        let current = GLOBAL_STATE.all_macros_disabled.load(Ordering::SeqCst);
        GLOBAL_STATE
            .all_macros_disabled
            .store(!current, Ordering::SeqCst);
        !current
    }

    pub fn are_all_macros_disabled(&self) -> bool {
        GLOBAL_STATE.all_macros_disabled.load(Ordering::SeqCst)
    }
}

pub fn safe_lock<T>(mutex: &Arc<Mutex<T>>) -> Option<std::sync::MutexGuard<'_, T>> {
    match mutex.lock() {
        Ok(guard) => Some(guard),
        Err(poisoned) => {
            error!("Mutex poisoned, recovering with poisoned data");
            Some(poisoned.into_inner())
        }
    }
}

pub fn is_game_focused() -> bool {
    const FOCUS_CACHE_TTL_MS: u64 = 100;
    let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    let last = GLOBAL_STATE.last_focus_check.load(Ordering::Relaxed);
    if now.saturating_sub(last) < FOCUS_CACHE_TTL_MS {
        return GLOBAL_STATE.cached_focus_value.load(Ordering::Relaxed);
    }
    let result = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            false
        } else {
            let mut process_id = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            if process_id == 0 {
                false
            } else {
                let handle = OpenProcess(
                    PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
                    false,
                    process_id,
                );
                if let Ok(h) = handle {
                    let mut buffer = [0u8; 260];
                    let len = K32GetModuleBaseNameA(h, None, &mut buffer);
                    if CloseHandle(h).is_err() {
                        warn!("CloseHandle failed in is_game_focused");
                    }
                    if len > 0 {
                        let name = std::str::from_utf8(&buffer[..len as usize]).unwrap_or("");
                        name.eq_ignore_ascii_case("OAR-Win64-Shipping.exe")
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    };
    GLOBAL_STATE
        .cached_focus_value
        .store(result, Ordering::Relaxed);
    GLOBAL_STATE.last_focus_check.store(now, Ordering::Relaxed);
    result
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum ClickMethod {
    #[default]
    SendInput,
    PostMessage,
}

impl std::fmt::Display for ClickMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClickMethod::SendInput => write!(f, "SendInput"),
            ClickMethod::PostMessage => write!(f, "PostMessage"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum AutoClickerMode {
    #[default]
    Mouse,
    Keyboard,
}

impl std::fmt::Display for AutoClickerMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutoClickerMode::Mouse => write!(f, "Mouse"),
            AutoClickerMode::Keyboard => write!(f, "Keyboard"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SerializableConfig {
    pub monitor_id: String,
    pub features: Vec<SerializableFeature>,
    pub auto_clicker_delay: u32,
    pub auto_clicker_method: ClickMethod,
    #[serde(default = "default_auto_clicker_click_count")]
    pub auto_clicker_click_count: u32,
    #[serde(default = "default_auto_clicker_mode")]
    pub auto_clicker_mode: AutoClickerMode,
    #[serde(default = "default_auto_clicker_key")]
    pub auto_clicker_key: ConfigKey,
    pub position_x: i32,
    pub position_y: i32,
    #[serde(default = "default_tips_skip_y")]
    pub tips_skip_y_offset: i32,
    #[serde(default = "default_restart_y")]
    pub restart_y_offset: i32,
    #[serde(default = "default_hacking_y")]
    pub hacking_y_offset: i32,
    #[serde(default = "default_hacking2_y")]
    pub hacking2_y_offset: i32,
    #[serde(default = "default_hacking_esc_y")]
    pub hacking_esc_y_offset: i32,
    #[serde(default = "default_gun_tool_digit")]
    pub gun_tool_digit: u32,
    #[serde(default = "default_double_click_button")]
    pub double_click_button: DoubleClickButton,
}

fn default_double_click_button() -> DoubleClickButton {
    DoubleClickButton::Left
}
fn default_tips_skip_y() -> i32 {
    830
}
fn default_restart_y() -> i32 {
    486
}
fn default_hacking_y() -> i32 {
    -140
}
fn default_hacking2_y() -> i32 {
    -140
}
fn default_hacking_esc_y() -> i32 {
    -140
}
fn default_auto_clicker_click_count() -> u32 {
    1
}
fn default_gun_tool_digit() -> u32 {
    1
}
fn default_auto_clicker_mode() -> AutoClickerMode {
    AutoClickerMode::Mouse
}
fn default_auto_clicker_key() -> ConfigKey {
    ConfigKey::KeyE
}

pub struct AppState {
    pub features: Vec<Feature>,
    pub monitor_id: String,
    pub x_offset: i32,
    pub y_offset: i32,
    pub width: i32,
    pub height: i32,
    pub shift_held: bool,
    pub auto_clicker_delay: u32,
    pub auto_clicker_method: ClickMethod,
    pub auto_clicker_click_count: u32,
    pub auto_clicker_mode: AutoClickerMode,
    pub auto_clicker_key: ConfigKey,
    pub position_x: i32,
    pub position_y: i32,
    pub tips_skip_y_offset: i32,
    pub restart_y_offset: i32,
    pub hacking_y_offset: i32,
    pub hacking2_y_offset: i32,
    pub hacking_esc_y_offset: i32,
    pub gun_tool_digit: u32,
    pub dev_mode: bool,
    pub presets: Vec<String>,
    pub selected_preset: String,
    pub preset_name_input: String,
    pub double_click_button: DoubleClickButton,
}

pub fn get_config_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(appdata)
        .join("nzconfig")
        .join("config.json")
}

pub fn get_presets_dir() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(appdata)
        .join("nzconfig")
        .join("presets")
}

impl AppState {
    pub fn save_config(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = get_config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let features: Vec<SerializableFeature> = self
            .features
            .iter()
            .map(|f| SerializableFeature {
                id: f.id,
                bind_key: f.bind_key.as_ref().map(|k| k.to_string()),
                enabled: f.enabled,
            })
            .collect();

        let config = SerializableConfig {
            monitor_id: self.monitor_id.clone(),
            features,
            auto_clicker_delay: self.auto_clicker_delay,
            auto_clicker_method: self.auto_clicker_method,
            auto_clicker_click_count: self.auto_clicker_click_count,
            auto_clicker_mode: self.auto_clicker_mode,
            auto_clicker_key: self.auto_clicker_key,
            position_x: self.position_x,
            position_y: self.position_y,
            tips_skip_y_offset: self.tips_skip_y_offset,
            restart_y_offset: self.restart_y_offset,
            hacking_y_offset: self.hacking_y_offset,
            hacking2_y_offset: self.hacking2_y_offset,
            hacking_esc_y_offset: self.hacking_esc_y_offset,
            gun_tool_digit: self.gun_tool_digit,
            double_click_button: self.double_click_button,
        };

        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;
        Ok(())
    }

    pub fn load_config(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = get_config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !config_path.exists() {
            return Ok(());
        }
        let json = std::fs::read_to_string(&config_path)?;
        let config: SerializableConfig = serde_json::from_str(&json)?;
        self.monitor_id = config.monitor_id;
        for sf in config.features {
            if let Some(feature) = self.features.iter_mut().find(|f| f.id == sf.id) {
                feature.bind_key = sf.bind_key.as_deref().and_then(BindKey::from_string);
                feature.enabled = sf.enabled;
            }
        }
        self.auto_clicker_delay = config.auto_clicker_delay;
        self.auto_clicker_method = config.auto_clicker_method;
        self.auto_clicker_click_count = config.auto_clicker_click_count;
        self.auto_clicker_mode = config.auto_clicker_mode;
        self.auto_clicker_key = config.auto_clicker_key;
        self.position_x = config.position_x;
        self.position_y = config.position_y;
        self.tips_skip_y_offset = config.tips_skip_y_offset;
        self.restart_y_offset = config.restart_y_offset;
        self.hacking_y_offset = config.hacking_y_offset;
        self.hacking2_y_offset = config.hacking2_y_offset;
        self.hacking_esc_y_offset = config.hacking_esc_y_offset;
        self.gun_tool_digit = config.gun_tool_digit.clamp(1, 3);
        self.double_click_button = config.double_click_button;
        Ok(())
    }

    pub fn refresh_presets(&mut self) {
        let presets_dir = get_presets_dir();
        let mut list = Vec::new();
        let _ = std::fs::create_dir_all(&presets_dir);
        if let Ok(entries) = std::fs::read_dir(&presets_dir) {
            for entry in entries {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            list.push(stem.to_string());
                        }
                    }
                }
            }
        }
        list.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        self.presets = list;
    }

    pub fn save_preset(&self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let presets_dir = get_presets_dir();
        std::fs::create_dir_all(&presets_dir)?;
        let path = presets_dir.join(format!("{}.json", name));
        let features: Vec<SerializableFeature> = self
            .features
            .iter()
            .map(|f| SerializableFeature {
                id: f.id,
                bind_key: f.bind_key.as_ref().map(|k| k.to_string()),
                enabled: f.enabled,
            })
            .collect();

        let config = SerializableConfig {
            monitor_id: self.monitor_id.clone(),
            features,
            auto_clicker_delay: self.auto_clicker_delay,
            auto_clicker_method: self.auto_clicker_method,
            auto_clicker_click_count: self.auto_clicker_click_count,
            auto_clicker_mode: self.auto_clicker_mode,
            auto_clicker_key: self.auto_clicker_key,
            position_x: self.position_x,
            position_y: self.position_y,
            tips_skip_y_offset: self.tips_skip_y_offset,
            restart_y_offset: self.restart_y_offset,
            hacking_y_offset: self.hacking_y_offset,
            hacking2_y_offset: self.hacking2_y_offset,
            hacking_esc_y_offset: self.hacking_esc_y_offset,
            gun_tool_digit: self.gun_tool_digit,
            double_click_button: self.double_click_button,
        };

        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn load_preset(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let presets_dir = get_presets_dir();
        let path = presets_dir.join(format!("{}.json", name));
        if !path.exists() {
            return Err("Preset file does not exist".into());
        }
        let json = std::fs::read_to_string(&path)?;
        let config: SerializableConfig = serde_json::from_str(&json)?;
        self.monitor_id = config.monitor_id;
        for sf in config.features {
            if let Some(feature) = self.features.iter_mut().find(|f| f.id == sf.id) {
                feature.bind_key = sf.bind_key.as_deref().and_then(BindKey::from_string);
                feature.enabled = sf.enabled;
            }
        }
        self.auto_clicker_delay = config.auto_clicker_delay;
        self.auto_clicker_method = config.auto_clicker_method;
        self.auto_clicker_click_count = config.auto_clicker_click_count;
        self.auto_clicker_mode = config.auto_clicker_mode;
        self.auto_clicker_key = config.auto_clicker_key;
        self.position_x = config.position_x;
        self.position_y = config.position_y;
        self.tips_skip_y_offset = config.tips_skip_y_offset;
        self.restart_y_offset = config.restart_y_offset;
        self.hacking_y_offset = config.hacking_y_offset;
        self.hacking2_y_offset = config.hacking2_y_offset;
        self.hacking_esc_y_offset = config.hacking_esc_y_offset;
        self.gun_tool_digit = config.gun_tool_digit.clamp(1, 3);
        self.double_click_button = config.double_click_button;
        let _ = self.save_config();
        Ok(())
    }

    pub fn delete_preset(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let presets_dir = get_presets_dir();
        let path = presets_dir.join(format!("{}.json", name));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    pub fn new() -> Self {
        let mut state = Self {
            features: vec![
                Feature {
                    id: FeatureId::HackingClickMtd,
                    name: "Hacking Device (Click mtd)".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::HackingJumpMtd,
                    name: "Hacking Device (Jump mtd)".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::HackingEscMtd,
                    name: "Hacking Device (Esc mtd)".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::TipsSkip,
                    name: "Tips Skip".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::Restart,
                    name: "Restart".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::NoFallDamage,
                    name: "No Fall Damage".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::ShiftToggle,
                    name: "Shift Toggle".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::AutoClicker,
                    name: "Auto Clicker".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::KeepItemClicker,
                    name: "Keep Item Clicker".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::DoubleClick,
                    name: "Double Click".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::FastLoadout,
                    name: "Fast Loadout".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::Bhop,
                    name: "Bhop".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::HoldItemBug,
                    name: "Hold Item Bug".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::GangstaGrip,
                    name: "Gangsta Grip".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::QuickExit,
                    name: "Quick Exit".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::ToggleAllMacros,
                    name: "Toggle All Macros".into(),
                    bind_key: None,
                    enabled: false,
                    selecting: false,
                },
            ],
            monitor_id: "1".into(),
            x_offset: 0,
            y_offset: 0,
            width: 0,
            height: 0,
            shift_held: false,
            auto_clicker_delay: 6,
            auto_clicker_method: ClickMethod::default(),
            auto_clicker_click_count: 1,
            auto_clicker_mode: AutoClickerMode::default(),
            auto_clicker_key: ConfigKey::KeyE,
            position_x: 0,
            position_y: 0,
            tips_skip_y_offset: 830,
            restart_y_offset: 486,
            hacking_y_offset: -140,
            hacking2_y_offset: -140,
            hacking_esc_y_offset: -140,
            gun_tool_digit: 1,
            dev_mode: false,
            presets: Vec::new(),
            selected_preset: String::new(),
            preset_name_input: String::new(),
            double_click_button: DoubleClickButton::Left,
        };
        state.update_screen_position();
        state.refresh_presets();
        if let Err(e) = state.load_config() {
            eprintln!("Failed to load config: {}", e);
        }
        state
    }

    pub fn update_screen_position(&mut self) {
        let mon_id: i32 = self.monitor_id.parse().unwrap_or(1);
        unsafe {
            let mut monitors: Vec<windows::Win32::Graphics::Gdi::MONITORINFO> = Vec::new();
            unsafe extern "system" fn enum_monitor_callback(
                h_monitor: windows::Win32::Graphics::Gdi::HMONITOR,
                _: windows::Win32::Graphics::Gdi::HDC,
                _: *mut windows::Win32::Foundation::RECT,
                dw_data: windows::Win32::Foundation::LPARAM,
            ) -> windows::Win32::Foundation::BOOL {
                unsafe {
                    let monitors =
                        &mut *(dw_data.0 as *mut Vec<windows::Win32::Graphics::Gdi::MONITORINFO>);
                    let mut info = windows::Win32::Graphics::Gdi::MONITORINFO {
                        cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>()
                            as u32,
                        ..Default::default()
                    };
                    if windows::Win32::Graphics::Gdi::GetMonitorInfoW(h_monitor, &mut info)
                        .as_bool()
                    {
                        monitors.push(info);
                    }
                    windows::Win32::Foundation::TRUE
                }
            }
            let _ = windows::Win32::Graphics::Gdi::EnumDisplayMonitors(
                None,
                None,
                Some(enum_monitor_callback),
                windows::Win32::Foundation::LPARAM(
                    &mut monitors as *mut Vec<windows::Win32::Graphics::Gdi::MONITORINFO> as isize,
                ),
            );
            if let Some(mon) = monitors.get((mon_id - 1).max(0) as usize) {
                self.x_offset = mon.rcMonitor.left;
                self.y_offset = mon.rcMonitor.top;
                self.width = mon.rcMonitor.right - mon.rcMonitor.left;
                self.height = mon.rcMonitor.bottom - mon.rcMonitor.top;
            }
        }
    }

    pub fn toggle_shift(&mut self) {
        self.shift_held = !self.shift_held;
        crate::worker::send_key_state(0x2A, self.shift_held);
    }

    pub fn release_shift(&mut self) {
        if self.shift_held {
            self.shift_held = false;
            crate::worker::send_key_state(0x2A, false);
        }
    }

    pub fn reset_to_defaults(&mut self) {
        self.auto_clicker_delay = 6;
        self.auto_clicker_method = ClickMethod::default();
        self.auto_clicker_click_count = 1;
        self.auto_clicker_mode = AutoClickerMode::default();
        self.auto_clicker_key = ConfigKey::KeyE;
        self.position_x = 0;
        self.position_y = 0;
        self.tips_skip_y_offset = 830;
        self.restart_y_offset = 486;
        self.hacking_y_offset = -140;
        self.hacking2_y_offset = -140;
        self.gun_tool_digit = 1;
        self.dev_mode = false;
        self.selected_preset = String::new();
        self.preset_name_input = String::new();
        self.monitor_id = "1".to_string();
        self.double_click_button = DoubleClickButton::Left;
        self.update_screen_position();
        self.refresh_presets();
    }
}
