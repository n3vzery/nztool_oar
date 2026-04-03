#![windows_subsystem = "windows"]

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::thread;
use std::time::Duration;
use windows::Win32::System::Threading::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::HiDpi::*;

// Global runtime state using Atomics for lock-free access in hooks
static REAL_LMB_DOWN: AtomicBool = AtomicBool::new(false);
static AUTOCLICKER_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE_HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// Must match `AppState::new().features.len()` and the order of variants in `feature_id_for_slot`.
const FEATURE_COUNT: usize = 7;
/// Last slot must stay `FeatureId::AutoClicker` (used by the autoclicker thread).
const AUTOCLICKER_FEATURE_IDX: usize = FEATURE_COUNT - 1;

// Feature runtime config (VK, enabled, block key to game). Order matches `feature_id_for_slot`.
static FEATURE_VKS: [AtomicU32; FEATURE_COUNT] = [
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0),
];
static FEATURE_ENABLED: [AtomicBool; FEATURE_COUNT] = [
    AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false),
    AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false),
    AtomicBool::new(false),
];
/// When true, the hook returns handled and the key does not reach the game (default).
static FEATURE_BLOCK_KEY: [AtomicBool; FEATURE_COUNT] = [
    AtomicBool::new(true), AtomicBool::new(true), AtomicBool::new(true),
    AtomicBool::new(true), AtomicBool::new(true), AtomicBool::new(true),
    AtomicBool::new(true),
];

static MONITOR_X: AtomicI32 = AtomicI32::new(0);
static MONITOR_Y: AtomicI32 = AtomicI32::new(0);
static MONITOR_W: AtomicI32 = AtomicI32::new(0);
static MONITOR_H: AtomicI32 = AtomicI32::new(0);

/// Target left-shift held state applied via `SendInput` (hook/UI stay lock-free here).
static SHIFT_HELD_TARGET: AtomicBool = AtomicBool::new(false);

static HACKING_TARGET_OFFSET_Y: AtomicI32 = AtomicI32::new(140);
static TIPS_SKIP_Y_RATIO_BITS: AtomicU32 = AtomicU32::new(0.75f32.to_bits());
static RESTART_Y_RATIO_BITS: AtomicU32 = AtomicU32::new(0.45f32.to_bits());

static FOCUS_CACHE: Mutex<Option<(isize, bool)>> = Mutex::new(None);

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("fast_clicker"))
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

static HOOK_INSTALL_STATE: AtomicU8 = AtomicU8::new(0);
static HOOK_LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn set_hook_status(ok_mouse: bool, ok_kbd: bool, detail: impl Into<String>) {
    let st = match (ok_mouse, ok_kbd) {
        (true, true) => 0u8,
        (false, false) => 2u8,
        _ => 1u8,
    };
    HOOK_INSTALL_STATE.store(st, Ordering::SeqCst);
    if let Ok(mut g) = HOOK_LAST_ERROR.lock() {
        *g = detail.into();
    }
}

#[derive(Clone, Copy)]
enum MacroJob {
    Hacking,
    Hacking2,
    TipsSkip,
    Restart,
    NoFallDamage,
}

static MACRO_TX: OnceLock<SyncSender<MacroJob>> = OnceLock::new();
static MACRO_DROPPED: AtomicU64 = AtomicU64::new(0);

fn enqueue_macro(job: MacroJob) {
    let Some(tx) = MACRO_TX.get() else { return };
    match tx.try_send(job) {
        Err(TrySendError::Full(_)) => {
            MACRO_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {}
        Ok(()) => {}
    }
}

#[inline]
fn atomic_load_f32_bits(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::SeqCst))
}

fn probe_foreground_is_oar(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.0.is_null() {
            return false;
        }
        let mut process_id = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if process_id == 0 {
            return false;
        }
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, process_id);
        if let Ok(h) = handle {
            let mut buffer = [0u16; 260];
            let len = K32GetModuleBaseNameW(h, None, &mut buffer);
            let _ = CloseHandle(h);
            if len > 0 {
                let name = String::from_utf16_lossy(&buffer[..len as usize]);
                return name.eq_ignore_ascii_case("OAR-Win64-Shipping.exe");
            }
        }
        false
    }
}

fn is_game_focused() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let key = hwnd.0 as isize;
        if let Ok(mut guard) = FOCUS_CACHE.lock() {
            if let Some((cached_hwnd, ok)) = *guard {
                if cached_hwnd == key {
                    return ok;
                }
            }
            let ok = probe_foreground_is_oar(hwnd);
            *guard = Some((key, ok));
            ok
        } else {
            probe_foreground_is_oar(hwnd)
        }
    }
}

#[inline]
fn feature_id_for_slot(i: usize) -> FeatureId {
    match i {
        0 => FeatureId::HackingPostMessage,
        1 => FeatureId::HackingPostMessage2,
        2 => FeatureId::TipsSkip,
        3 => FeatureId::Restart,
        4 => FeatureId::NoFallDamage,
        5 => FeatureId::ShiftToggle,
        6 => FeatureId::AutoClicker,
        _ => unreachable!("slot past FEATURE_COUNT"),
    }
}

fn runtime_fingerprint(s: &AppState) -> u64 {
    let mut h = DefaultHasher::new();
    s.monitor_id.hash(&mut h);
    s.x_offset.hash(&mut h);
    s.y_offset.hash(&mut h);
    s.width.hash(&mut h);
    s.height.hash(&mut h);
    s.tips_skip_y_ratio.to_bits().hash(&mut h);
    s.restart_y_ratio.to_bits().hash(&mut h);
    s.hacking_target_offset_y.hash(&mut h);
    for f in &s.features {
        f.vk.hash(&mut h);
        f.enabled.hash(&mut h);
        f.block_key.hash(&mut h);
    }
    SHIFT_HELD_TARGET.load(Ordering::SeqCst).hash(&mut h);
    h.finish()
}

fn apply_shift_from_target() {
    let down = SHIFT_HELD_TARGET.load(Ordering::SeqCst);
    send_key_state(0x2A, down);
}

// Helper to pack coordinates into LPARAM safely for PostMessage
fn pack_lparam(x: i32, y: i32) -> isize {
    let low = (x as u32 & 0xFFFF) as isize;
    let high = ((y as u32 & 0xFFFF) << 16) as isize;
    high | low
}

fn post_left_click_pair(hwnd: HWND, l_param: LPARAM) -> bool {
    unsafe {
        PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), l_param).is_ok()
            && PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), l_param).is_ok()
    }
}

unsafe extern "system" fn low_level_mouse_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code == HC_ACTION as i32 {
        unsafe {
            let ms_ll = *(l_param.0 as *const MSLLHOOKSTRUCT);
            if (ms_ll.flags & LLMHF_INJECTED) == 0 {
                match w_param.0 as u32 {
                    WM_LBUTTONDOWN => REAL_LMB_DOWN.store(true, Ordering::SeqCst),
                    WM_LBUTTONUP => REAL_LMB_DOWN.store(false, Ordering::SeqCst),
                    _ => {}
                }
            }
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), n_code, w_param, l_param) }
}

unsafe extern "system" fn low_level_kbd_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code == HC_ACTION as i32 {
        unsafe {
            let kbd = *(l_param.0 as *const KBDLLHOOKSTRUCT);
            // LLKHF_INJECTED: ignore synthesized key events so we don't recurse on SendInput
            if !kbd.flags.contains(LLKHF_INJECTED) && w_param.0 as u32 == WM_KEYDOWN {
                if is_game_focused() {
                    let vk = kbd.vkCode;
                    for i in 0..FEATURE_COUNT {
                        if FEATURE_ENABLED[i].load(Ordering::SeqCst) && FEATURE_VKS[i].load(Ordering::SeqCst) == vk {
                            let feature_id = feature_id_for_slot(i);
                            let block_key = FEATURE_BLOCK_KEY[i].load(Ordering::SeqCst);

                            if feature_id == FeatureId::AutoClicker {
                                let current = AUTOCLICKER_ACTIVE.load(Ordering::SeqCst);
                                AUTOCLICKER_ACTIVE.store(!current, Ordering::SeqCst);
                            } else {
                                match feature_id {
                                    FeatureId::AutoClicker => unreachable!("handled above"),
                                    FeatureId::ShiftToggle => {
                                        let next = !SHIFT_HELD_TARGET.load(Ordering::SeqCst);
                                        SHIFT_HELD_TARGET.store(next, Ordering::SeqCst);
                                        thread::spawn(|| apply_shift_from_target());
                                    }
                                    FeatureId::NoFallDamage => enqueue_macro(MacroJob::NoFallDamage),
                                    FeatureId::HackingPostMessage => enqueue_macro(MacroJob::Hacking),
                                    FeatureId::HackingPostMessage2 => enqueue_macro(MacroJob::Hacking2),
                                    FeatureId::TipsSkip => enqueue_macro(MacroJob::TipsSkip),
                                    FeatureId::Restart => enqueue_macro(MacroJob::Restart),
                                }
                            }
                            return if block_key {
                                LRESULT(1)
                            } else {
                                CallNextHookEx(HHOOK::default(), n_code, w_param, l_param)
                            };
                        }
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), n_code, w_param, l_param) }
}

#[derive(PartialEq, Clone, Copy)]
enum FeatureId {
    HackingPostMessage,
    HackingPostMessage2,
    TipsSkip,
    Restart,
    NoFallDamage,
    ShiftToggle,
    AutoClicker,
}

struct Feature {
    id: FeatureId,
    name: String,
    vk: Option<u32>,
    enabled: bool,
    /// When true (default), the hotkey is not delivered to the game. Disable to keep movement etc.
    block_key: bool,
    selecting: bool,
}

struct AppState {
    features: Vec<Feature>,
    monitor_id: String,
    x_offset: i32,
    y_offset: i32,
    width: i32,
    height: i32,
    /// Fraction of monitor height for tips-skip Y (0–1).
    tips_skip_y_ratio: f32,
    /// Fraction of monitor height for restart-menu Y (0–1).
    restart_y_ratio: f32,
    /// Pixels below vertical center for hacking PostMessage aim.
    hacking_target_offset_y: i32,
}

impl AppState {
    fn new() -> Self {
        let mut state = Self {
            features: vec![
                Feature { id: FeatureId::HackingPostMessage, name: "Hacking Device (PostMessage)".into(), vk: None, enabled: false, block_key: true, selecting: false },
                Feature { id: FeatureId::HackingPostMessage2, name: "Hacking Device (PostMessage 2)".into(), vk: None, enabled: false, block_key: true, selecting: false },
                Feature { id: FeatureId::TipsSkip, name: "Tips Skip".into(), vk: None, enabled: false, block_key: true, selecting: false },
                Feature { id: FeatureId::Restart, name: "Restart".into(), vk: None, enabled: false, block_key: true, selecting: false },
                Feature { id: FeatureId::NoFallDamage, name: "No Fall Damage".into(), vk: None, enabled: false, block_key: true, selecting: false },
                Feature { id: FeatureId::ShiftToggle, name: "Shift Toggle".into(), vk: None, enabled: false, block_key: true, selecting: false },
                Feature { id: FeatureId::AutoClicker, name: "Auto Clicker".into(), vk: None, enabled: false, block_key: true, selecting: false },
            ],
            monitor_id: "1".into(),
            x_offset: 0,
            y_offset: 0,
            width: 0,
            height: 0,
            tips_skip_y_ratio: 0.75,
            restart_y_ratio: 0.45,
            hacking_target_offset_y: 140,
        };
        debug_assert_eq!(state.features.len(), FEATURE_COUNT);
        state.update_screen_position();
        state
    }

    fn sync_to_runtime(&self) {
        MONITOR_X.store(self.x_offset, Ordering::SeqCst);
        MONITOR_Y.store(self.y_offset, Ordering::SeqCst);
        MONITOR_W.store(self.width, Ordering::SeqCst);
        MONITOR_H.store(self.height, Ordering::SeqCst);
        TIPS_SKIP_Y_RATIO_BITS.store(self.tips_skip_y_ratio.to_bits(), Ordering::SeqCst);
        RESTART_Y_RATIO_BITS.store(self.restart_y_ratio.to_bits(), Ordering::SeqCst);
        HACKING_TARGET_OFFSET_Y.store(self.hacking_target_offset_y, Ordering::SeqCst);
        for (i, feature) in self.features.iter().enumerate().take(FEATURE_COUNT) {
            FEATURE_VKS[i].store(feature.vk.unwrap_or(0), Ordering::SeqCst);
            FEATURE_ENABLED[i].store(feature.enabled, Ordering::SeqCst);
            FEATURE_BLOCK_KEY[i].store(feature.block_key, Ordering::SeqCst);
        }
    }

    fn update_screen_position(&mut self) {
        let mon_id: i32 = self.monitor_id.parse().unwrap_or(1);
        unsafe {
            let mut monitors: Vec<MONITORINFO> = Vec::new();
            unsafe extern "system" fn enum_monitor_callback(h_monitor: HMONITOR, _: HDC, _: *mut RECT, dw_data: LPARAM) -> BOOL {
                unsafe {
                    let monitors = &mut *(dw_data.0 as *mut Vec<MONITORINFO>);
                    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                    if GetMonitorInfoW(h_monitor, &mut info).as_bool() { monitors.push(info); }
                    TRUE
                }
            }
            let _ = EnumDisplayMonitors(None, None, Some(enum_monitor_callback), LPARAM(&mut monitors as *mut Vec<MONITORINFO> as isize));
            if let Some(mon) = monitors.get((mon_id - 1).max(0) as usize) {
                self.x_offset = mon.rcMonitor.left;
                self.y_offset = mon.rcMonitor.top;
                self.width = mon.rcMonitor.right - mon.rcMonitor.left;
                self.height = mon.rcMonitor.bottom - mon.rcMonitor.top;
            }
        }
    }

    fn release_shift(&mut self) {
        if SHIFT_HELD_TARGET.swap(false, Ordering::SeqCst) {
            send_key_state(0x2A, false);
        }
    }

    fn apply_user_config(&mut self, c: UserConfig) {
        if !c.monitor_id.is_empty() {
            self.monitor_id = c.monitor_id;
        }
        for (i, row) in c.features.iter().enumerate().take(FEATURE_COUNT) {
            if i < self.features.len() {
                self.features[i].vk = row.vk;
                self.features[i].enabled = row.enabled;
                self.features[i].block_key = row.block_key;
            }
        }
        self.tips_skip_y_ratio = c.tips_skip_y_ratio.clamp(0.0, 1.0);
        self.restart_y_ratio = c.restart_y_ratio.clamp(0.0, 1.0);
        self.hacking_target_offset_y = c.hacking_target_offset_y.clamp(0, 2160);
        self.update_screen_position();
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct FeatureSaved {
    vk: Option<u32>,
    enabled: bool,
    block_key: bool,
}

impl Default for FeatureSaved {
    fn default() -> Self {
        Self {
            vk: None,
            enabled: false,
            block_key: true,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct UserConfig {
    monitor_id: String,
    features: Vec<FeatureSaved>,
    tips_skip_y_ratio: f32,
    restart_y_ratio: f32,
    hacking_target_offset_y: i32,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            monitor_id: "1".into(),
            features: Vec::new(),
            tips_skip_y_ratio: default_tips_ratio(),
            restart_y_ratio: default_restart_ratio(),
            hacking_target_offset_y: default_hack_off(),
        }
    }
}

fn default_tips_ratio() -> f32 {
    0.75
}

fn default_restart_ratio() -> f32 {
    0.45
}

fn default_hack_off() -> i32 {
    140
}

fn load_user_config() -> Option<UserConfig> {
    let path = config_path()?;
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_user_config(state: &AppState) -> Result<(), String> {
    let Some(dir) = config_dir() else {
        return Err("APPDATA not set".into());
    };
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let Some(path) = config_path() else {
        return Err("config path".into());
    };
    let features: Vec<FeatureSaved> = state
        .features
        .iter()
        .take(FEATURE_COUNT)
        .map(|f| FeatureSaved {
            vk: f.vk,
            enabled: f.enabled,
            block_key: f.block_key,
        })
        .collect();
    let cfg = UserConfig {
        monitor_id: state.monitor_id.clone(),
        features,
        tips_skip_y_ratio: state.tips_skip_y_ratio,
        restart_y_ratio: state.restart_y_ratio,
        hacking_target_offset_y: state.hacking_target_offset_y,
    };
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

struct KeyBindApp {
    state: Arc<Mutex<AppState>>,
    last_runtime_fingerprint: u64,
    last_save_error: Option<String>,
}

impl Drop for KeyBindApp {
    fn drop(&mut self) {
        if let Ok(s) = self.state.lock() {
            let _ = save_user_config(&s);
        }
        let thread_id = MOUSE_HOOK_THREAD_ID.load(Ordering::SeqCst);
        if thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageA(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl KeyBindApp {
    fn start_hotkey_listener(_state: Arc<Mutex<AppState>>) {
        let (macro_tx, macro_rx) = sync_channel(1);
        let _ = MACRO_TX.set(macro_tx);
        thread::spawn(move || KeyBindApp::macro_worker_loop(macro_rx));

        // Single message loop: WH_MOUSE_LL (physical LMB for autoclicker) + WH_KEYBOARD_LL (hotkeys).
        thread::spawn(|| {
            unsafe {
                MOUSE_HOOK_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);

                let mouse_hook = SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(low_level_mouse_proc),
                    HINSTANCE::default(),
                    0,
                );
                let kbd_hook = SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(low_level_kbd_proc),
                    HINSTANCE::default(),
                    0,
                );

                match (mouse_hook, kbd_hook) {
                    (Ok(mh), Ok(kh)) => {
                        set_hook_status(true, true, "");
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                        let _ = UnhookWindowsHookEx(mh);
                        let _ = UnhookWindowsHookEx(kh);
                    }
                    (Ok(mh), Err(e)) => {
                        let _ = UnhookWindowsHookEx(mh);
                        let msg = format!("keyboard hook failed: {:?}", e);
                        set_hook_status(true, false, msg.clone());
                        eprintln!("SetWindowsHookExW(WH_KEYBOARD_LL): {}. Hotkeys disabled.", msg);
                    }
                    (Err(e), Ok(kh)) => {
                        let _ = UnhookWindowsHookEx(kh);
                        let msg = format!("mouse hook failed: {:?}", e);
                        set_hook_status(false, true, msg.clone());
                        eprintln!("SetWindowsHookExW(WH_MOUSE_LL): {}.", msg);
                    }
                    (Err(e1), Err(e2)) => {
                        let msg = format!("mouse: {:?}; keyboard: {:?}", e1, e2);
                        set_hook_status(false, false, msg.clone());
                        eprintln!("SetWindowsHookExW: {}.", msg);
                    }
                }
            }
        });

        thread::spawn(|| {
            loop {
                let enabled = FEATURE_ENABLED[AUTOCLICKER_FEATURE_IDX].load(Ordering::SeqCst);
                let active = AUTOCLICKER_ACTIVE.load(Ordering::SeqCst);

                if active && enabled && is_game_focused() {
                    if REAL_LMB_DOWN.load(Ordering::SeqCst) {
                        send_mouse_click();
                        thread::sleep(Duration::from_millis(30));
                    } else {
                        thread::sleep(Duration::from_millis(5));
                    }
                } else {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        });
    }

    fn hacking_method_post_message(x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            // 1. Wait (DELAY_MS = 50)
            thread::sleep(Duration::from_millis(50));

            // 2. Get game window
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }

            // 3. Screen center calculation
            let center_x = x + w / 2;
            let center_y = y + h / 2;
            let target_x = center_x;
            let off_y = HACKING_TARGET_OFFSET_Y.load(Ordering::SeqCst);
            let target_y = center_y - off_y;

            // 4. Convert Screen to Client Coordinates for PostMessage
            let mut pt = POINT { x: target_x, y: target_y };
            let _ = ScreenToClient(hwnd, &mut pt);

            // 5. Instant Move (Still screen-relative)
            let _ = SetCursorPos(target_x, target_y);

            // 6. Direct Message Blast (Packed safely)
            let l_param = pack_lparam(pt.x, pt.y);
            let lp = LPARAM(l_param);
            for _ in 0..100 {
                if !post_left_click_pair(hwnd, lp) {
                    eprintln!("fast_clicker: PostMessage click burst aborted (PostMessageA failed).");
                    break;
                }
                thread::sleep(Duration::from_micros(400));
            }

            // 7. Move to top and click once
            let _ = SetCursorPos(center_x, y);
            send_mouse_click();
        }
    }

    fn hacking_method2(x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            // 1. Wait (DELAY_MS = 50)
            thread::sleep(Duration::from_millis(50));

            // 2. Get game window
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }

            // 3. Screen center calculation
            let center_x = x + w / 2;
            let center_y = y + h / 2;
            let target_x = center_x;
            let off_y = HACKING_TARGET_OFFSET_Y.load(Ordering::SeqCst);
            let target_y = center_y - off_y;

            // 4. Convert Screen to Client Coordinates
            let mut pt = POINT { x: target_x, y: target_y };
            let _ = ScreenToClient(hwnd, &mut pt);

            // 5. Instant Move
            let _ = SetCursorPos(target_x, target_y);

            // 6. Direct Message Blast
            let l_param = pack_lparam(pt.x, pt.y);
            let lp = LPARAM(l_param);
            for _ in 0..100 {
                if !post_left_click_pair(hwnd, lp) {
                    eprintln!("fast_clicker: PostMessage click burst aborted (PostMessageA failed).");
                    break;
                }
                thread::sleep(Duration::from_micros(400));
            }

            // 7. Jump (Space scancode = 0x39)
            send_key_tap(0x39);
        }
    }

    fn tips_skip(x: i32, y: i32, w: i32, h: i32) {
        let r = atomic_load_f32_bits(&TIPS_SKIP_Y_RATIO_BITS).clamp(0.0, 1.0);
        let dy = (h as f32 * r) as i32;
        move_mouse(x + w / 2, y + dy);
        send_mouse_click();
    }

    fn restart(x: i32, y: i32, w: i32, h: i32) {
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(100));
        let r = atomic_load_f32_bits(&RESTART_Y_RATIO_BITS).clamp(0.0, 1.0);
        let dy = (h as f32 * r) as i32;
        move_mouse(x + w / 2, y + dy);
        send_mouse_click();
    }

    fn no_fall_damage() {
        send_key_tap(0x01); // ESC
        thread::sleep(Duration::from_millis(30));
        send_key_tap(0x01); // ESC
    }

    fn macro_worker_loop(rx: std::sync::mpsc::Receiver<MacroJob>) {
        while let Ok(job) = rx.recv() {
            let x = MONITOR_X.load(Ordering::SeqCst);
            let y = MONITOR_Y.load(Ordering::SeqCst);
            let w = MONITOR_W.load(Ordering::SeqCst);
            let h = MONITOR_H.load(Ordering::SeqCst);
            match job {
                MacroJob::Hacking => Self::hacking_method_post_message(x, y, w, h),
                MacroJob::Hacking2 => Self::hacking_method2(x, y, w, h),
                MacroJob::TipsSkip => Self::tips_skip(x, y, w, h),
                MacroJob::Restart => Self::restart(x, y, w, h),
                MacroJob::NoFallDamage => Self::no_fall_damage(),
            }
        }
    }
}

impl eframe::App for KeyBindApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let mut s = self.state.lock().unwrap();
        let fp = runtime_fingerprint(&s);
        if fp != self.last_runtime_fingerprint {
            s.sync_to_runtime();
            self.last_runtime_fingerprint = fp;
            self.last_save_error = save_user_config(&s).err();
        }
        if let Some(idx) = s.features.iter().position(|f| f.selecting) {
            let key = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Key { key, pressed: true, .. } = e { Some(*key) } else { None }
                })
            });

            if let Some(k) = key {
                        if k == egui::Key::Escape {
                            s.features[idx].selecting = false;
                        } else {
                            if let Some(vk) = egui_to_vk(k) {
                                s.features[idx].vk = Some(vk);
                                s.features[idx].selecting = false;
                            }
                        }
                    }
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Key Bindings App (Rust OAR Helper)");
            ui.add_space(6.0);

            let hook_st = HOOK_INSTALL_STATE.load(Ordering::Relaxed);
            let hook_detail = HOOK_LAST_ERROR
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match hook_st {
                0 => {
                    ui.label(egui::RichText::new("Hooks: OK").color(egui::Color32::from_rgb(0, 140, 60)));
                }
                1 => {
                    ui.label(
                        egui::RichText::new(format!("Hooks: partial — {}", hook_detail))
                            .color(egui::Color32::from_rgb(200, 120, 0)),
                    );
                }
                _ => {
                    ui.label(
                        egui::RichText::new(format!("Hooks: failed — {}", hook_detail))
                            .color(egui::Color32::from_rgb(180, 0, 0)),
                    );
                }
            }
            let dropped = MACRO_DROPPED.load(Ordering::Relaxed);
            if dropped > 0 {
                ui.label(format!(
                    "Macros skipped (hotkey spam / busy queue): {}",
                    dropped
                ));
            }
            if let Some(ref err) = self.last_save_error {
                ui.colored_label(egui::Color32::RED, format!("Config save error: {}", err));
            }
            if let Some(ref p) = config_path() {
                ui.label(format!("Config file: {}", p.display()));
            }

            ui.add_space(8.0);

            egui::Grid::new("features_grid")
                .num_columns(5)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    for i in 0..s.features.len() {
                        ui.label(format!("{}:", s.features[i].name));

                        let key_text = if s.features[i].selecting { "Waiting...".into() }
                                       else if let Some(vk) = s.features[i].vk { format!("VK 0x{:02X}", vk) }
                                       else { "Select Key".into() };

                        if ui.button(key_text).clicked() { s.features[i].selecting = true; }

                        ui.checkbox(&mut s.features[i].block_key, "Block game key")
                            .on_hover_text("Off: macro runs but the key still reaches the game (e.g. WASD).");

                        if ui.button("Reset").clicked() {
                            if s.features[i].id == FeatureId::ShiftToggle { s.release_shift(); }
                            s.features[i].vk = None;
                            s.features[i].enabled = false;
                            s.features[i].block_key = true;
                            s.features[i].selecting = false;
                        }

                        let shift_on = SHIFT_HELD_TARGET.load(Ordering::SeqCst);
                        let mut color = if s.features[i].enabled { egui::Color32::from_rgb(0, 150, 0) }
                                        else { egui::Color32::from_rgb(150, 0, 0) };
                        if s.features[i].id == FeatureId::ShiftToggle && shift_on && s.features[i].enabled {
                            color = egui::Color32::BLUE;
                        }

                        if ui.add(egui::Button::new("Enable/Disable").fill(color)).clicked() {
                            if s.features[i].vk.is_some() {
                                s.features[i].enabled = !s.features[i].enabled;
                                if !s.features[i].enabled && s.features[i].id == FeatureId::ShiftToggle { s.release_shift(); }
                            }
                        }

                        ui.end_row();
                    }
                });

            ui.add_space(12.0);
            ui.collapsing("Screen / UI tuning", |ui| {
                ui.add(
                    egui::Slider::new(&mut s.tips_skip_y_ratio, 0.0..=1.0)
                        .text("Tips skip — Y as fraction of monitor height"),
                );
                ui.add(
                    egui::Slider::new(&mut s.restart_y_ratio, 0.0..=1.0)
                        .text("Restart — Y as fraction of monitor height"),
                );
                ui.add(
                    egui::Slider::new(&mut s.hacking_target_offset_y, 0..=600)
                        .text("Hacking aim — pixels below screen center"),
                );
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label("Monitor ID:");
                if ui.text_edit_singleline(&mut s.monitor_id).changed() { s.update_screen_position(); }
            });
            ui.add_space(5.0);
            ui.label(format!("Monitor Pos: {}x{}, Size: {}x{}", s.x_offset, s.y_offset, s.width, s.height));
            if SHIFT_HELD_TARGET.load(Ordering::SeqCst) {
                ui.colored_label(egui::Color32::LIGHT_BLUE, "SHIFT IS CURRENTLY HELD");
            }
        });
    }
}

fn move_mouse(x: i32, y: i32) { unsafe { let _ = SetCursorPos(x, y); } }

fn send_mouse_click() { send_instant_burst_clicks(1); }

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
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
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
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_key_state(scan: u16, down: bool) {
    unsafe {
        let mut input = INPUT::default();
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki.wScan = scan;
        input.Anonymous.ki.dwFlags = if down { KEYEVENTF_SCANCODE } else { KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP };
        let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn egui_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    match key {
        A => Some(0x41), B => Some(0x42), C => Some(0x43), D => Some(0x44),
        E => Some(0x45), F => Some(0x46), G => Some(0x47), H => Some(0x48),
        I => Some(0x49), J => Some(0x4A), K => Some(0x4B), L => Some(0x4C),
        M => Some(0x4D), N => Some(0x4E), O => Some(0x4F), P => Some(0x50),
        Q => Some(0x51), R => Some(0x52), S => Some(0x53), T => Some(0x54),
        U => Some(0x55), V => Some(0x56), W => Some(0x57), X => Some(0x58),
        Y => Some(0x59), Z => Some(0x5A),
        Num0 => Some(0x30), Num1 => Some(0x31), Num2 => Some(0x32), Num3 => Some(0x33),
        Num4 => Some(0x34), Num5 => Some(0x35), Num6 => Some(0x36), Num7 => Some(0x37),
        Num8 => Some(0x38), Num9 => Some(0x39),
        F1 => Some(0x70), F2 => Some(0x71), F3 => Some(0x72), F4 => Some(0x73),
        F5 => Some(0x74), F6 => Some(0x75), F7 => Some(0x76), F8 => Some(0x77),
        F9 => Some(0x78), F10 => Some(0x79), F11 => Some(0x7A), F12 => Some(0x7B),
        Space => Some(0x20), Enter => Some(0x0D), Escape => Some(0x1B),
        Tab => Some(0x09), Backspace => Some(0x08), Insert => Some(0x2D),
        Delete => Some(0x2E), Home => Some(0x24), End => Some(0x23),
        PageUp => Some(0x21), PageDown => Some(0x22),
        ArrowUp => Some(0x26), ArrowDown => Some(0x28),
        ArrowLeft => Some(0x25), ArrowRight => Some(0x27),
        _ => None,
    }
}

fn main() -> eframe::Result {
    unsafe { let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([580.0, 520.0]),
        ..Default::default()
    };
    let mut app_state = AppState::new();
    if let Some(cfg) = load_user_config() {
        app_state.apply_user_config(cfg);
    }
    let state = Arc::new(Mutex::new(app_state));
    state.lock().unwrap().sync_to_runtime();
    KeyBindApp::start_hotkey_listener(state.clone());
    let fp = runtime_fingerprint(&state.lock().unwrap());
    eframe::run_native(
        "Key Bindings App",
        options,
        Box::new(move |_| {
            Ok(Box::new(KeyBindApp {
                state,
                last_runtime_fingerprint: fp,
                last_save_error: None,
            }))
        }),
    )
}
