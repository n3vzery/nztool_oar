#![windows_subsystem = "windows"]

use eframe::egui;
use log::{error, info, warn};
use rdev::{grab, Event, EventType, Key};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
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

// global state
// static because windows hooks need it
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
    all_macros_disabled: AtomicBool,
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
            all_macros_disabled: AtomicBool::new(false),
        }
    }
}

static GLOBAL_STATE: GlobalInputState = GlobalInputState::new();

// globals for mouse hook to dispatch xbutton events
static MOUSE_HOOK_TX: OnceLock<mpsc::Sender<WorkerMessage>> = OnceLock::new();
static MOUSE_HOOK_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();
static MOUSE_HOOK_INPUT: OnceLock<InputState> = OnceLock::new();

// Worker thread messages for feature execution
enum WorkerMessage {
    ShiftToggle,
    NoFallDamage,
    HackingClickMtd {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        offset_y: i32,
    },
    HackingJumpMtd {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        offset_y: i32,
    },
    HackingEscMtd {
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
    FastLoadout,
    HoldItemBug,
    GangstaGrip { digit: u32 },
    QuickExit { x: i32, y: i32 },
    DoubleClick(DoubleClickButton),
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

    fn toggle_all_macros_disabled(&self) -> bool {
        let current = GLOBAL_STATE.all_macros_disabled.load(Ordering::SeqCst);
        GLOBAL_STATE
            .all_macros_disabled
            .store(!current, Ordering::SeqCst);
        !current
    }

    fn are_all_macros_disabled(&self) -> bool {
        GLOBAL_STATE.all_macros_disabled.load(Ordering::SeqCst)
    }
}

// safe mutex lock
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
    Comma => Comma, Dot => Dot, Slash => Slash, SemiColon => SemiColon,
    Quote => Quote, LeftBracket => LeftBracket, RightBracket => RightBracket,
    BackSlash => BackSlash, Minus => Minus, Equal => Equal, Backquote => BackQuote,
});

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
const HACKING_DELAY_MS: u64 = 12;
const MOUSE_CLICK_PRE_DELAY_MS: u64 = 2;
const HOLD_ITEM_TAP_DELAY_MS: u64 = 1;
const RESTART_KEY_DELAY_MS: u64 = 30;
const RESTART_SETTLE_DELAY_MS: u64 = 10;
const NO_FALL_DAMAGE_DELAY_MS: u64 = 7;
const QUICK_EXIT_DELAY_MS: u64 = 15;
const BHOP_TAP_INTERVAL_MS: u64 = 3;
const POLL_INTERVAL_MS: u64 = 5;
const AUTO_CLICKER_MIN_DELAY_MS: u32 = 0;
const AUTO_CLICKER_MAX_DELAY_MS: u32 = 500;

// pack coords to LPARAM for PostMessage
// i16 cast fixes negative coords
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

            // ignore injected events to avoid looping
            if (ms_ll.flags & LLMHF_INJECTED) == 0 {
                if w_param.0 as u32 == WM_LBUTTONDOWN {
                    GLOBAL_STATE.lmb_down.store(true, Ordering::SeqCst);
                    if GLOBAL_STATE.click_debug_enabled.load(Ordering::SeqCst) {
                        info!("Click at: ({}, {})", ms_ll.pt.x, ms_ll.pt.y);
                    }
                } else if w_param.0 as u32 == WM_LBUTTONUP {
                    GLOBAL_STATE.lmb_down.store(false, Ordering::SeqCst);
                } else if w_param.0 as u32 == 0x020B || w_param.0 as u32 == 0x020C {
                    // WM_XBUTTONDOWN (0x020B) / WM_XBUTTONUP (0x020C) - side mouse buttons
                    let xbutton = (ms_ll.mouseData >> 16) as u16;
                    let bind_key = if xbutton == 1 {
                        BindKey::Mouse4
                    } else {
                        BindKey::Mouse5
                    };

                    // check if this button is bound to any enabled feature
                    let is_bound = if is_game_focused() {
                        if let Some(state) = MOUSE_HOOK_STATE.get() {
                            if let Some(s) = safe_lock(state) {
                                s.features.iter().any(|f| f.enabled && f.bind_key == Some(bind_key))
                            } else { false }
                        } else { false }
                    } else { false };

                    if is_bound {
                        // only dispatch macro on button down, not up
                        if w_param.0 as u32 == 0x020B {
                            thread::spawn(move || {
                                handle_mouse_bind(bind_key);
                            });
                        }
                        // block the event from reaching the game
                        return LRESULT(1);
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), n_code, w_param, l_param) }
}

// dispatches a mouse button bind through the worker channel
fn handle_mouse_bind(bind_key: BindKey) {
    if !is_game_focused() {
        return;
    }

    let (Some(tx), Some(state), Some(input)) = (
        MOUSE_HOOK_TX.get(),
        MOUSE_HOOK_STATE.get(),
        MOUSE_HOOK_INPUT.get(),
    ) else {
        return;
    };

    let Some(s) = safe_lock(state) else {
        return;
    };

    let Some(feature) = s.features.iter().find(|f| f.enabled && f.bind_key == Some(bind_key)) else {
        return;
    };

    let feature_id = feature.id;
    let x = s.x_offset;
    let y = s.y_offset;
    let w = s.width;
    let h = s.height;
    let tips_y = s.tips_skip_y_offset;
    let restart_y = s.restart_y_offset;
    let hack_y = s.hacking_y_offset;
    let hack2_y = s.hacking2_y_offset;
    let hack_esc_y = s.hacking_esc_y_offset;
    let gun_digit = s.gun_tool_digit;
    let double_click_btn = s.double_click_button;
    drop(s);

    if feature_id == FeatureId::ToggleAllMacros {
        let _ = input.toggle_all_macros_disabled();
        return;
    }
    if input.are_all_macros_disabled() {
        return;
    }
    if feature_id == FeatureId::AutoClicker {
        let _ = input.toggle_autoclicker();
        return;
    }
    if feature_id == FeatureId::Bhop {
        let _ = input.toggle_bhop();
        return;
    }
    if feature_id == FeatureId::KeepItemClicker {
        let _ = input.toggle_lmb_hold();
        return;
    }

    let msg = match feature_id {
        FeatureId::ShiftToggle => Some(WorkerMessage::ShiftToggle),
        FeatureId::NoFallDamage => Some(WorkerMessage::NoFallDamage),
        FeatureId::HackingClickMtd => Some(WorkerMessage::HackingClickMtd { x, y, w, h, offset_y: hack_y }),
        FeatureId::HackingJumpMtd => Some(WorkerMessage::HackingJumpMtd { x, y, w, h, offset_y: hack2_y }),
        FeatureId::HackingEscMtd => Some(WorkerMessage::HackingEscMtd { x, y, w, h, offset_y: hack_esc_y }),
        FeatureId::TipsSkip => Some(WorkerMessage::TipsSkip { x, w, y: y + tips_y }),
        FeatureId::Restart => Some(WorkerMessage::Restart { x, w, y: y + restart_y }),
        FeatureId::FastLoadout => Some(WorkerMessage::FastLoadout),
        FeatureId::HoldItemBug => Some(WorkerMessage::HoldItemBug),
        FeatureId::GangstaGrip => Some(WorkerMessage::GangstaGrip { digit: gun_digit }),
        FeatureId::QuickExit => Some(WorkerMessage::QuickExit { x, y }),
        FeatureId::DoubleClick => Some(WorkerMessage::DoubleClick(double_click_btn)),
        _ => None,
    };

    if let Some(m) = msg {
        let _ = tx.send(m);
    }
}

unsafe extern "system" fn low_level_keyboard_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code == HC_ACTION as i32 {
        unsafe {
            let kb_ll = *(l_param.0 as *const KBDLLHOOKSTRUCT);

            // ignore injected keys to prevent loops
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

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum FeatureId {
    #[serde(alias = "HackingPostMessage")]
    HackingClickMtd,
    #[serde(alias = "HackingPostMessage2")]
    HackingJumpMtd,
    #[serde(alias = "HackingEsc")]
    HackingEscMtd,
    TipsSkip,
    Restart,
    NoFallDamage,
    ShiftToggle,
    // TODO: FastDrag - planned for future implementation
    AutoClicker,
    #[serde(alias = "GrabNoGun")]
    FastLoadout,
    Bhop,
    HoldItemBug,
    #[serde(alias = "LMBHoldToggle")]
    KeepItemClicker,
    #[serde(alias = "GunAndTool")]
    GangstaGrip,
    QuickExit,
    ToggleAllMacros,
    DoubleClick,
}

impl std::fmt::Display for FeatureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::str::FromStr for FeatureId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "HackingClickMtd" | "HackingPostMessage" => Ok(Self::HackingClickMtd),
            "HackingJumpMtd" | "HackingPostMessage2" => Ok(Self::HackingJumpMtd),
            "HackingEscMtd" | "HackingEsc" => Ok(Self::HackingEscMtd),
            "TipsSkip" => Ok(Self::TipsSkip),
            "Restart" => Ok(Self::Restart),
            "NoFallDamage" => Ok(Self::NoFallDamage),
            "ShiftToggle" => Ok(Self::ShiftToggle),
            "AutoClicker" => Ok(Self::AutoClicker),
            "FastLoadout" | "GrabNoGun" => Ok(Self::FastLoadout),
            "Bhop" => Ok(Self::Bhop),
            "HoldItemBug" => Ok(Self::HoldItemBug),
            "KeepItemClicker" | "LMBHoldToggle" => Ok(Self::KeepItemClicker),
            "GangstaGrip" | "GunAndTool" => Ok(Self::GangstaGrip),
            "QuickExit" => Ok(Self::QuickExit),
            "ToggleAllMacros" => Ok(Self::ToggleAllMacros),
            "DoubleClick" => Ok(Self::DoubleClick),
            _ => Err(format!("Unknown variant: {}", s)),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum DoubleClickButton {
    #[default]
    Left,
    Right,
}

impl std::fmt::Display for DoubleClickButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DoubleClickButton::Left => write!(f, "LMB"),
            DoubleClickButton::Right => write!(f, "RMB"),
        }
    }
}

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

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
enum AutoClickerMode {
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
struct SerializableConfig {
    monitor_id: String,
    features: Vec<SerializableFeature>,
    auto_clicker_delay: u32,
    auto_clicker_method: ClickMethod,
    #[serde(default = "default_auto_clicker_click_count")]
    auto_clicker_click_count: u32,
    #[serde(default = "default_auto_clicker_mode")]
    auto_clicker_mode: AutoClickerMode,
    #[serde(default = "default_auto_clicker_key")]
    auto_clicker_key: ConfigKey,
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
    #[serde(default = "default_hacking_esc_y")]
    hacking_esc_y_offset: i32,
    #[serde(default = "default_gun_tool_digit")]
    gun_tool_digit: u32,
    #[serde(default = "default_double_click_button")]
    double_click_button: DoubleClickButton,
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

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BindKey {
    Keyboard(Key),
    MouseMiddle,
    Mouse4,
    Mouse5,
}

impl BindKey {
    pub fn to_string(&self) -> String {
        match self {
            BindKey::Keyboard(k) => key_to_string(*k),
            BindKey::MouseMiddle => "Mouse Middle".to_string(),
            BindKey::Mouse4 => "Mouse 4".to_string(),
            BindKey::Mouse5 => "Mouse 5".to_string(),
        }
    }

    pub fn from_string(s: &str) -> Option<Self> {
        match s {
            "Mouse Middle" => Some(BindKey::MouseMiddle),
            "Mouse 4" => Some(BindKey::Mouse4),
            "Mouse 5" => Some(BindKey::Mouse5),
            _ => string_to_key(s).map(BindKey::Keyboard),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct SerializableFeature {
    id: FeatureId,
    bind_key: Option<String>,
    enabled: bool,
}

struct Feature {
    id: FeatureId,
    name: String,
    bind_key: Option<BindKey>,
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
    auto_clicker_mode: AutoClickerMode,
    auto_clicker_key: ConfigKey,
    position_x: i32,
    position_y: i32,
    tips_skip_y_offset: i32,
    restart_y_offset: i32,
    hacking_y_offset: i32,
    hacking2_y_offset: i32,
    hacking_esc_y_offset: i32,
    gun_tool_digit: u32,
    dev_mode: bool,
    presets: Vec<String>,
    selected_preset: String,
    preset_name_input: String,
    double_click_button: DoubleClickButton,
}

// Get the path to the config directory and file
fn get_config_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let config_dir = std::path::PathBuf::from(appdata).join("nzconfig");
    
    config_dir.join("config.json")
}

fn get_presets_dir() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(appdata).join("nzconfig").join("presets")
}

// Convert rdev::Key to ConfigKey then to string
fn key_to_string(key: Key) -> String {
    let s = ConfigKey::from_rdev(key)
        .map(|k| k.to_string())
        .unwrap_or_else(|| format!("{:?}", key));
    if s.starts_with("Key") && s.len() == 4 {
        s[3..].to_uppercase()
    } else {
        s
    }
}

// Convert string to ConfigKey then to rdev::Key
fn string_to_key(s: &str) -> Option<Key> {
    let normalized = if s.len() == 1 && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic()) {
        format!("Key{}", s.to_uppercase())
    } else {
        s.to_string()
    };
    normalized.parse::<ConfigKey>().ok().map(|k| k.to_rdev())
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

        // Write to file with pretty formatting
        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        Ok(())
    }

    // Load configuration from JSON file
    fn load_config(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = get_config_path();

        // ensure dir exists
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
                feature.bind_key = sf.bind_key.as_deref().and_then(BindKey::from_string);
                feature.enabled = sf.enabled;
            }
        }

        // Load auto_clicker_delay and position values
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

    fn refresh_presets(&mut self) {
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

    fn save_preset(&self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
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

    fn load_preset(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
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

    fn delete_preset(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let presets_dir = get_presets_dir();
        let path = presets_dir.join(format!("{}.json", name));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

impl AppState {
    fn new() -> Self {
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
                    name: "Gangsta Grip (In multiplayer)".into(),
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
                Feature {
                    id: FeatureId::DoubleClick,
                    name: "Double Click".into(),
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

struct KeyBindApp {
    state: Arc<Mutex<AppState>>,
    prev_ctrl: bool,
    prev_shift: bool,
    prev_alt: bool,
    prev_capslock: bool,
    prev_mouse_mid: bool,
    prev_mouse4: bool,
    prev_mouse5: bool,
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

        // store refs for the mouse hook to use
        let _ = MOUSE_HOOK_TX.set(worker_tx.clone());
        let _ = MOUSE_HOOK_STATE.set(state.clone());
        let _ = MOUSE_HOOK_INPUT.set(input_state.clone());

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

        // bhop loop
        thread::spawn(move || {
            loop {
                let bhop_enabled = input_bhop.is_bhop_active();
                let all_disabled = input_bhop.are_all_macros_disabled();

                if bhop_enabled && is_game_focused() && !all_disabled {
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

        // shift polling loop
        // using winapi since rdev misses modifiers
        let state_clone_mod = state.clone();
        thread::spawn(move || {
            let mut prev_shift = false;
            loop {
                let shift_down = input_mod_poll.is_shift_down();
                let all_disabled = input_mod_poll.are_all_macros_disabled();

                if is_game_focused() && !all_disabled {
                    let Some(s) = safe_lock(&state_clone_mod) else {
                        error!("Failed to lock state in modifier polling thread");
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                        continue;
                    };
                    if shift_down && !prev_shift
                        && let Some(f) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.bind_key == Some(BindKey::Keyboard(Key::ShiftLeft)))
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
                let (enabled, delay_ms, click_method, click_count, mode, key) = {
                    let Some(s) = safe_lock(&state_clone_ac) else {
                        error!("Failed to lock state in autoclicker thread");
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                        continue;
                    };
                    let enabled = s
                        .features
                        .iter()
                        .any(|f| f.id == FeatureId::AutoClicker && f.enabled);
                    (
                        enabled,
                        s.auto_clicker_delay,
                        s.auto_clicker_method,
                        s.auto_clicker_click_count,
                        s.auto_clicker_mode,
                        s.auto_clicker_key,
                    )
                };
                let active = input_ac.is_autoclicker_active();
                let all_disabled = input_ac.are_all_macros_disabled();

                if active && enabled && is_game_focused() && !all_disabled {
                    if input_ac.is_lmb_down() {
                        match mode {
                            AutoClickerMode::Mouse => {
                                match click_method {
                                    ClickMethod::SendInput => send_mouse_click(),
                                    ClickMethod::PostMessage => send_mouse_click_postmessage(click_count),
                                }
                            }
                            AutoClickerMode::Keyboard => {
                                let rdev_k = key.to_rdev();
                                if let Some(scan) = rdev_key_to_scancode(rdev_k) {
                                    send_key_tap(scan);
                                }
                            }
                        }
                        if delay_ms > 0 {
                            thread::sleep(Duration::from_millis(delay_ms as u64));
                        } else {
                            thread::yield_now();
                        }
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
                    s.features.iter().any(|f| f.id == FeatureId::KeepItemClicker && f.enabled)
                };

                let active = input_lmb_hold.is_lmb_hold_active();
                let focused = is_game_focused();
                let all_disabled = input_lmb_hold.are_all_macros_disabled();
                let currently_active = active && enabled && focused && !all_disabled;

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
                    WorkerMessage::HackingClickMtd {
                        x,
                        y,
                        w,
                        h,
                        offset_y,
                    } => {
                        Self::hacking_click_mtd(x, y, w, h, offset_y);
                    }
                    WorkerMessage::HackingJumpMtd {
                        x,
                        y,
                        w,
                        h,
                        offset_y,
                    } => {
                        Self::hacking_jump_mtd(x, y, w, h, offset_y);
                    }
                    WorkerMessage::HackingEscMtd {
                        x,
                        y,
                        w,
                        h,
                        offset_y,
                    } => {
                        Self::hacking_esc_mtd(x, y, w, h, offset_y);
                    }
                    WorkerMessage::TipsSkip { x, w, y } => {
                        Self::tips_skip(x, w, y);
                    }
                    WorkerMessage::Restart { x, w, y } => {
                        Self::restart(x, w, y);
                    }
                    WorkerMessage::FastLoadout => {
                        Self::fast_loadout();
                    }
                    WorkerMessage::HoldItemBug => {
                        Self::hold_item_bug();
                    }
                    WorkerMessage::GangstaGrip { digit } => {
                        Self::gangsta_grip(digit);
                    }

                    WorkerMessage::QuickExit { x, y } => {
                        Self::quick_exit(x, y);
                    }
                    WorkerMessage::DoubleClick(btn) => {
                        Self::double_click(btn);
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
                if let EventType::ButtonRelease(_)
                | EventType::MouseMove { .. }
                | EventType::Wheel { .. } = event.event_type
                {
                    return Some(event);
                }

                if let EventType::ButtonPress(b) = event.event_type {
                    if b == rdev::Button::Left || b == rdev::Button::Right {
                        return Some(event);
                    }
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

                    let matched_key = match event.event_type {
                        EventType::KeyPress(k) => Some(BindKey::Keyboard(k)),
                        EventType::ButtonPress(rdev::Button::Middle) => Some(BindKey::MouseMiddle),
                        EventType::ButtonPress(rdev::Button::Unknown(4)) => Some(BindKey::Mouse4),
                        EventType::ButtonPress(rdev::Button::Unknown(5)) => Some(BindKey::Mouse5),
                        _ => None,
                    };

                    if let Some(key) = matched_key {
                        if let Some(feature) = s
                            .features
                            .iter()
                            .find(|f| f.enabled && f.bind_key == Some(key))
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
                            let hack_esc_y = s.hacking_esc_y_offset;
                            let gun_digit = s.gun_tool_digit;
                            let double_click_btn = s.double_click_button;

                            // Handle toggle features immediately
                            if feature_id == FeatureId::ToggleAllMacros {
                                let _ = input_hotkey.toggle_all_macros_disabled();
                                return None;
                            }

                            if input_hotkey.are_all_macros_disabled() {
                                return Some(event);
                            }

                            if feature_id == FeatureId::AutoClicker {
                                let _ = input_hotkey.toggle_autoclicker();
                                return None;
                            }

                            if feature_id == FeatureId::Bhop {
                                let _ = input_hotkey.toggle_bhop();
                                return None;
                            }

                            if feature_id == FeatureId::KeepItemClicker {
                                let _ = input_hotkey.toggle_lmb_hold();
                                return None;
                            }

                            // Build worker message for other features
                            let action = match feature_id {
                                FeatureId::ShiftToggle => Some(WorkerMessage::ShiftToggle),
                                FeatureId::NoFallDamage => Some(WorkerMessage::NoFallDamage),
                                FeatureId::HackingClickMtd => {
                                    Some(WorkerMessage::HackingClickMtd {
                                        x,
                                        y,
                                        w,
                                        h,
                                        offset_y: hack_y,
                                    })
                                }
                                FeatureId::HackingJumpMtd => {
                                    Some(WorkerMessage::HackingJumpMtd {
                                        x,
                                        y,
                                        w,
                                        h,
                                        offset_y: hack2_y,
                                    })
                                }
                                FeatureId::HackingEscMtd => {
                                    Some(WorkerMessage::HackingEscMtd {
                                        x,
                                        y,
                                        w,
                                        h,
                                        offset_y: hack_esc_y,
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
                                FeatureId::FastLoadout => Some(WorkerMessage::FastLoadout),
                                FeatureId::HoldItemBug => Some(WorkerMessage::HoldItemBug),
                                FeatureId::GangstaGrip => Some(WorkerMessage::GangstaGrip { digit: gun_digit }),
                                FeatureId::QuickExit => Some(WorkerMessage::QuickExit { x, y }),
                                FeatureId::DoubleClick => Some(WorkerMessage::DoubleClick(double_click_btn)),
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

    fn hacking_click_mtd(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
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
            // yield to prevent queue flood
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

    fn hacking_jump_mtd(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
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
            // yield to prevent queue flood
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

    fn hacking_esc_mtd(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
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
                warn!("ScreenToClient failed for hacking method esc");
            }

            // 5. Instant Move
            set_cursor_pos_safe(target_x, target_y);

            // 6. Direct Message Blast
            let l_param = pack_lparam(pt.x, pt.y);

            // CLICK_COUNT = 100, MK_LBUTTON = 0x0001
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
                    "PostMessageA failed {} times in hacking method esc",
                    failed_count
                );
            }

            // 7. ESC twice with 1ms delay
            send_key_tap(0x01);
            thread::sleep(Duration::from_millis(1));
            send_key_tap(0x01);
        }
    }

    fn tips_skip(x: i32, w: i32, y_abs: i32) {
        unsafe {
            let _ = BlockInput(TRUE);
        }
        move_mouse(x + w / 2, y_abs);
        thread::sleep(Duration::from_millis(RESTART_SETTLE_DELAY_MS));
        send_mouse_click();
        unsafe {
            let _ = BlockInput(FALSE);
        }
    }

    fn restart(x: i32, w: i32, y_abs: i32) {
        unsafe {
            let _ = BlockInput(TRUE);
        }
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(RESTART_KEY_DELAY_MS));
        move_mouse(x + w / 2, y_abs);
        thread::sleep(Duration::from_millis(RESTART_SETTLE_DELAY_MS));
        send_mouse_click();
        unsafe {
            let _ = BlockInput(FALSE);
        }
    }

    fn no_fall_damage() {
        send_key_tap(0x01); // ESC
        thread::sleep(Duration::from_millis(NO_FALL_DAMAGE_DELAY_MS));
        send_key_tap(0x01); // ESC
    }

    fn fast_loadout() {
        unsafe {
            // Scroll wheel up (WHEEL_DELTA = 120)
            let mut wheel_input = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
            wheel_input.Anonymous.mi.dwFlags = MOUSEEVENTF_WHEEL;
            wheel_input.Anonymous.mi.mouseData = 120;
            if SendInput(&[wheel_input], std::mem::size_of::<INPUT>() as i32) == 0 {
                error!("SendInput failed in fast_loadout (wheel)");
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

    #[allow(dead_code)]
    fn gangsta_grip(digit: u32) {
        Self::send_mouse_state(true);
        send_key_tap(0x01); // ESC
        Self::send_mouse_state(false);
        // 1=0x02, 2=0x03, 3=0x04
        send_key_tap(digit as u16 + 1);
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

    fn double_click(btn: DoubleClickButton) {
        let (down_flag, up_flag) = match btn {
            DoubleClickButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            DoubleClickButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        };
        unsafe {
            let mut input_down1 = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
            input_down1.Anonymous.mi.dwFlags = down_flag;

            let mut input_up1 = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
            input_up1.Anonymous.mi.dwFlags = up_flag;

            if SendInput(&[input_down1, input_up1], std::mem::size_of::<INPUT>() as i32) == 0 {
                error!("SendInput failed in double_click first tap");
            }

            thread::sleep(Duration::from_millis(20));

            let mut input_down2 = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
            input_down2.Anonymous.mi.dwFlags = down_flag;

            let mut input_up2 = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
            input_up2.Anonymous.mi.dwFlags = up_flag;

            if SendInput(&[input_down2, input_up2], std::mem::size_of::<INPUT>() as i32) == 0 {
                error!("SendInput failed in double_click second tap");
            }
        }
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
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(state_lock) = UPDATE_STATE.get() {
            if let Ok(mut update) = state_lock.lock() {
                if update.has_update && !update.skipped {
                    let mut is_open = true;
                    egui::Window::new("Update Available")
                        .collapsible(false)
                        .resizable(false)
                        .open(&mut is_open)
                        .show(ctx, |ui| {
                            ui.label(format!(
                                "A new update ({}) is available. Do you want to update?",
                                update.new_version
                            ));
                            ui.add_space(8.0);
                            
                            if update.in_progress {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(&update.status);
                                });
                            } else {
                                ui.label(&update.status);
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Update").clicked() {
                                        start_update();
                                    }
                                    if ui.button("Skip").clicked() {
                                        update.skipped = true;
                                    }
                                });
                            }
                        });
                    if !is_open {
                        update.skipped = true;
                    }
                }
            }
        }

        let Some(mut s) = safe_lock(&self.state) else {
            error!("Failed to lock state in UI update");
            return;
        };
        if let Some(idx) = s.features.iter().position(|f| f.selecting) {
            // Spam repaint to detect modifier changes
            ctx.request_repaint_after(Duration::from_millis(16));

            // Check for regular key events
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

            // check mods via winapi (bypasses egui focus block)
            let ctrl_down = unsafe { (GetAsyncKeyState(VK_CONTROL.0 as i32) & 0x8000u16 as i16) != 0 };
            let shift_down = unsafe { (GetAsyncKeyState(VK_SHIFT.0 as i32) & 0x8000u16 as i16) != 0 };
            let alt_down = unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) & 0x8000u16 as i16) != 0 };
            let capslock_down = unsafe { (GetAsyncKeyState(VK_CAPITAL.0 as i32) & 0x8000u16 as i16) != 0 };
            let mouse_mid_down = unsafe { (GetAsyncKeyState(0x04) & 0x8000u16 as i16) != 0 };
            let mouse4_down = unsafe { (GetAsyncKeyState(0x05) & 0x8000u16 as i16) != 0 };
            let mouse5_down = unsafe { (GetAsyncKeyState(0x06) & 0x8000u16 as i16) != 0 };

            let new_bind = if ctrl_down && !self.prev_ctrl {
                Some(BindKey::Keyboard(Key::ControlLeft))
            } else if shift_down && !self.prev_shift {
                Some(BindKey::Keyboard(Key::ShiftLeft))
            } else if alt_down && !self.prev_alt {
                Some(BindKey::Keyboard(Key::Alt))
            } else if capslock_down && !self.prev_capslock {
                Some(BindKey::Keyboard(Key::CapsLock))
            } else if mouse_mid_down && !self.prev_mouse_mid {
                Some(BindKey::MouseMiddle)
            } else if mouse4_down && !self.prev_mouse4 {
                Some(BindKey::Mouse4)
            } else if mouse5_down && !self.prev_mouse5 {
                Some(BindKey::Mouse5)
            } else if let Some(k) = key {
                if k == egui::Key::Escape {
                    s.features[idx].selecting = false;
                    None
                } else {
                    egui_to_rdev_key(k).map(BindKey::Keyboard)
                }
            } else {
                None
            };

            self.prev_ctrl = ctrl_down;
            self.prev_shift = shift_down;
            self.prev_alt = alt_down;
            self.prev_capslock = capslock_down;
            self.prev_mouse_mid = mouse_mid_down;
            self.prev_mouse4 = mouse4_down;
            self.prev_mouse5 = mouse5_down;

            if let Some(bk) = new_bind {
                s.features[idx].bind_key = Some(bk);
                s.features[idx].selecting = false;
                let _ = s.save_config();
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
            let title = "Nztool OAR v2.3.4";
            ui.vertical_centered(|ui| {
                ui.heading(title);
                if InputState.are_all_macros_disabled() {
                    ui.colored_label(egui::Color32::RED, "ALL MACROS DISABLED");
                }
            });
            ui.add_space(5.0);
            ui.separator();
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
                        } else if let Some(k) = s.features[i].bind_key {
                            k.to_string()
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
                            if s.features[i].id == FeatureId::KeepItemClicker {
                                InputState.set_lmb_hold_active(false);
                                send_mouse_hold(false);
                            }
                            s.features[i].bind_key = None;
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
                        if s.features[i].id == FeatureId::KeepItemClicker
                            && InputState.is_lmb_hold_active()
                            && s.features[i].enabled
                        {
                            color = egui::Color32::BLUE;
                        }
                        if s.features[i].id == FeatureId::ToggleAllMacros
                            && InputState.are_all_macros_disabled()
                            && s.features[i].enabled
                        {
                            color = egui::Color32::BLUE;
                        }

                        if ui
                            .add(egui::Button::new("Enable/Disable").fill(color))
                            .clicked()
                            && s.features[i].bind_key.is_some() {
                                s.features[i].enabled = !s.features[i].enabled;
                                if !s.features[i].enabled {
                                    if s.features[i].id == FeatureId::ShiftToggle {
                                        s.release_shift();
                                    }
                                    if s.features[i].id == FeatureId::KeepItemClicker {
                                        InputState.set_lmb_hold_active(false);
                                        send_mouse_hold(false);
                                    }
                                    if s.features[i].id == FeatureId::ToggleAllMacros {
                                        GLOBAL_STATE.all_macros_disabled.store(false, Ordering::SeqCst);
                                    }
                                }
                                let _ = s.save_config();
                            }

                        ui.end_row(); // Move to the next row in the grid
                    }
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
            egui::CollapsingHeader::new("Options").show(ui, |ui| {
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

                // Auto Clicker Mode selection
                ui.horizontal(|ui| {
                    ui.label("Auto Clicker Mode:");
                    let mouse_selected = s.auto_clicker_mode == AutoClickerMode::Mouse;
                    let kb_selected = s.auto_clicker_mode == AutoClickerMode::Keyboard;
                    if ui.selectable_label(mouse_selected, "Mouse").clicked() {
                        s.auto_clicker_mode = AutoClickerMode::Mouse;
                        let _ = s.save_config();
                    }
                    if ui.selectable_label(kb_selected, "Keyboard").clicked() {
                        s.auto_clicker_mode = AutoClickerMode::Keyboard;
                        let _ = s.save_config();
                    }
                });

                if s.auto_clicker_mode == AutoClickerMode::Keyboard {
                    ui.horizontal(|ui| {
                        ui.label("Key to Spam:");
                        let prev_key = s.auto_clicker_key;
                        egui::ComboBox::from_id_salt("ac_key_select")
                            .selected_text(format!("{:?}", s.auto_clicker_key))
                            .show_ui(ui, |ui| {
                                let keys = [
                                    ConfigKey::KeyE,
                                    ConfigKey::KeyF,
                                    ConfigKey::Space,
                                    ConfigKey::KeyQ,
                                    ConfigKey::KeyR,
                                    ConfigKey::KeyC,
                                    ConfigKey::KeyV,
                                    ConfigKey::KeyG,
                                    ConfigKey::Tab,
                                ];
                                for k in keys {
                                    ui.selectable_value(&mut s.auto_clicker_key, k, format!("{:?}", k));
                                }
                            });
                        if s.auto_clicker_key != prev_key {
                            let _ = s.save_config();
                        }
                    });
                }

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

                if s.auto_clicker_mode == AutoClickerMode::Mouse {
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
                }

                ui.add_space(5.0);

                // Double Click Button selection
                ui.horizontal(|ui| {
                    ui.label("Double Click Button:");
                    let left_selected = s.double_click_button == DoubleClickButton::Left;
                    let right_selected = s.double_click_button == DoubleClickButton::Right;
                    if ui.selectable_label(left_selected, "LMB").clicked() {
                        s.double_click_button = DoubleClickButton::Left;
                        let _ = s.save_config();
                    }
                    if ui.selectable_label(right_selected, "RMB").clicked() {
                        s.double_click_button = DoubleClickButton::Right;
                        let _ = s.save_config();
                    }
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

                // Y offset for Hacking Device Click
                ui.horizontal(|ui| {
                    ui.label("Hacking Click Y Offset:");
                    ui.add(egui::DragValue::new(&mut s.hacking_y_offset).speed(1.0));
                });

                // Y offset for Hacking Device Jump
                ui.horizontal(|ui| {
                    ui.label("Hacking Jump Y Offset:");
                    ui.add(egui::DragValue::new(&mut s.hacking2_y_offset).speed(1.0));
                });

                // Y offset for Hacking Device Esc mtd
                ui.horizontal(|ui| {
                    ui.label("Hacking Esc Y Offset:");
                    ui.add(egui::DragValue::new(&mut s.hacking_esc_y_offset).speed(1.0));
                });

                ui.add_space(5.0);


                // Gangsta Grip Digit & Save/Load/Default buttons
                ui.horizontal(|ui| {
                    ui.label("Gangsta Grip Digit:");
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

                    ui.add_space(20.0); // Visual separator

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


                ui.add_space(5.0);
                ui.separator();
                ui.label("Presets");

                // Dropdown of existing presets
                ui.horizontal(|ui| {
                    ui.label("Select Preset:");
                    let mut selected = s.selected_preset.clone();
                    let prev_selected = selected.clone();

                    egui::ComboBox::from_id_salt("preset_select")
                        .selected_text(if selected.is_empty() { "None".to_string() } else { selected.clone() })
                        .show_ui(ui, |ui| {
                            for preset in &s.presets {
                                ui.selectable_value(&mut selected, preset.clone(), preset.clone());
                            }
                        });

                    if selected != prev_selected {
                        s.selected_preset = selected.clone();
                        // Load the preset automatically when selected
                        if let Err(e) = s.load_preset(&selected) {
                            error!("Failed to load preset: {}", e);
                        }
                    }

                    // Delete button (only if a preset is selected)
                    if !s.selected_preset.is_empty() {
                        if ui.button("Delete").clicked() {
                            let to_delete = s.selected_preset.clone();
                            let _ = s.delete_preset(&to_delete);
                            s.selected_preset = String::new();
                            s.refresh_presets();
                        }
                    }
                });

                // Create new preset input and button
                ui.horizontal(|ui| {
                    ui.label("New Preset Name:");
                    ui.add(egui::TextEdit::singleline(&mut s.preset_name_input).desired_width(120.0));
                    if ui.button("Create").clicked() {
                        let name = s.preset_name_input.trim().to_string();
                        if !name.is_empty() {
                            if let Err(e) = s.save_preset(&name) {
                                error!("Failed to save preset: {}", e);
                            } else {
                                s.selected_preset = name;
                                s.preset_name_input = String::new();
                                s.refresh_presets();
                            }
                        }
                    }
                });



                ui.add_space(10.0);
                ui.separator();
                ui.label("Misc");
                ui.horizontal(|ui| {
                    if ui.button("Kill OAR").clicked() {
                        let mut cmd = std::process::Command::new("taskkill");
                        cmd.args(["/IM", "OAR-Win64-Shipping.exe", "/F"]);
                        use std::os::windows::process::CommandExt;
                        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
                        let _ = cmd.spawn();
                    }
                });
            });

            if s.dev_mode {
                ui.add_space(10.0);
                ui.separator();
                ui.heading("Developer Tools");

                // Dev Step Delays sliders


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
    });
}
}

// safe cursor move
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

fn rdev_key_to_scancode(key: Key) -> Option<u16> {
    match key {
        Key::Escape => Some(0x01),
        Key::Num1 => Some(0x02),
        Key::Num2 => Some(0x03),
        Key::Num3 => Some(0x04),
        Key::Num4 => Some(0x05),
        Key::Num5 => Some(0x06),
        Key::Num6 => Some(0x07),
        Key::Num7 => Some(0x08),
        Key::Num8 => Some(0x09),
        Key::Num9 => Some(0x0A),
        Key::Num0 => Some(0x0B),
        Key::Minus => Some(0x0C),
        Key::Equal => Some(0x0D),
        Key::Backspace => Some(0x0E),
        Key::Tab => Some(0x0F),
        Key::KeyQ => Some(0x10),
        Key::KeyW => Some(0x11),
        Key::KeyE => Some(0x12),
        Key::KeyR => Some(0x13),
        Key::KeyT => Some(0x14),
        Key::KeyY => Some(0x15),
        Key::KeyU => Some(0x16),
        Key::KeyI => Some(0x17),
        Key::KeyO => Some(0x18),
        Key::KeyP => Some(0x19),
        Key::KeyA => Some(0x1E),
        Key::KeyS => Some(0x1F),
        Key::KeyD => Some(0x20),
        Key::KeyF => Some(0x21),
        Key::KeyG => Some(0x22),
        Key::KeyH => Some(0x23),
        Key::KeyJ => Some(0x24),
        Key::KeyK => Some(0x25),
        Key::KeyL => Some(0x26),
        Key::KeyZ => Some(0x2C),
        Key::KeyX => Some(0x2D),
        Key::KeyC => Some(0x2E),
        Key::KeyV => Some(0x2F),
        Key::KeyB => Some(0x30),
        Key::KeyN => Some(0x31),
        Key::KeyM => Some(0x32),
        Key::Space => Some(0x39),
        _ => None,
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
        Comma => Some(Key::Comma),
        Period => Some(Key::Dot),
        Slash => Some(Key::Slash),
        Semicolon => Some(Key::SemiColon),
        Quote => Some(Key::Quote),
        OpenBracket => Some(Key::LeftBracket),
        CloseBracket => Some(Key::RightBracket),
        Backslash => Some(Key::BackSlash),
        Minus => Some(Key::Minus),
        Equals => Some(Key::Equal),
        Backtick => Some(Key::BackQuote),
        _ => None,
    }
}

struct UpdateState {
    has_update: bool,
    new_version: String,
    download_url: String,
    skipped: bool,
    in_progress: bool,
    status: String,
}

static UPDATE_STATE: OnceLock<Arc<Mutex<UpdateState>>> = OnceLock::new();

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn is_version_newer(new: &str, current: &str) -> bool {
    let new_parts: Vec<&str> = new.split('.').collect();
    let cur_parts: Vec<&str> = current.split('.').collect();
    for i in 0..std::cmp::max(new_parts.len(), cur_parts.len()) {
        let n = new_parts.get(i).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let c = cur_parts.get(i).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        if n > c {
            return true;
        } else if n < c {
            return false;
        }
    }
    false
}

fn check_for_updates() {
    let state = UPDATE_STATE.get().unwrap().clone();
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
        let current_version = "2.3.4";

        if is_version_newer(clean_tag, current_version) {
            if let Some(asset) = release.assets.iter().find(|a| a.name == "nztool_oar.exe") {
                if let Ok(mut s) = state.lock() {
                    s.has_update = true;
                    s.new_version = release.tag_name.clone();
                    s.download_url = asset.browser_download_url.clone();
                    s.status = format!("New version {} is available!", release.tag_name);
                }
            }
        }
    });
}

fn start_update() {
    let state = UPDATE_STATE.get().unwrap().clone();
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

fn load_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("nz.ico")).unwrap();
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    egui::IconData {
        rgba: rgba.into_vec(),
        width,
        height,
    }
}

fn main() -> eframe::Result {
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));

    // Clean up old executable if it exists
    if let Ok(mut exe_path) = std::env::current_exe() {
        exe_path.set_extension("exe.old");
        if exe_path.exists() {
            let _ = std::fs::remove_file(exe_path);
        }
    }

    // Initialize update state
    let _ = UPDATE_STATE.set(Arc::new(Mutex::new(UpdateState {
        has_update: false,
        new_version: String::new(),
        download_url: String::new(),
        skipped: false,
        in_progress: false,
        status: String::new(),
    })));

    // Trigger background update check
    check_for_updates();

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([450.0, 550.0])
            .with_icon(load_icon()),
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
                prev_mouse_mid: false,
                prev_mouse4: false,
                prev_mouse5: false,
            }))
        }),
    )
}






