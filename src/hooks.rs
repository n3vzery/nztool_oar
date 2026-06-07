use crate::features::{BindKey, DoubleClickButton, FeatureId};
use crate::state::{
    AppState, AutoClickerMode, ClickMethod, GLOBAL_STATE, InputState, is_game_focused, safe_lock,
};
use crate::worker::{BHOP_TAP_INTERVAL_MS, POLL_INTERVAL_MS, Worker, WorkerMessage};
use log::{error, info};
use rdev::{Event, EventType, Key, grab};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// globals for mouse hook to dispatch xbutton events
pub static MOUSE_HOOK_TX: OnceLock<mpsc::Sender<WorkerMessage>> = OnceLock::new();
pub static MOUSE_HOOK_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();
pub static MOUSE_HOOK_INPUT: OnceLock<InputState> = OnceLock::new();

pub unsafe extern "system" fn low_level_mouse_proc(
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
                                s.features
                                    .iter()
                                    .any(|f| f.enabled && f.bind_key == Some(bind_key))
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };

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
pub fn handle_mouse_bind(bind_key: BindKey) {
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

    let Some(feature) = s
        .features
        .iter()
        .find(|f| f.enabled && f.bind_key == Some(bind_key))
    else {
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
        FeatureId::HackingClickMtd => Some(WorkerMessage::HackingClickMtd {
            x,
            y,
            w,
            h,
            offset_y: hack_y,
        }),
        FeatureId::HackingJumpMtd => Some(WorkerMessage::HackingJumpMtd {
            x,
            y,
            w,
            h,
            offset_y: hack2_y,
        }),
        FeatureId::HackingEscMtd => Some(WorkerMessage::HackingEscMtd {
            x,
            y,
            w,
            h,
            offset_y: hack_esc_y,
        }),
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

    if let Some(m) = msg {
        let _ = tx.send(m);
    }
}

pub unsafe extern "system" fn low_level_keyboard_proc(
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

pub fn start_hotkey_listener(state: Arc<Mutex<AppState>>, input_state: InputState) {
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
            input_mouse
                .set_mouse_hook_thread_id(windows::Win32::System::Threading::GetCurrentThreadId());

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
            input_keyboard.set_keyboard_hook_thread_id(
                windows::Win32::System::Threading::GetCurrentThreadId(),
            );

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
                    crate::worker::send_key_tap(0x39); // Space key
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
                if shift_down
                    && !prev_shift
                    && let Some(f) = s.features.iter().find(|f| {
                        f.enabled && f.bind_key == Some(BindKey::Keyboard(Key::ShiftLeft))
                    })
                    && f.id == FeatureId::ShiftToggle
                {
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
                        AutoClickerMode::Mouse => match click_method {
                            ClickMethod::SendInput => crate::worker::send_mouse_click(),
                            ClickMethod::PostMessage => {
                                crate::worker::send_mouse_click_postmessage(click_count)
                            }
                        },
                        AutoClickerMode::Keyboard => {
                            let rdev_k = key.to_rdev();
                            if let Some(scan) = crate::worker::rdev_key_to_scancode(rdev_k) {
                                crate::worker::send_key_tap(scan);
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
                s.features
                    .iter()
                    .any(|f| f.id == FeatureId::KeepItemClicker && f.enabled)
            };

            let active = input_lmb_hold.is_lmb_hold_active();
            let focused = is_game_focused();
            let all_disabled = input_lmb_hold.are_all_macros_disabled();
            let currently_active = active && enabled && focused && !all_disabled;

            if currently_active {
                crate::worker::send_mouse_hold(true);
                was_active_and_focused = true;
                thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            } else {
                if was_active_and_focused {
                    crate::worker::send_mouse_hold(false);
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
                    Worker::no_fall_damage();
                }
                WorkerMessage::HackingClickMtd {
                    x,
                    y,
                    w,
                    h,
                    offset_y,
                } => {
                    Worker::hacking_click_mtd(x, y, w, h, offset_y);
                }
                WorkerMessage::HackingJumpMtd {
                    x,
                    y,
                    w,
                    h,
                    offset_y,
                } => {
                    Worker::hacking_jump_mtd(x, y, w, h, offset_y);
                }
                WorkerMessage::HackingEscMtd {
                    x,
                    y,
                    w,
                    h,
                    offset_y,
                } => {
                    Worker::hacking_esc_mtd(x, y, w, h, offset_y);
                }
                WorkerMessage::TipsSkip { x, w, y } => {
                    Worker::tips_skip(x, w, y);
                }
                WorkerMessage::Restart { x, w, y } => {
                    Worker::restart(x, w, y);
                }
                WorkerMessage::FastLoadout => {
                    Worker::fast_loadout();
                }
                WorkerMessage::HoldItemBug => {
                    Worker::hold_item_bug();
                }
                WorkerMessage::GangstaGrip { digit } => {
                    Worker::gangsta_grip(digit);
                }

                WorkerMessage::QuickExit { x, y } => {
                    Worker::quick_exit(x, y);
                }
                WorkerMessage::DoubleClick(btn) => {
                    Worker::double_click(btn);
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
                            FeatureId::HackingClickMtd => Some(WorkerMessage::HackingClickMtd {
                                x,
                                y,
                                w,
                                h,
                                offset_y: hack_y,
                            }),
                            FeatureId::HackingJumpMtd => Some(WorkerMessage::HackingJumpMtd {
                                x,
                                y,
                                w,
                                h,
                                offset_y: hack2_y,
                            }),
                            FeatureId::HackingEscMtd => Some(WorkerMessage::HackingEscMtd {
                                x,
                                y,
                                w,
                                h,
                                offset_y: hack_esc_y,
                            }),
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
                            FeatureId::GangstaGrip => {
                                Some(WorkerMessage::GangstaGrip { digit: gun_digit })
                            }
                            FeatureId::QuickExit => Some(WorkerMessage::QuickExit { x, y }),
                            FeatureId::DoubleClick => {
                                Some(WorkerMessage::DoubleClick(double_click_btn))
                            }
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
