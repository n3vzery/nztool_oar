#![windows_subsystem = "windows"]

use eframe::egui;
use log::{error, warn};
use rdev::{grab, Event, EventType, Key};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// Global input state - replaces static variables
// Kept as statics because Windows hooks require them (hooks run in system context)
static REAL_LMB_DOWN: AtomicBool = AtomicBool::new(false);
static REAL_SPACE_DOWN: AtomicBool = AtomicBool::new(false);
static AUTOCLICKER_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE_HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static KEYBOARD_HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static BHOP_ACTIVE: AtomicBool = AtomicBool::new(false);
static REAL_CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static REAL_SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static REAL_ALT_DOWN: AtomicBool = AtomicBool::new(false);
static REAL_CAPSLOCK_DOWN: AtomicBool = AtomicBool::new(false);

// Focus cache to reduce WinAPI calls
static LAST_FOCUS_CHECK: AtomicU64 = AtomicU64::new(0);
static CACHED_FOCUS_VALUE: AtomicBool = AtomicBool::new(false);

// Graceful shutdown flag for rdev::grab thread
static RDEV_SHUTDOWN: AtomicBool = AtomicBool::new(false);

// Worker thread messages for feature execution
enum WorkerMessage {
    ShiftToggle,
    CtrlToggle,
    AltToggle,
    NoFallDamage,
    HackingPostMessage {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        offset_y: i32,
    },
    HackingPostMessage2 {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        offset_y: i32,
    },
    TipsSkip {
        x: i32,
        w: i32,
        y: i32,
    },
    Restart {
        x: i32,
        w: i32,
        y: i32,
    },
    GrabNoGun,
    HoldItemBug,
}

// InputState provides a clean API over the static variables
#[derive(Clone)]
struct InputState;

impl InputState {
    fn is_lmb_down(&self) -> bool {
        REAL_LMB_DOWN.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn set_lmb_down(&self, down: bool) {
        REAL_LMB_DOWN.store(down, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    fn set_space_down(&self, down: bool) {
        REAL_SPACE_DOWN.store(down, Ordering::SeqCst);
    }

    fn is_space_down(&self) -> bool {
        REAL_SPACE_DOWN.load(Ordering::SeqCst)
    }

    fn toggle_autoclicker(&self) -> bool {
        let current = AUTOCLICKER_ACTIVE.load(Ordering::SeqCst);
        AUTOCLICKER_ACTIVE.store(!current, Ordering::SeqCst);
        !current
    }

    fn is_autoclicker_active(&self) -> bool {
        AUTOCLICKER_ACTIVE.load(Ordering::SeqCst)
    }

    fn toggle_bhop(&self) -> bool {
        let current = BHOP_ACTIVE.load(Ordering::SeqCst);
        BHOP_ACTIVE.store(!current, Ordering::SeqCst);
        !current
    }

    fn is_bhop_active(&self) -> bool {
        BHOP_ACTIVE.load(Ordering::SeqCst)
    }

    fn is_ctrl_down(&self) -> bool {
        REAL_CTRL_DOWN.load(Ordering::SeqCst)
    }

    fn is_shift_down(&self) -> bool {
        REAL_SHIFT_DOWN.load(Ordering::SeqCst)
    }

    fn is_alt_down(&self) -> bool {
        REAL_ALT_DOWN.load(Ordering::SeqCst)
    }

    fn is_capslock_down(&self) -> bool {
        REAL_CAPSLOCK_DOWN.load(Ordering::SeqCst)
    }

    fn set_mouse_hook_thread_id(&self, id: u32) {
        MOUSE_HOOK_THREAD_ID.store(id, Ordering::SeqCst);
    }

    fn get_mouse_hook_thread_id(&self) -> u32 {
        MOUSE_HOOK_THREAD_ID.load(Ordering::SeqCst)
    }

    fn set_keyboard_hook_thread_id(&self, id: u32) {
        KEYBOARD_HOOK_THREAD_ID.store(id, Ordering::SeqCst);
    }

    fn get_keyboard_hook_thread_id(&self) -> u32 {
        KEYBOARD_HOOK_THREAD_ID.load(Ordering::SeqCst)
    }
}

// --- KEY MAPPING MACRO ---
macro_rules! define_keys {
    ($name:ident { $($variant:ident => $rdev:ident),* $(,)? }) => {
        #[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
        pub enum $name {
            $($variant),*
        }

        impl $name {
            pub fn to_rdev(&self) -> Key {
                match self {
                    $(Self::$variant => Key::$rdev),*
                }
            }

            pub fn from_rdev(key: Key) -> Option<Self> {
                match key {
                    $(Key::$rdev => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:?}", self)
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    _ => Err(format!("Unknown key: {}", s)),
                }
            }
        }
    };
}

// --- ENUM SERIALIZATION MACRO ---
macro_rules! define_enum {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
        pub enum $name {
            $($variant),*
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:?}", self)
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    _ => Err(format!("Unknown variant: {}", s)),
                }
            }
        }
    };
}

define_keys!(ConfigKey {
    KeyA => KeyA, KeyB => KeyB, KeyC => KeyC, KeyD => KeyD, KeyE => KeyE,
    KeyF => KeyF, KeyG => KeyG, KeyH => KeyH, KeyI => KeyI, KeyJ => KeyJ,
    KeyK => KeyK, KeyL => KeyL, KeyM => KeyM, KeyN => KeyN, KeyO => KeyO,
    KeyP => KeyP, KeyQ => KeyQ, KeyR => KeyR, KeyS => KeyS, KeyT => KeyT,
    KeyU => KeyU, KeyV => KeyV, KeyW => KeyW, KeyX => KeyX, KeyY => KeyY,
    KeyZ => KeyZ, Num0 => Num0, Num1 => Num1, Num2 => Num2, Num3 => Num3,
    Num4 => Num4, Num5 => Num5, Num6 => Num6, Num7 => Num7, Num8 => Num8,
    Num9 => Num9, F1 => F1, F2 => F2, F3 => F3, F4 => F4, F5 => F5,
    F6 => F6, F7 => F7, F8 => F8, F9 => F9, F10 => F10, F11 => F11,
    F12 => F12, Space => Space, Return => Return, Escape => Escape,
    Tab => Tab, Backspace => Backspace, Insert => Insert, Delete => Delete,
    Home => Home, End => End, PageUp => PageUp, PageDown => PageDown,
    UpArrow => UpArrow, DownArrow => DownArrow, LeftArrow => LeftArrow,
    RightArrow => RightArrow, Alt => Alt, ControlLeft => ControlLeft,
    ControlRight => ControlRight, ShiftLeft => ShiftLeft, ShiftRight => ShiftRight,
    MetaLeft => MetaLeft, MetaRight => MetaRight, CapsLock => CapsLock,
    NumLock => NumLock, ScrollLock => ScrollLock,
});

// Modifier key type for UI binding
#[derive(Clone, Copy, PartialEq, Debug)]
enum KeyModifier {
    Ctrl,
    Shift,
    Alt,
    CapsLock,
}

// Helper to check if the game window is currently focused
fn is_game_focused() -> bool {
    // Cache with 100ms TTL to reduce WinAPI calls
    const FOCUS_CACHE_TTL_MS: u64 = 100;

    let now = {
        // Get current time in milliseconds using WinAPI
        let elapsed = unsafe { GetTickCount64() };
        elapsed
    };

    let last = LAST_FOCUS_CHECK.load(Ordering::Relaxed);
    if now.saturating_sub(last) < FOCUS_CACHE_TTL_MS {
        return CACHED_FOCUS_VALUE.load(Ordering::Relaxed);
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

    CACHED_FOCUS_VALUE.store(result, Ordering::Relaxed);
    LAST_FOCUS_CHECK.store(now, Ordering::Relaxed);
    result
}

// --- CONSTANTS ---
const HACKING_DELAY_MS: u64 = 50;
const MOUSE_CLICK_PRE_DELAY_MS: u64 = 10;
const HOLD_ITEM_TAP_DELAY_MS: u64 = 7;
const RESTART_KEY_DELAY_MS: u64 = 100;
const NO_FALL_DAMAGE_DELAY_MS: u64 = 30;
const BHOP_TAP_INTERVAL_MS: u64 = 15;
const POLL_INTERVAL_MS: u64 = 5;
const AUTO_CLICKER_MIN_DELAY_MS: u32 = 1;
const AUTO_CLICKER_MAX_DELAY_MS: u32 = 50;

// Helper to pack coordinates into LPARAM safely for PostMessage
// Handles negative coordinates by casting through i16 (16-bit signed)
fn pack_lparam(x: i32, y: i32) -> isize {
    let low = ((x as i16) as u16 as u32) as isize;
    let high = (((y as i16) as u16 as u32) << 16) as isize;
    high | low
}

unsafe extern "system" fn low_level_mouse_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code == HC_ACTION as i32 {
        unsafe {
            let ms_ll = *(l_param.0 as *const MSLLHOOKSTRUCT);

            // LLMHF_INJECTED (0x01) means the event was generated programmatically (SendInput).
            // We ignore such events to prevent self-triggering.
            if (ms_ll.flags & LLMHF_INJECTED) == 0 {
                if w_param.0 as u32 == WM_LBUTTONDOWN {
                    REAL_LMB_DOWN.store(true, Ordering::SeqCst);
                } else if w_param.0 as u32 == WM_LBUTTONUP {
                    REAL_LMB_DOWN.store(false, Ordering::SeqCst);
                }
            }
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), n_code, w_param, l_param) }
}

unsafe extern "system" fn low_level_keyboard_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code == HC_ACTION as i32 {
        unsafe {
            let kb_ll = *(l_param.0 as *const KBDLLHOOKSTRUCT);

            // LLKHF_INJECTED (0x10) means the event was generated programmatically.
            // We ignore such events to prevent self-triggering.
            if !kb_ll.flags.contains(LLKHF_INJECTED) {
                let is_key_down =
                    w_param.0 as u32 == WM_KEYDOWN || w_param.0 as u32 == WM_SYSKEYDOWN;
                let is_key_up = w_param.0 as u32 == WM_KEYUP || w_param.0 as u32 == WM_SYSKEYUP;

                // VK_SPACE = 0x20
                if kb_ll.vkCode == 0x20 {
                    if is_key_down {
                        REAL_SPACE_DOWN.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        REAL_SPACE_DOWN.store(false, Ordering::SeqCst);
                    }
                }

                // VK_SHIFT = 0x10, VK_LSHIFT = 0xA0, VK_RSHIFT = 0xA1
                if kb_ll.vkCode == 0x10 || kb_ll.vkCode == 0xA0 || kb_ll.vkCode == 0xA1 {
                    if is_key_down {
                        REAL_SHIFT_DOWN.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        REAL_SHIFT_DOWN.store(false, Ordering::SeqCst);
                    }
                }

                // VK_CONTROL = 0x11, VK_LCONTROL = 0xA2, VK_RCONTROL = 0xA3
                if kb_ll.vkCode == 0x11 || kb_ll.vkCode == 0xA2 || kb_ll.vkCode == 0xA3 {
                    if is_key_down {
                        REAL_CTRL_DOWN.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        REAL_CTRL_DOWN.store(false, Ordering::SeqCst);
                    }
                }

                // VK_MENU = 0x12, VK_LMENU = 0xA4, VK_RMENU = 0xA5
                if kb_ll.vkCode == 0x12 || kb_ll.vkCode == 0xA4 || kb_ll.vkCode == 0xA5 {
                    if is_key_down {
                        REAL_ALT_DOWN.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        REAL_ALT_DOWN.store(false, Ordering::SeqCst);
                    }
                }

                // VK_CAPITAL = 0x14
                if kb_ll.vkCode == 0x14 {
                    if is_key_down {
                        REAL_CAPSLOCK_DOWN.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        REAL_CAPSLOCK_DOWN.store(false, Ordering::SeqCst);
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), n_code, w_param, l_param) }
}

define_enum!(FeatureId {
    HackingPostMessage,
    HackingPostMessage2,
    TipsSkip,
    Restart,
    NoFallDamage,
    ShiftToggle,
    CtrlToggle,
    AltToggle,
    AutoClicker,
    GrabNoGun,
    Bhop,
    HoldItemBug,
});

// Serializable structure for config file
#[derive(Serialize, Deserialize, Clone)]
struct SerializableConfig {
    monitor_id: String,
    features: Vec<SerializableFeature>,
    auto_clicker_delay: u32,
    position_x: i32,
    position_y: i32,
    #[serde(default = "default_tips_skip_y")]
    tips_skip_y_offset: i32,
    #[serde(default = "default_restart_y")]
    restart_y_offset: i32,
    #[serde(default = "default_hacking_y")]
    hacking_y_offset: i32,
    #[serde(default = "default_hacking2_y")]
    hacking2_y_offset: i32,
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

#[derive(Serialize, Deserialize, Clone)]
struct SerializableFeature {
    id: FeatureId,
    rdev_key: Option<String>,
    enabled: bool,
}

struct Feature {
    id: FeatureId,
    name: String,
    rdev_key: Option<Key>,
    enabled: bool,
    selecting: bool,
}

struct AppState {
    features: Vec<Feature>,
    monitor_id: String,
    x_offset: i32,
    y_offset: i32,
    width: i32,
    height: i32,
    shift_held: bool,
    ctrl_held: bool,
    alt_held: bool,
    auto_clicker_delay: u32,
    position_x: i32,
    position_y: i32,
    tips_skip_y_offset: i32,
    restart_y_offset: i32,
    hacking_y_offset: i32,
    hacking2_y_offset: i32,
}

// Get the path to the config directory and file
fn get_config_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let config_dir = std::path::PathBuf::from(appdata).join("nzconfig");
    let config_file = config_dir.join("config.json");
    config_file
}

// Convert rdev::Key to ConfigKey then to string
fn key_to_string(key: Key) -> String {
    ConfigKey::from_rdev(key)
        .map(|k| k.to_string())
        .unwrap_or_else(|| format!("{:?}", key))
}

// Convert string to ConfigKey then to rdev::Key
fn string_to_key(s: &str) -> Option<Key> {
    s.parse::<ConfigKey>().ok().map(|k| k.to_rdev())
}

impl AppState {
    // Save configuration to JSON file
    fn save_config(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = get_config_path();

        // Create directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create serializable config
        let features: Vec<SerializableFeature> = self
            .features
            .iter()
            .map(|f| SerializableFeature {
                id: f.id,
                rdev_key: f.rdev_key.as_ref().map(|k| key_to_string(*k)),
                enabled: f.enabled,
            })
            .collect();

        let config = SerializableConfig {
            monitor_id: self.monitor_id.clone(),
            features,
            auto_clicker_delay: self.auto_clicker_delay,
            position_x: self.position_x,
            position_y: self.position_y,
            tips_skip_y_offset: self.tips_skip_y_offset,
            restart_y_offset: self.restart_y_offset,
            hacking_y_offset: self.hacking_y_offset,
            hacking2_y_offset: self.hacking2_y_offset,
        };

        // Write to file with pretty formatting
        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        Ok(())
    }

    // Load configuration from JSON file
    fn load_config(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = get_config_path();

        // Create directory if it doesn't exist (in case config file doesn't exist yet)
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Return Ok if file doesn't exist (use defaults)
        if !config_path.exists() {
            return Ok(());
        }

        // Read and parse JSON
        let json = std::fs::read_to_string(&config_path)?;
        let config: SerializableConfig = serde_json::from_str(&json)?;

        // Load monitor_id
        self.monitor_id = config.monitor_id;

        // Load features
        for sf in config.features {
            if let Some(feature) = self.features.iter_mut().find(|f| f.id == sf.id) {
                feature.rdev_key = sf.rdev_key.as_ref().and_then(|k| string_to_key(k));
                feature.enabled = sf.enabled;
            }
        }

        // Load auto_clicker_delay and position values
        self.auto_clicker_delay = config.auto_clicker_delay;
        self.position_x = config.position_x;
        self.position_y = config.position_y;
        self.tips_skip_y_offset = config.tips_skip_y_offset;
        self.restart_y_offset = config.restart_y_offset;
        self.hacking_y_offset = config.hacking_y_offset;
        self.hacking2_y_offset = config.hacking2_y_offset;

        Ok(())
    }
}

impl AppState {
    fn new() -> Self {
        let mut state = Self {
            features: vec![
                Feature {
                    id: FeatureId::HackingPostMessage,
                    name: "Hacking Device (PostMessage)".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::HackingPostMessage2,
                    name: "Hacking Device (PostMessage 2)".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::TipsSkip,
                    name: "Tips Skip".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::Restart,
                    name: "Restart".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::NoFallDamage,
                    name: "No Fall Damage".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::ShiftToggle,
                    name: "Shift Toggle".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::CtrlToggle,
                    name: "Ctrl Toggle".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::AltToggle,
                    name: "Alt Toggle".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::AutoClicker,
                    name: "Auto Clicker".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::GrabNoGun,
                    name: "Grab No Gun".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::Bhop,
                    name: "Bhop".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::HoldItemBug,
                    name: "Hold Item Bug".into(),
                    rdev_key: None,
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
            ctrl_held: false,
            alt_held: false,
            auto_clicker_delay: 6,
            position_x: 0,
            position_y: 0,
            tips_skip_y_offset: 830,
            restart_y_offset: 486,
            hacking_y_offset: -140,
            hacking2_y_offset: -140,
        };
        state.update_screen_position();

        // Load config from file if it exists
        if let Err(e) = Self::load_config(&mut state) {
            eprintln!("Failed to load config: {}", e);
        }

        state
    }

    fn update_screen_position(&mut self) {
        let mon_id: i32 = self.monitor_id.parse().unwrap_or(1);
        unsafe {
            let mut monitors: Vec<MONITORINFO> = Vec::new();
            unsafe extern "system" fn enum_monitor_callback(
                h_monitor: HMONITOR,
                _: HDC,
                _: *mut RECT,
                dw_data: LPARAM,
            ) -> BOOL {
                unsafe {
                    let monitors = &mut *(dw_data.0 as *mut Vec<MONITORINFO>);
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if GetMonitorInfoW(h_monitor, &mut info).as_bool() {
                        monitors.push(info);
                    }
                    TRUE
                }
            }
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(enum_monitor_callback),
                LPARAM(&mut monitors as *mut Vec<MONITORINFO> as isize),
            );
            if let Some(mon) = monitors.get((mon_id - 1).max(0) as usize) {
                self.x_offset = mon.rcMonitor.left;
                self.y_offset = mon.rcMonitor.top;
                self.width = mon.rcMonitor.right - mon.rcMonitor.left;
                self.height = mon.rcMonitor.bottom - mon.rcMonitor.top;
            }
        }
    }

    fn toggle_shift(&mut self) {
        self.shift_held = !self.shift_held;
        send_key_state(0x2A, self.shift_held);
    }

    fn release_shift(&mut self) {
        if self.shift_held {
            self.shift_held = false;
            send_key_state(0x2A, false);
        }
    }

    fn toggle_ctrl(&mut self) {
        self.ctrl_held = !self.ctrl_held;
        send_key_state(0x1D, self.ctrl_held);
    }

    #[allow(dead_code)]
    fn release_ctrl(&mut self) {
        if self.ctrl_held {
            self.ctrl_held = false;
            send_key_state(0x1D, false);
        }
    }

    fn toggle_alt(&mut self) {
        self.alt_held = !self.alt_held;
        send_key_state(0x38, self.alt_held);
    }

    #[allow(dead_code)]
    fn release_alt(&mut self) {
        if self.alt_held {
            self.alt_held = false;
            send_key_state(0x38, false);
        }
    }

    fn reset_to_defaults(&mut self) {
        self.auto_clicker_delay = 6;
        self.position_x = 0;
        self.position_y = 0;
        self.tips_skip_y_offset = 830;
        self.restart_y_offset = 486;
        self.hacking_y_offset = -140;
        self.hacking2_y_offset = -140;
        self.monitor_id = "1".to_string();
        self.update_screen_position();
    }
}

struct KeyBindApp {
    state: Arc<Mutex<AppState>>,
    prev_ctrl: bool,
    prev_shift: bool,
    prev_alt: bool,
    prev_capslock: bool,
}

impl Drop for KeyBindApp {
    fn drop(&mut self) {
        // Signal rdev::grab thread to shut down gracefully
        RDEV_SHUTDOWN.store(true, Ordering::SeqCst);

        let mouse_thread_id = InputState.get_mouse_hook_thread_id();
        if mouse_thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageA(mouse_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        let keyboard_thread_id = InputState.get_keyboard_hook_thread_id();
        if keyboard_thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageA(keyboard_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl KeyBindApp {
    fn start_hotkey_listener(state: Arc<Mutex<AppState>>, input_state: InputState) {
        let input_mouse = input_state.clone();
        let input_keyboard = input_state.clone();
        let input_bhop = input_state.clone();
        let input_ac = input_state.clone();
        let input_hotkey = input_state.clone();
        let input_mod_poll = input_state.clone();

        // Create worker channel early so all threads can access it
        let (worker_tx, worker_rx) = mpsc::channel::<WorkerMessage>();
        let worker_tx_mod = worker_tx.clone();
        let worker_tx_hk = worker_tx.clone();

        // Start the real mouse listener in a separate thread with a message loop
        thread::spawn(move || {
            unsafe {
                // Store current thread ID for clean shutdown
                input_mouse.set_mouse_hook_thread_id(GetCurrentThreadId());

                let hook = match SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(low_level_mouse_proc),
                    HINSTANCE::default(),
                    0,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        error!("SetWindowsHookExW failed for mouse hook: {:?}", e);
                        return;
                    }
                };
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if UnhookWindowsHookEx(hook).is_err() {
                    error!("UnhookWindowsHookEx failed for mouse hook");
                }
            }
        });

        // Start the keyboard hook in a separate thread with a message loop
        thread::spawn(move || {
            unsafe {
                // Store current thread ID for clean shutdown
                input_keyboard.set_keyboard_hook_thread_id(GetCurrentThreadId());

                let hook = match SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(low_level_keyboard_proc),
                    HINSTANCE::default(),
                    0,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        error!("SetWindowsHookExW failed for keyboard hook: {:?}", e);
                        return;
                    }
                };
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if UnhookWindowsHookEx(hook).is_err() {
                    error!("UnhookWindowsHookEx failed for keyboard hook");
                }
            }
        });

        // Bhop worker thread - sends Space taps when Space is physically held
        thread::spawn(move || {
            loop {
                let bhop_enabled = input_bhop.is_bhop_active();

                if bhop_enabled && is_game_focused() {
                    if input_bhop.is_space_down() {
                        send_key_tap(0x39); // Space key
                        thread::sleep(Duration::from_millis(BHOP_TAP_INTERVAL_MS));
                    } else {
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                    }
                } else {
                    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                }
            }
        });

        // Modifier key polling thread - triggers hotkeys for Ctrl/Shift/Alt
        // rdev::grab doesn't capture modifier keys, so we poll via Windows hook state
        let state_clone_mod = state.clone();
        thread::spawn(move || {
            let mut prev_ctrl = false;
            let mut prev_shift = false;
            let mut prev_alt = false;
            let prev_capslock = false;
            loop {
                let ctrl_down = input_mod_poll.is_ctrl_down();
                let shift_down = input_mod_poll.is_shift_down();
                let alt_down = input_mod_poll.is_alt_down();
                let capslock_down = input_mod_poll.is_capslock_down();

                if is_game_focused() {
                    let s = state_clone_mod.lock().unwrap();
                    if ctrl_down && !prev_ctrl {
                        if let Some(f) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.rdev_key == Some(Key::ControlLeft))
                        {
                            if f.id == FeatureId::CtrlToggle {
                                let _ = worker_tx_mod.send(WorkerMessage::CtrlToggle);
                            }
                        }
                    }
                    if shift_down && !prev_shift {
                        if let Some(f) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.rdev_key == Some(Key::ShiftLeft))
                        {
                            if f.id == FeatureId::ShiftToggle {
                                let _ = worker_tx_mod.send(WorkerMessage::ShiftToggle);
                            }
                        }
                    }
                    if alt_down && !prev_alt {
                        if let Some(f) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.rdev_key == Some(Key::Alt))
                        {
                            if f.id == FeatureId::AltToggle {
                                let _ = worker_tx_mod.send(WorkerMessage::AltToggle);
                            }
                        }
                    }
                    if capslock_down && !prev_capslock {
                        if let Some(f) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.rdev_key == Some(Key::CapsLock))
                        {
                            if f.id == FeatureId::ShiftToggle {
                                let _ = worker_tx_mod.send(WorkerMessage::ShiftToggle);
                            } else if f.id == FeatureId::CtrlToggle {
                                let _ = worker_tx_mod.send(WorkerMessage::CtrlToggle);
                            } else if f.id == FeatureId::AltToggle {
                                let _ = worker_tx_mod.send(WorkerMessage::AltToggle);
                            }
                        }
                    }
                }

                prev_ctrl = ctrl_down;
                prev_shift = shift_down;
                prev_alt = alt_down;
                thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
        });

        let state_clone_ac = state.clone();
        thread::spawn(move || {
            loop {
                // Read the global autoclicker mode and check if the feature is enabled in UI
                let (enabled, delay_ms) = {
                    let s = state_clone_ac.lock().unwrap();
                    let enabled = s
                        .features
                        .iter()
                        .any(|f| f.id == FeatureId::AutoClicker && f.enabled);
                    (enabled, s.auto_clicker_delay)
                };
                let active = input_ac.is_autoclicker_active();

                if active && enabled && is_game_focused() {
                    if input_ac.is_lmb_down() {
                        send_mouse_click();
                        thread::sleep(Duration::from_millis(delay_ms as u64));
                    } else {
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
                    }
                } else {
                    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
                }
            }
        });

        // Worker thread for feature execution
        let worker_state = state.clone();
        thread::spawn(move || {
            while let Ok(msg) = worker_rx.recv() {
                match msg {
                    WorkerMessage::ShiftToggle => {
                        worker_state.lock().unwrap().toggle_shift();
                    }
                    WorkerMessage::CtrlToggle => {
                        worker_state.lock().unwrap().toggle_ctrl();
                    }
                    WorkerMessage::AltToggle => {
                        worker_state.lock().unwrap().toggle_alt();
                    }
                    WorkerMessage::NoFallDamage => {
                        Self::no_fall_damage();
                    }
                    WorkerMessage::HackingPostMessage {
                        x,
                        y,
                        w,
                        h,
                        offset_y,
                    } => {
                        Self::hacking_method_post_message(x, y, w, h, offset_y);
                    }
                    WorkerMessage::HackingPostMessage2 {
                        x,
                        y,
                        w,
                        h,
                        offset_y,
                    } => {
                        Self::hacking_method2(x, y, w, h, offset_y);
                    }
                    WorkerMessage::TipsSkip { x, w, y } => {
                        Self::tips_skip(x, w, y);
                    }
                    WorkerMessage::Restart { x, w, y } => {
                        Self::restart(x, w, y);
                    }
                    WorkerMessage::GrabNoGun => {
                        Self::grab_no_gun();
                    }
                    WorkerMessage::HoldItemBug => {
                        Self::hold_item_bug();
                    }
                }
            }
        });

        let state_clone_hk = state.clone();
        thread::spawn(move || {
            let callback = move |event: Event| {
                // Gracefully shut down if requested
                if RDEV_SHUTDOWN.load(Ordering::SeqCst) {
                    return None;
                }

                // If it's a mouse event, just let it pass through to the system
                if let EventType::ButtonPress(_)
                | EventType::ButtonRelease(_)
                | EventType::MouseMove { .. }
                | EventType::Wheel { .. } = event.event_type
                {
                    return Some(event);
                }

                // Check if game is focused before handling hotkeys
                if !is_game_focused() {
                    return Some(event);
                }

                // Minimal lock scope - copy only needed data
                let (feature_action, _should_block) = {
                    let s = state_clone_hk.lock().unwrap();

                    if let EventType::KeyPress(key) = event.event_type {
                        if let Some(feature) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.rdev_key == Some(key))
                        {
                            let feature_id = feature.id;
                            let x = s.x_offset;
                            let y = s.y_offset;
                            let w = s.width;
                            let h = s.height;
                            let tips_y = s.tips_skip_y_offset;
                            let restart_y = s.restart_y_offset;
                            let hack_y = s.hacking_y_offset;
                            let hack2_y = s.hacking2_y_offset;

                            // Handle toggle features immediately
                            if feature_id == FeatureId::AutoClicker {
                                let _ = input_hotkey.toggle_autoclicker();
                                return None;
                            }

                            if feature_id == FeatureId::Bhop {
                                let _ = input_hotkey.toggle_bhop();
                                return None;
                            }

                            // Build worker message for other features
                            let action = match feature_id {
                                FeatureId::ShiftToggle => Some(WorkerMessage::ShiftToggle),
                                FeatureId::CtrlToggle => Some(WorkerMessage::CtrlToggle),
                                FeatureId::AltToggle => Some(WorkerMessage::AltToggle),
                                FeatureId::NoFallDamage => Some(WorkerMessage::NoFallDamage),
                                FeatureId::HackingPostMessage => {
                                    Some(WorkerMessage::HackingPostMessage {
                                        x,
                                        y,
                                        w,
                                        h,
                                        offset_y: hack_y,
                                    })
                                }
                                FeatureId::HackingPostMessage2 => {
                                    Some(WorkerMessage::HackingPostMessage2 {
                                        x,
                                        y,
                                        w,
                                        h,
                                        offset_y: hack2_y,
                                    })
                                }
                                FeatureId::TipsSkip => Some(WorkerMessage::TipsSkip {
                                    x,
                                    w,
                                    y: y + tips_y,
                                }),
                                FeatureId::Restart => Some(WorkerMessage::Restart {
                                    x,
                                    w,
                                    y: y + restart_y,
                                }),
                                FeatureId::GrabNoGun => Some(WorkerMessage::GrabNoGun),
                                FeatureId::HoldItemBug => Some(WorkerMessage::HoldItemBug),
                                _ => None,
                            };

                            (action, true)
                        } else {
                            (None, false)
                        }
                    } else {
                        (None, false)
                    }
                }; // Lock released here

                // Send to worker thread without holding lock
                if let Some(action) = feature_action {
                    let _ = worker_tx_hk.send(action);
                    return None;
                }

                Some(event)
            };

            if let Err(error) = grab(callback) {
                eprintln!("Error in hotkey listener: {:?}", error);
            }
        });
    }

    fn hacking_method_post_message(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
        unsafe {
            // 1. Wait
            thread::sleep(Duration::from_millis(HACKING_DELAY_MS));

            // 2. Get game window
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }

            // 3. Screen center calculation
            let center_x = x + w / 2;
            let center_y = y + h / 2;
            let target_x = center_x;
            let target_y = center_y + offset_y;

            // 4. Convert Screen to Client Coordinates for PostMessage
            let mut pt = POINT {
                x: target_x,
                y: target_y,
            };
            if !ScreenToClient(hwnd, &mut pt).as_bool() {
                warn!("ScreenToClient failed for hacking method");
            }

            // 5. Instant Move (Still screen-relative)
            if SetCursorPos(target_x, target_y).is_err() {
                warn!("SetCursorPos failed in hacking method");
            }

            // 6. Direct Message Blast (Packed safely)
            let l_param = pack_lparam(pt.x, pt.y);

            // CLICK_COUNT = 100, MK_LBUTTON = 0x0001
            // Added yield_now to prevent message queue overflow while maintaining high speed
            let mut failed_count = 0;
            for _ in 0..100 {
                if PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), LPARAM(l_param)).is_err() {
                    failed_count += 1;
                }
                if PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(l_param)).is_err() {
                    failed_count += 1;
                }
                thread::yield_now();
            }
            if failed_count > 0 {
                warn!(
                    "PostMessageA failed {} times in hacking method",
                    failed_count
                );
            }

            // 7. Move to top and click once
            if SetCursorPos(center_x, y).is_err() {
                warn!("SetCursorPos failed after hacking click");
            }
            send_mouse_click();
        }
    }

    fn hacking_method2(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
        unsafe {
            // 1. Wait
            thread::sleep(Duration::from_millis(HACKING_DELAY_MS));

            // 2. Get game window
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }

            // 3. Screen center calculation
            let center_x = x + w / 2;
            let center_y = y + h / 2;
            let target_x = center_x;
            let target_y = center_y + offset_y;

            // 4. Convert Screen to Client Coordinates
            let mut pt = POINT {
                x: target_x,
                y: target_y,
            };
            if !ScreenToClient(hwnd, &mut pt).as_bool() {
                warn!("ScreenToClient failed for hacking method 2");
            }

            // 5. Instant Move
            if SetCursorPos(target_x, target_y).is_err() {
                warn!("SetCursorPos failed in hacking method 2");
            }

            // 6. Direct Message Blast
            let l_param = pack_lparam(pt.x, pt.y);

            // CLICK_COUNT = 100, MK_LBUTTON = 0x0001
            // Added yield_now to prevent message queue overflow while maintaining high speed
            let mut failed_count = 0;
            for _ in 0..100 {
                if PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), LPARAM(l_param)).is_err() {
                    failed_count += 1;
                }
                if PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(l_param)).is_err() {
                    failed_count += 1;
                }
                thread::yield_now();
            }
            if failed_count > 0 {
                warn!(
                    "PostMessageA failed {} times in hacking method 2",
                    failed_count
                );
            }

            // 7. Jump (Space scancode = 0x39)
            send_key_tap(0x39);
        }
    }

    fn tips_skip(x: i32, w: i32, y_abs: i32) {
        move_mouse(x + w / 2, y_abs);
        send_mouse_click();
    }

    fn restart(x: i32, w: i32, y_abs: i32) {
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(RESTART_KEY_DELAY_MS));
        move_mouse(x + w / 2, y_abs);
        send_mouse_click();
    }

    fn no_fall_damage() {
        send_key_tap(0x01); // ESC
        thread::sleep(Duration::from_millis(NO_FALL_DAMAGE_DELAY_MS));
        send_key_tap(0x01); // ESC
    }

    fn grab_no_gun() {
        unsafe {
            // Scroll wheel up (WHEEL_DELTA = 120)
            let mut wheel_input = INPUT::default();
            wheel_input.r#type = INPUT_MOUSE;
            wheel_input.Anonymous.mi.dwFlags = MOUSEEVENTF_WHEEL;
            wheel_input.Anonymous.mi.mouseData = 120;
            if SendInput(&[wheel_input], std::mem::size_of::<INPUT>() as i32) == 0 {
                error!("SendInput failed in grab_no_gun (wheel)");
            }

            // Small delay before mouse click
            thread::sleep(Duration::from_millis(MOUSE_CLICK_PRE_DELAY_MS));

            // Single left mouse click
            send_mouse_click();
        }
    }

    fn hold_item_bug() {
        Self::send_mouse_state(true);
        thread::sleep(Duration::from_millis(HOLD_ITEM_TAP_DELAY_MS));
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(HOLD_ITEM_TAP_DELAY_MS));
        Self::send_mouse_state(false);
        thread::sleep(Duration::from_millis(HOLD_ITEM_TAP_DELAY_MS));
        send_key_tap(0x01);
    }

    fn send_mouse_state(down: bool) {
        unsafe {
            let mut input = INPUT::default();
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi.dwFlags = if down {
                MOUSEEVENTF_LEFTDOWN
            } else {
                MOUSEEVENTF_LEFTUP
            };
            if SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 0 {
                error!("SendInput failed in send_mouse_state (down: {})", down);
            }
        }
    }
}

impl eframe::App for KeyBindApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let mut s = self.state.lock().unwrap();
        if let Some(idx) = s.features.iter().position(|f| f.selecting) {
            // Request continuous repaint with short interval while selecting to catch modifier key state changes
            ctx.request_repaint_after(Duration::from_millis(16)); // ~60 FPS

            // Check for regular key press events (without modifiers)
            let key = ctx.input(|i| {
                for event in &i.events {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    {
                        // Only accept regular keys when no modifiers are pressed
                        if !modifiers.ctrl && !modifiers.shift && !modifiers.alt {
                            if *key == egui::Key::Escape || egui_to_rdev_key(*key).is_some() {
                                return Some(*key);
                            }
                        }
                    }
                }
                None
            });

            // Check for modifier key presses using direct Windows API (GetAsyncKeyState)
            // This works even when nztool window is focused, unlike hooks which may be blocked by egui
            let ctrl_down =
                unsafe { (GetAsyncKeyState(VK_CONTROL.0 as i32) & 0x8000u16 as i16) != 0 };
            let shift_down =
                unsafe { (GetAsyncKeyState(VK_SHIFT.0 as i32) & 0x8000u16 as i16) != 0 };
            let alt_down = unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) & 0x8000u16 as i16) != 0 };
            let capslock_down =
                unsafe { (GetAsyncKeyState(VK_CAPITAL.0 as i32) & 0x8000u16 as i16) != 0 };

            let modifier_key = if ctrl_down && !self.prev_ctrl {
                Some(KeyModifier::Ctrl)
            } else if shift_down && !self.prev_shift {
                Some(KeyModifier::Shift)
            } else if alt_down && !self.prev_alt {
                Some(KeyModifier::Alt)
            } else if capslock_down && !self.prev_capslock {
                Some(KeyModifier::CapsLock)
            } else {
                None
            };

            self.prev_ctrl = ctrl_down;
            self.prev_shift = shift_down;
            self.prev_alt = alt_down;
            self.prev_capslock = capslock_down;

            if let Some(k) = key {
                if k == egui::Key::Escape {
                    s.features[idx].selecting = false;
                } else {
                    let rd_key = egui_to_rdev_key(k);
                    if rd_key.is_some() {
                        s.features[idx].rdev_key = rd_key;
                        s.features[idx].selecting = false;
                        let _ = s.save_config();
                    }
                }
            } else if let Some(modifier) = modifier_key {
                let rd_key = match modifier {
                    KeyModifier::Ctrl => Some(Key::ControlLeft),
                    KeyModifier::Shift => Some(Key::ShiftLeft),
                    KeyModifier::Alt => Some(Key::Alt),
                    KeyModifier::CapsLock => Some(Key::CapsLock),
                };
                if rd_key.is_some() {
                    s.features[idx].rdev_key = rd_key;
                    s.features[idx].selecting = false;
                    let _ = s.save_config();
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Nztool OAR (Rust edition)");
            ui.add_space(10.0);

            egui::Grid::new("features_grid")
                .num_columns(4)
                .spacing([20.0, 8.0])
                .show(ui, |ui| {
                    for i in 0..s.features.len() {
                        // 1. Feature Name
                        ui.label(format!("{}:", s.features[i].name));

                        // 2. Select Key Button
                        let key_text = if s.features[i].selecting {
                            "Waiting...".into()
                        } else if let Some(k) = s.features[i].rdev_key {
                            rdev_key_to_name(k)
                        } else {
                            "Select Key".into()
                        };

                        if ui.button(key_text).clicked() {
                            s.features[i].selecting = true;
                        }

                        // 3. Reset Button
                        if ui.button("Reset").clicked() {
                            if s.features[i].id == FeatureId::ShiftToggle {
                                s.release_shift();
                            }
                            s.features[i].rdev_key = None;
                            s.features[i].enabled = false;
                            s.features[i].selecting = false;
                            // Save config when reset
                            let _ = s.save_config();
                        }

                        // 4. Enable/Disable Button
                        let mut color = if s.features[i].enabled {
                            egui::Color32::from_rgb(0, 150, 0)
                        } else {
                            egui::Color32::from_rgb(150, 0, 0)
                        };
                        if s.features[i].id == FeatureId::ShiftToggle
                            && s.shift_held
                            && s.features[i].enabled
                        {
                            color = egui::Color32::BLUE;
                        }

                        if ui
                            .add(egui::Button::new("Enable/Disable").fill(color))
                            .clicked()
                        {
                            if s.features[i].rdev_key.is_some() {
                                s.features[i].enabled = !s.features[i].enabled;
                                if !s.features[i].enabled
                                    && s.features[i].id == FeatureId::ShiftToggle
                                {
                                    s.release_shift();
                                }
                                // Save config when enable/disable state changes
                                let _ = s.save_config();
                            }
                        }

                        ui.end_row(); // Move to the next row in the grid
                    }
                });

            ui.add_space(10.0);
            ui.separator();
            egui::CollapsingHeader::new("Screen Editor").show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Monitor ID
                    ui.horizontal(|ui| {
                        ui.label("Monitor ID:");
                        if ui.text_edit_singleline(&mut s.monitor_id).changed() {
                            s.update_screen_position();
                            let _ = s.save_config();
                        }
                    });

                    // Monitor Pos (display only)
                    ui.label(format!(
                        "Monitor Pos: {}x{}, Size: {}x{}",
                        s.x_offset, s.y_offset, s.width, s.height
                    ));

                    ui.add_space(5.0);

                    // Auto Clicker Delay slider
                    ui.horizontal(|ui| {
                        ui.label("Auto Clicker Delay:");
                        ui.add(
                            egui::Slider::new(
                                &mut s.auto_clicker_delay,
                                AUTO_CLICKER_MIN_DELAY_MS..=AUTO_CLICKER_MAX_DELAY_MS,
                            )
                            .text("ms"),
                        );
                    });

                    ui.add_space(5.0);

                    // Position X/Y with Save/Load/Clear
                    ui.horizontal(|ui| {
                        ui.label("Position X:");
                        ui.add(egui::DragValue::new(&mut s.position_x).speed(1.0));
                        ui.label("Position Y:");
                        ui.add(egui::DragValue::new(&mut s.position_y).speed(1.0));
                    });

                    ui.add_space(5.0);

                    // Y offset for Tips Skip
                    ui.horizontal(|ui| {
                        ui.label("Tips Skip Y Offset:");
                        ui.add(egui::DragValue::new(&mut s.tips_skip_y_offset).speed(1.0));
                    });

                    // Y offset for Restart
                    ui.horizontal(|ui| {
                        ui.label("Restart Y Offset:");
                        ui.add(egui::DragValue::new(&mut s.restart_y_offset).speed(1.0));
                    });

                    // Y offset for Hacking Device
                    ui.horizontal(|ui| {
                        ui.label("Hacking Y Offset:");
                        ui.add(egui::DragValue::new(&mut s.hacking_y_offset).speed(1.0));
                    });

                    // Y offset for Hacking Device 2
                    ui.horizontal(|ui| {
                        ui.label("Hacking2 Y Offset:");
                        ui.add(egui::DragValue::new(&mut s.hacking2_y_offset).speed(1.0));
                    });

                    ui.add_space(10.0);

                    // Save/Load/Default buttons for Screen Editor
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            let _ = s.save_config();
                        }
                        if ui.button("Load").clicked() {
                            let _ = s.load_config();
                        }
                        if ui.button("Default").clicked() {
                            s.reset_to_defaults();
                        }
                    });
                });
            });
        });
    }
}

fn move_mouse(x: i32, y: i32) {
    unsafe {
        if SetCursorPos(x, y).is_err() {
            error!("Failed to move cursor to ({}, {})", x, y);
        }
    }
}

fn send_mouse_click() {
    send_instant_burst_clicks(1);
}

fn send_instant_burst_clicks(count: usize) {
    let mut inputs = Vec::with_capacity(count * 2);
    for _ in 0..count {
        let mut down = INPUT::default();
        down.r#type = INPUT_MOUSE;
        down.Anonymous.mi.dwFlags = MOUSEEVENTF_LEFTDOWN;
        inputs.push(down);

        let mut up = INPUT::default();
        up.r#type = INPUT_MOUSE;
        up.Anonymous.mi.dwFlags = MOUSEEVENTF_LEFTUP;
        inputs.push(up);
    }

    unsafe {
        let result = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if result == 0 {
            error!("SendInput failed to send {} mouse clicks", count);
        }
    }
}

fn send_key_tap(scan: u16) {
    unsafe {
        let mut inputs = [INPUT::default(); 2];
        inputs[0].r#type = INPUT_KEYBOARD;
        inputs[0].Anonymous.ki.wScan = scan;
        inputs[0].Anonymous.ki.dwFlags = KEYEVENTF_SCANCODE;
        inputs[1].r#type = INPUT_KEYBOARD;
        inputs[1].Anonymous.ki.wScan = scan;
        inputs[1].Anonymous.ki.dwFlags = KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP;
        if SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) == 0 {
            error!("SendInput failed to send key tap (scan: {})", scan);
        }
    }
}

fn send_key_state(scan: u16, down: bool) {
    unsafe {
        let mut input = INPUT::default();
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki.wScan = scan;
        input.Anonymous.ki.dwFlags = if down {
            KEYEVENTF_SCANCODE
        } else {
            KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP
        };
        if SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 0 {
            error!(
                "SendInput failed to set key state (scan: {}, down: {})",
                scan, down
            );
        }
    }
}

fn egui_to_rdev_key(key: egui::Key) -> Option<Key> {
    use egui::Key::*;
    match key {
        A => Some(Key::KeyA),
        B => Some(Key::KeyB),
        C => Some(Key::KeyC),
        D => Some(Key::KeyD),
        E => Some(Key::KeyE),
        F => Some(Key::KeyF),
        G => Some(Key::KeyG),
        H => Some(Key::KeyH),
        I => Some(Key::KeyI),
        J => Some(Key::KeyJ),
        K => Some(Key::KeyK),
        L => Some(Key::KeyL),
        M => Some(Key::KeyM),
        N => Some(Key::KeyN),
        O => Some(Key::KeyO),
        P => Some(Key::KeyP),
        Q => Some(Key::KeyQ),
        R => Some(Key::KeyR),
        S => Some(Key::KeyS),
        T => Some(Key::KeyT),
        U => Some(Key::KeyU),
        V => Some(Key::KeyV),
        W => Some(Key::KeyW),
        X => Some(Key::KeyX),
        Y => Some(Key::KeyY),
        Z => Some(Key::KeyZ),
        Num0 => Some(Key::Num0),
        Num1 => Some(Key::Num1),
        Num2 => Some(Key::Num2),
        Num3 => Some(Key::Num3),
        Num4 => Some(Key::Num4),
        Num5 => Some(Key::Num5),
        Num6 => Some(Key::Num6),
        Num7 => Some(Key::Num7),
        Num8 => Some(Key::Num8),
        Num9 => Some(Key::Num9),
        F1 => Some(Key::F1),
        F2 => Some(Key::F2),
        F3 => Some(Key::F3),
        F4 => Some(Key::F4),
        F5 => Some(Key::F5),
        F6 => Some(Key::F6),
        F7 => Some(Key::F7),
        F8 => Some(Key::F8),
        F9 => Some(Key::F9),
        F10 => Some(Key::F10),
        F11 => Some(Key::F11),
        F12 => Some(Key::F12),
        Space => Some(Key::Space),
        Enter => Some(Key::Return),
        Escape => Some(Key::Escape),
        Tab => Some(Key::Tab),
        Backspace => Some(Key::Backspace),
        Insert => Some(Key::Insert),
        Delete => Some(Key::Delete),
        Home => Some(Key::Home),
        End => Some(Key::End),
        PageUp => Some(Key::PageUp),
        PageDown => Some(Key::PageDown),
        ArrowUp => Some(Key::UpArrow),
        ArrowDown => Some(Key::DownArrow),
        ArrowLeft => Some(Key::LeftArrow),
        ArrowRight => Some(Key::RightArrow),
        _ => None,
    }
}

fn rdev_key_to_name(key: Key) -> String {
    ConfigKey::from_rdev(key)
        .map(|k| match k {
            ConfigKey::KeyA => "A".to_string(),
            ConfigKey::KeyB => "B".to_string(),
            ConfigKey::KeyC => "C".to_string(),
            ConfigKey::KeyD => "D".to_string(),
            ConfigKey::KeyE => "E".to_string(),
            ConfigKey::KeyF => "F".to_string(),
            ConfigKey::KeyG => "G".to_string(),
            ConfigKey::KeyH => "H".to_string(),
            ConfigKey::KeyI => "I".to_string(),
            ConfigKey::KeyJ => "J".to_string(),
            ConfigKey::KeyK => "K".to_string(),
            ConfigKey::KeyL => "L".to_string(),
            ConfigKey::KeyM => "M".to_string(),
            ConfigKey::KeyN => "N".to_string(),
            ConfigKey::KeyO => "O".to_string(),
            ConfigKey::KeyP => "P".to_string(),
            ConfigKey::KeyQ => "Q".to_string(),
            ConfigKey::KeyR => "R".to_string(),
            ConfigKey::KeyS => "S".to_string(),
            ConfigKey::KeyT => "T".to_string(),
            ConfigKey::KeyU => "U".to_string(),
            ConfigKey::KeyV => "V".to_string(),
            ConfigKey::KeyW => "W".to_string(),
            ConfigKey::KeyX => "X".to_string(),
            ConfigKey::KeyY => "Y".to_string(),
            ConfigKey::KeyZ => "Z".to_string(),
            ConfigKey::Num0 => "0".to_string(),
            ConfigKey::Num1 => "1".to_string(),
            ConfigKey::Num2 => "2".to_string(),
            ConfigKey::Num3 => "3".to_string(),
            ConfigKey::Num4 => "4".to_string(),
            ConfigKey::Num5 => "5".to_string(),
            ConfigKey::Num6 => "6".to_string(),
            ConfigKey::Num7 => "7".to_string(),
            ConfigKey::Num8 => "8".to_string(),
            ConfigKey::Num9 => "9".to_string(),
            ConfigKey::ControlLeft | ConfigKey::ControlRight => "Ctrl".to_string(),
            ConfigKey::ShiftLeft | ConfigKey::ShiftRight => "Shift".to_string(),
            ConfigKey::UpArrow => "↑".to_string(),
            ConfigKey::DownArrow => "↓".to_string(),
            ConfigKey::LeftArrow => "←".to_string(),
            ConfigKey::RightArrow => "→".to_string(),
            ConfigKey::Return => "Enter".to_string(),
            _ => k.to_string(),
        })
        .unwrap_or_else(|| format!("{:?}", key))
}

fn main() -> eframe::Result {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([450.0, 500.0]),
        ..Default::default()
    };
    let state = Arc::new(Mutex::new(AppState::new()));
    KeyBindApp::start_hotkey_listener(state.clone(), InputState);
    eframe::run_native(
        "nztool",
        options,
        Box::new(|_| {
            Ok(Box::new(KeyBindApp {
                state,
                prev_ctrl: false,
                prev_shift: false,
                prev_alt: false,
                prev_capslock: false,
            }))
        }),
    )
}
