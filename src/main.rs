#![windows_subsystem = "windows"]

use eframe::egui;
use log::{error, info, warn};
use rdev::{grab, Event, EventType, Key};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Console::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// Simple logger implementation to output to console
struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            println!("{} - {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: SimpleLogger = SimpleLogger;

// Global input state - consolidated structure
// Kept as statics because Windows hooks require them (hooks run in system context)
struct GlobalInputState {
    lmb_down: AtomicBool,
    space_down: AtomicBool,
    autoclicker_active: AtomicBool,
    bhop_active: AtomicBool,
    ctrl_down: AtomicBool,
    shift_down: AtomicBool,
    alt_down: AtomicBool,
    capslock_down: AtomicBool,
    mouse_hook_thread_id: AtomicU32,
    keyboard_hook_thread_id: AtomicU32,
    last_focus_check: AtomicU64,
    cached_focus_value: AtomicBool,
    rdev_shutdown: AtomicBool,
    lmb_hold_active: AtomicBool,
    click_debug_enabled: AtomicBool,
}

impl GlobalInputState {
    const fn new() -> Self {
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
        }
    }
}

static GLOBAL_STATE: GlobalInputState = GlobalInputState::new();

// Worker thread messages for feature execution
enum WorkerMessage {
    ShiftToggle,
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
    GunAndTool { digit: u32 },
    QuickExit { x: i32, y: i32 },
}

// InputState provides a clean API over the global state
#[derive(Clone)]
struct InputState;

impl InputState {
    fn is_lmb_down(&self) -> bool {
        GLOBAL_STATE.lmb_down.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn set_lmb_down(&self, down: bool) {
        GLOBAL_STATE.lmb_down.store(down, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    fn set_space_down(&self, down: bool) {
        GLOBAL_STATE.space_down.store(down, Ordering::SeqCst);
    }

    fn is_space_down(&self) -> bool {
        GLOBAL_STATE.space_down.load(Ordering::SeqCst)
    }

    fn toggle_autoclicker(&self) -> bool {
        let current = GLOBAL_STATE.autoclicker_active.load(Ordering::SeqCst);
        GLOBAL_STATE
            .autoclicker_active
            .store(!current, Ordering::SeqCst);
        !current
    }

    fn is_autoclicker_active(&self) -> bool {
        GLOBAL_STATE.autoclicker_active.load(Ordering::SeqCst)
    }

    fn toggle_bhop(&self) -> bool {
        let current = GLOBAL_STATE.bhop_active.load(Ordering::SeqCst);
        GLOBAL_STATE.bhop_active.store(!current, Ordering::SeqCst);
        !current
    }

    fn is_bhop_active(&self) -> bool {
        GLOBAL_STATE.bhop_active.load(Ordering::SeqCst)
    }

    fn toggle_lmb_hold(&self) -> bool {
        let current = GLOBAL_STATE.lmb_hold_active.load(Ordering::SeqCst);
        GLOBAL_STATE
            .lmb_hold_active
            .store(!current, Ordering::SeqCst);
        !current
    }

    fn is_lmb_hold_active(&self) -> bool {
        GLOBAL_STATE.lmb_hold_active.load(Ordering::SeqCst)
    }

    fn set_lmb_hold_active(&self, active: bool) {
        GLOBAL_STATE.lmb_hold_active.store(active, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    fn is_ctrl_down(&self) -> bool {
        GLOBAL_STATE.ctrl_down.load(Ordering::SeqCst)
    }

    fn is_shift_down(&self) -> bool {
        GLOBAL_STATE.shift_down.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn is_alt_down(&self) -> bool {
        GLOBAL_STATE.alt_down.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn is_capslock_down(&self) -> bool {
        GLOBAL_STATE.capslock_down.load(Ordering::SeqCst)
    }

    fn set_mouse_hook_thread_id(&self, id: u32) {
        GLOBAL_STATE
            .mouse_hook_thread_id
            .store(id, Ordering::SeqCst);
    }

    fn get_mouse_hook_thread_id(&self) -> u32 {
        GLOBAL_STATE.mouse_hook_thread_id.load(Ordering::SeqCst)
    }

    fn set_keyboard_hook_thread_id(&self, id: u32) {
        GLOBAL_STATE
            .keyboard_hook_thread_id
            .store(id, Ordering::SeqCst);
    }

    fn get_keyboard_hook_thread_id(&self) -> u32 {
        GLOBAL_STATE.keyboard_hook_thread_id.load(Ordering::SeqCst)
    }
}

// Helper function to safely lock mutex and handle poisoned state
fn safe_lock<T>(mutex: &Arc<Mutex<T>>) -> Option<std::sync::MutexGuard<'_, T>> {
    match mutex.lock() {
        Ok(guard) => Some(guard),
        Err(poisoned) => {
            error!("Mutex poisoned, recovering with poisoned data");
            Some(poisoned.into_inner())
        }
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
        
        unsafe { GetTickCount64() }
    };

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

// --- CONSTANTS ---
const HACKING_DELAY_MS: u64 = 50;
const MOUSE_CLICK_PRE_DELAY_MS: u64 = 10;
const HOLD_ITEM_TAP_DELAY_MS: u64 = 7;
const RESTART_KEY_DELAY_MS: u64 = 100;
const NO_FALL_DAMAGE_DELAY_MS: u64 = 30;
const QUICK_EXIT_DELAY_MS: u64 = 60;
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
                    GLOBAL_STATE.lmb_down.store(true, Ordering::SeqCst);
                    if GLOBAL_STATE.click_debug_enabled.load(Ordering::SeqCst) {
                        info!("Click at: ({}, {})", ms_ll.pt.x, ms_ll.pt.y);
                    }
                } else if w_param.0 as u32 == WM_LBUTTONUP {
                    GLOBAL_STATE.lmb_down.store(false, Ordering::SeqCst);
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
                        GLOBAL_STATE.space_down.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        GLOBAL_STATE.space_down.store(false, Ordering::SeqCst);
                    }
                }

                // VK_SHIFT = 0x10, VK_LSHIFT = 0xA0, VK_RSHIFT = 0xA1
                if kb_ll.vkCode == 0x10 || kb_ll.vkCode == 0xA0 || kb_ll.vkCode == 0xA1 {
                    if is_key_down {
                        GLOBAL_STATE.shift_down.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        GLOBAL_STATE.shift_down.store(false, Ordering::SeqCst);
                    }
                }

                // VK_CONTROL = 0x11, VK_LCONTROL = 0xA2, VK_RCONTROL = 0xA3
                if kb_ll.vkCode == 0x11 || kb_ll.vkCode == 0xA2 || kb_ll.vkCode == 0xA3 {
                    if is_key_down {
                        GLOBAL_STATE.ctrl_down.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        GLOBAL_STATE.ctrl_down.store(false, Ordering::SeqCst);
                    }
                }

                // VK_MENU = 0x12, VK_LMENU = 0xA4, VK_RMENU = 0xA5
                if kb_ll.vkCode == 0x12 || kb_ll.vkCode == 0xA4 || kb_ll.vkCode == 0xA5 {
                    if is_key_down {
                        GLOBAL_STATE.alt_down.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        GLOBAL_STATE.alt_down.store(false, Ordering::SeqCst);
                    }
                }

                // VK_CAPITAL = 0x14
                if kb_ll.vkCode == 0x14 {
                    if is_key_down {
                        GLOBAL_STATE.capslock_down.store(true, Ordering::SeqCst);
                    } else if is_key_up {
                        GLOBAL_STATE.capslock_down.store(false, Ordering::SeqCst);
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
    // TODO: FastDrag - planned for future implementation
    AutoClicker,
    GrabNoGun,
    Bhop,
    HoldItemBug,
    LmbHoldToggle,
    GunAndTool,
    QuickExit,
});

// Serializable structure for config file
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
enum ClickMethod {
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

#[derive(Serialize, Deserialize, Clone)]
struct SerializableConfig {
    monitor_id: String,
    features: Vec<SerializableFeature>,
    auto_clicker_delay: u32,
    auto_clicker_method: ClickMethod,
    #[serde(default = "default_auto_clicker_click_count")]
    auto_clicker_click_count: u32,
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
    #[serde(default = "default_gun_tool_digit")]
    gun_tool_digit: u32,
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
fn default_auto_clicker_click_count() -> u32 {
    1
}
fn default_gun_tool_digit() -> u32 {
    1
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
    auto_clicker_delay: u32,
    auto_clicker_method: ClickMethod,
    auto_clicker_click_count: u32,
    position_x: i32,
    position_y: i32,
    tips_skip_y_offset: i32,
    restart_y_offset: i32,
    hacking_y_offset: i32,
    hacking2_y_offset: i32,
    gun_tool_digit: u32,
    dev_mode: bool,
}

// Get the path to the config directory and file
fn get_config_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let config_dir = std::path::PathBuf::from(appdata).join("nzconfig");
    
    config_dir.join("config.json")
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
            auto_clicker_method: self.auto_clicker_method,
            auto_clicker_click_count: self.auto_clicker_click_count,
            position_x: self.position_x,
            position_y: self.position_y,
            tips_skip_y_offset: self.tips_skip_y_offset,
            restart_y_offset: self.restart_y_offset,
            hacking_y_offset: self.hacking_y_offset,
            hacking2_y_offset: self.hacking2_y_offset,
            gun_tool_digit: self.gun_tool_digit,
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
        self.auto_clicker_method = config.auto_clicker_method;
        self.auto_clicker_click_count = config.auto_clicker_click_count;
        self.position_x = config.position_x;
        self.position_y = config.position_y;
        self.tips_skip_y_offset = config.tips_skip_y_offset;
        self.restart_y_offset = config.restart_y_offset;
        self.hacking_y_offset = config.hacking_y_offset;
        self.hacking2_y_offset = config.hacking2_y_offset;
        self.gun_tool_digit = config.gun_tool_digit.clamp(1, 3);

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
                Feature {
                    id: FeatureId::LmbHoldToggle,
                    name: "LMB Hold Toggle".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::GunAndTool,
                    name: "Gun & Tool (In multiplayer)".into(),
                    rdev_key: None,
                    enabled: false,
                    selecting: false,
                },
                Feature {
                    id: FeatureId::QuickExit,
                    name: "Quick Exit".into(),
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
            auto_clicker_delay: 6,
            auto_clicker_method: ClickMethod::default(),
            auto_clicker_click_count: 1,
            position_x: 0,
            position_y: 0,
            tips_skip_y_offset: 830,
            restart_y_offset: 486,
            hacking_y_offset: -140,
            hacking2_y_offset: -140,
            gun_tool_digit: 1,
            dev_mode: false,
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

    fn reset_to_defaults(&mut self) {
        self.auto_clicker_delay = 6;
        self.auto_clicker_method = ClickMethod::default();
        self.auto_clicker_click_count = 1;
        self.position_x = 0;
        self.position_y = 0;
        self.tips_skip_y_offset = 830;
        self.restart_y_offset = 486;
        self.hacking_y_offset = -140;
        self.hacking2_y_offset = -140;
        self.gun_tool_digit = 1;
        self.dev_mode = false;
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
        GLOBAL_STATE.rdev_shutdown.store(true, Ordering::SeqCst);

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
                loop {
                    // Check shutdown flag before blocking on GetMessageW
                    if GLOBAL_STATE.rdev_shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    match GetMessageW(&mut msg, HWND::default(), 0, 0) {
                        BOOL(0) => break, // WM_QUIT received
                        BOOL(-1) => {
                            error!("GetMessageW failed in mouse hook");
                            break;
                        }
                        _ => {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
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
                loop {
                    // Check shutdown flag before blocking on GetMessageW
                    if GLOBAL_STATE.rdev_shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    match GetMessageW(&mut msg, HWND::default(), 0, 0) {
                        BOOL(0) => break, // WM_QUIT received
                        BOOL(-1) => {
                            error!("GetMessageW failed in keyboard hook");
                            break;
                        }
                        _ => {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
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

        // Modifier key polling thread - triggers hotkeys for Shift
        // rdev::grab doesn't capture modifier keys, so we poll via Windows hook state
        let state_clone_mod = state.clone();
        thread::spawn(move || {
            let mut prev_shift = false;
            loop {
                let shift_down = input_mod_poll.is_shift_down();

                if is_game_focused() {
                    let Some(s) = safe_lock(&state_clone_mod) else {
                        error!("Failed to lock state in modifier polling thread");
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                        continue;
                    };
                    if shift_down && !prev_shift
                        && let Some(f) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.rdev_key == Some(Key::ShiftLeft))
                            && f.id == FeatureId::ShiftToggle {
                                let _ = worker_tx_mod.send(WorkerMessage::ShiftToggle);
                            }
                }

                prev_shift = shift_down;
                thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
        });

        let state_clone_ac = state.clone();
        thread::spawn(move || {
            loop {
                let (enabled, delay_ms, click_method, click_count) = {
                    let Some(s) = safe_lock(&state_clone_ac) else {
                        error!("Failed to lock state in autoclicker thread");
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                        continue;
                    };
                    let enabled = s
                        .features
                        .iter()
                        .any(|f| f.id == FeatureId::AutoClicker && f.enabled);
                    (enabled, s.auto_clicker_delay, s.auto_clicker_method, s.auto_clicker_click_count)
                };
                let active = input_ac.is_autoclicker_active();

                if active && enabled && is_game_focused() {
                    if input_ac.is_lmb_down() {
                        match click_method {
                            ClickMethod::SendInput => send_mouse_click(),
                            ClickMethod::PostMessage => send_mouse_click_postmessage(click_count),
                        }
                        thread::sleep(Duration::from_millis(delay_ms as u64));
                    } else {
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                    }
                } else {
                    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                }
            }
        });

        // LMB Hold Toggle worker thread
        let state_clone_lmb_hold = state.clone();
        let input_lmb_hold = input_state.clone();
        thread::spawn(move || {
            let mut was_active_and_focused = false;
            loop {
                let enabled = {
                    let Some(s) = safe_lock(&state_clone_lmb_hold) else {
                        error!("Failed to lock state in LMB hold thread");
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                        continue;
                    };
                    s.features.iter().any(|f| f.id == FeatureId::LmbHoldToggle && f.enabled)
                };

                let active = input_lmb_hold.is_lmb_hold_active();
                let focused = is_game_focused();
                let currently_active = active && enabled && focused;

                if currently_active {
                    send_mouse_hold(true);
                    was_active_and_focused = true;
                    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                } else {
                    if was_active_and_focused {
                        send_mouse_hold(false);
                        was_active_and_focused = false;
                    }
                    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                }
            }
        });

        // Worker thread for feature execution
        let worker_state = state.clone();
        thread::spawn(move || {
            while let Ok(msg) = worker_rx.recv() {
                match msg {
                    WorkerMessage::ShiftToggle => {
                        if let Some(mut s) = safe_lock(&worker_state) {
                            s.toggle_shift();
                        }
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
                    WorkerMessage::GunAndTool { digit } => {
                        Self::gun_and_tool(digit);
                    }
                    WorkerMessage::QuickExit { x, y } => {
                        Self::quick_exit(x, y);
                    }
                }
            }
        });

        let state_clone_hk = state.clone();
        thread::spawn(move || {
            let callback = move |event: Event| {
                // Gracefully shut down if requested
                if GLOBAL_STATE.rdev_shutdown.load(Ordering::SeqCst) {
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
                    let Some(s) = safe_lock(&state_clone_hk) else {
                        error!("Failed to lock state in hotkey callback");
                        return Some(event);
                    };

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
                            let gun_digit = s.gun_tool_digit;

                            // Handle toggle features immediately
                            if feature_id == FeatureId::AutoClicker {
                                let _ = input_hotkey.toggle_autoclicker();
                                return None;
                            }

                            if feature_id == FeatureId::Bhop {
                                let _ = input_hotkey.toggle_bhop();
                                return None;
                            }

                            if feature_id == FeatureId::LmbHoldToggle {
                                let _ = input_hotkey.toggle_lmb_hold();
                                return None;
                            }

                            // Build worker message for other features
                            let action = match feature_id {
                                FeatureId::ShiftToggle => Some(WorkerMessage::ShiftToggle),
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
                                FeatureId::GunAndTool => Some(WorkerMessage::GunAndTool { digit: gun_digit }),
                                FeatureId::QuickExit => Some(WorkerMessage::QuickExit { x, y }),
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
            set_cursor_pos_safe(target_x, target_y);

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
            set_cursor_pos_safe(center_x, y);
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
            set_cursor_pos_safe(target_x, target_y);

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
            let mut wheel_input = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
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

    fn gun_and_tool(digit: u32) {
        Self::send_mouse_state(true);
        thread::sleep(Duration::from_millis(100));
        send_key_tap(0x01); // ESC
        thread::sleep(Duration::from_millis(100));
        Self::send_mouse_state(false);
        thread::sleep(Duration::from_millis(100));
        // 1=0x02, 2=0x03, 3=0x04
        send_key_tap(digit as u16 + 1);
        thread::sleep(Duration::from_millis(100));
        send_key_tap(0x01); // ESC
    }
    
    fn quick_exit(x_offset: i32, y_offset: i32) {
        send_key_tap(0x01); // ESC
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        move_mouse(x_offset + 722, y_offset + 731);
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        send_mouse_click();
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        move_mouse(x_offset + 719, y_offset + 546);
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        send_mouse_click();
    }

    fn send_mouse_state(down: bool) {
        unsafe {
            let mut input = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
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
        let Some(mut s) = safe_lock(&self.state) else {
            error!("Failed to lock state in UI update");
            return;
        };
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
                        if !modifiers.ctrl && !modifiers.shift && !modifiers.alt
                            && (*key == egui::Key::Escape || egui_to_rdev_key(*key).is_some()) {
                                return Some(*key);
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
                            if s.features[i].id == FeatureId::LmbHoldToggle {
                                InputState.set_lmb_hold_active(false);
                                send_mouse_hold(false);
                            }
                            s.features[i].rdev_key = None;
                            s.features[i].enabled = false;
                            s.features[i].selecting = false;
                            let _ = s.save_config();
                        }

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
                        if s.features[i].id == FeatureId::LmbHoldToggle
                            && InputState.is_lmb_hold_active()
                            && s.features[i].enabled
                        {
                            color = egui::Color32::BLUE;
                        }

                        if ui
                            .add(egui::Button::new("Enable/Disable").fill(color))
                            .clicked()
                            && s.features[i].rdev_key.is_some() {
                                s.features[i].enabled = !s.features[i].enabled;
                                if !s.features[i].enabled {
                                    if s.features[i].id == FeatureId::ShiftToggle {
                                        s.release_shift();
                                    }
                                    if s.features[i].id == FeatureId::LmbHoldToggle {
                                        InputState.set_lmb_hold_active(false);
                                        send_mouse_hold(false);
                                    }
                                }
                                let _ = s.save_config();
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

                    // Auto Clicker Method selection
                    ui.horizontal(|ui| {
                        ui.label("Click Method:");
                        let send_input_selected = s.auto_clicker_method == ClickMethod::SendInput;
                        let post_message_selected = s.auto_clicker_method == ClickMethod::PostMessage;
                        if ui.selectable_label(send_input_selected, "SendInput").clicked() {
                            s.auto_clicker_method = ClickMethod::SendInput;
                            let _ = s.save_config();
                        }
                        if ui.selectable_label(post_message_selected, "PostMessage").clicked() {
                            s.auto_clicker_method = ClickMethod::PostMessage;
                            let _ = s.save_config();
                        }
                    });

                    // PostMessage click count
                    if s.auto_clicker_method == ClickMethod::PostMessage {
                        ui.horizontal(|ui| {
                            ui.label("Clicks per trigger:");
                            ui.add(egui::DragValue::new(&mut s.auto_clicker_click_count).speed(1.0).range(1..=20));
                            if ui.button("Apply").clicked() {
                                let _ = s.save_config();
                            }
                        });
                    }

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

                    ui.add_space(5.0);

                    // Gun & Tool Digit
                    ui.horizontal(|ui| {
                        ui.label("Gun & Tool Digit:");
                        let mut val = s.gun_tool_digit;
                        if ui.add(egui::DragValue::new(&mut val).range(1..=6969)).changed() {
                            if val == 6969 {
                                s.dev_mode = true;
                                s.gun_tool_digit = 3;
                            } else {
                                s.gun_tool_digit = val.clamp(1, 3);
                            }
                            let _ = s.save_config();
                        }
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

            if s.dev_mode {
                ui.add_space(10.0);
                ui.separator();
                ui.heading("Developer Tools");
                let mut debug_enabled = GLOBAL_STATE.click_debug_enabled.load(Ordering::SeqCst);
                if ui.checkbox(&mut debug_enabled, "Enable Click Debugging").changed() {
                    GLOBAL_STATE
                        .click_debug_enabled
                        .store(debug_enabled, Ordering::SeqCst);
                    unsafe {
                        if debug_enabled {
                            let _ = AllocConsole();
                        } else {
                            let _ = FreeConsole();
                        }
                    }
                }

                if ui.button("Open Log Directory").clicked() {
                    let config_path = get_config_path();
                    if let Some(parent) = config_path.parent() {
                        let _ = std::process::Command::new("explorer")
                            .arg(parent)
                            .spawn();
                    }
                }
            }
        });
    }
}

// Helper function to safely set cursor position with retry logic
fn set_cursor_pos_safe(x: i32, y: i32) -> bool {
    const MAX_RETRIES: u32 = 3;
    const RETRY_DELAY_MS: u64 = 5;

    for attempt in 0..MAX_RETRIES {
        if unsafe { SetCursorPos(x, y).is_ok() } {
            return true;
        }
        if attempt < MAX_RETRIES - 1 {
            warn!("SetCursorPos attempt {} failed, retrying...", attempt + 1);
            thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
        }
    }
    error!(
        "SetCursorPos failed after {} attempts for ({}, {})",
        MAX_RETRIES, x, y
    );
    false
}

fn move_mouse(x: i32, y: i32) {
    set_cursor_pos_safe(x, y);
}

fn send_mouse_click() {
    send_instant_burst_clicks(1);
}

fn send_mouse_click_postmessage(click_count: u32) {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            error!("GetForegroundWindow returned null in PostMessage click");
            return;
        }

        let mut cursor_pos = POINT::default();
        if GetCursorPos(&mut cursor_pos).is_err() {
            error!("GetCursorPos failed in PostMessage click");
            return;
        }

        let mut pt = POINT {
            x: cursor_pos.x,
            y: cursor_pos.y,
        };
        if !ScreenToClient(hwnd, &mut pt).as_bool() {
            warn!("ScreenToClient failed in PostMessage click");
        }

        let l_param = pack_lparam(pt.x, pt.y);

        for _ in 0..click_count {
            if PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), LPARAM(l_param)).is_err() {
                error!("PostMessageA WM_LBUTTONDOWN failed");
            }
            if PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(l_param)).is_err() {
                error!("PostMessageA WM_LBUTTONUP failed");
            }
        }
    }
}

fn send_instant_burst_clicks(count: usize) {
    let mut inputs = Vec::with_capacity(count * 2);
    for _ in 0..count {
        let mut down = INPUT {
            r#type: INPUT_MOUSE,
            ..Default::default()
        };
        down.Anonymous.mi.dwFlags = MOUSEEVENTF_LEFTDOWN;
        inputs.push(down);

        let mut up = INPUT {
            r#type: INPUT_MOUSE,
            ..Default::default()
        };
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

fn send_mouse_hold(down: bool) {
    unsafe {
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            ..Default::default()
        };
        input.Anonymous.mi.dwFlags = if down {
            MOUSEEVENTF_LEFTDOWN
        } else {
            MOUSEEVENTF_LEFTUP
        };
        if SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 0 {
            error!("SendInput failed in send_mouse_hold (down: {})", down);
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
        let mut input = INPUT {
            r#type: INPUT_KEYBOARD,
            ..Default::default()
        };
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
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
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
