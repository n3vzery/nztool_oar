use crate::features::DoubleClickButton;
use log::{error, warn};
use rdev::Key;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// Worker thread messages for feature execution
pub enum WorkerMessage {
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
    GangstaGrip {
        digit: u32,
    },
    QuickExit {
        x: i32,
        y: i32,
    },
    DoubleClick(DoubleClickButton),
}

// --- CONSTANTS ---
pub const HACKING_DELAY_MS: u64 = 12;
pub const MOUSE_CLICK_PRE_DELAY_MS: u64 = 2;
pub const HOLD_ITEM_TAP_DELAY_MS: u64 = 1;
pub const RESTART_KEY_DELAY_MS: u64 = 30;
pub const RESTART_SETTLE_DELAY_MS: u64 = 10;
pub const NO_FALL_DAMAGE_DELAY_MS: u64 = 7;
pub const QUICK_EXIT_DELAY_MS: u64 = 15;
pub const BHOP_TAP_INTERVAL_MS: u64 = 3;
pub const POLL_INTERVAL_MS: u64 = 5;
pub const AUTO_CLICKER_MIN_DELAY_MS: u32 = 0;
pub const AUTO_CLICKER_MAX_DELAY_MS: u32 = 500;

// pack coords to LPARAM for PostMessage
pub fn pack_lparam(x: i32, y: i32) -> isize {
    let low = ((x as i16) as u16 as u32) as isize;
    let high = (((y as i16) as u16 as u32) << 16) as isize;
    high | low
}

pub struct Worker;

impl Worker {
    pub fn hacking_click_mtd(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
        unsafe {
            thread::sleep(Duration::from_millis(HACKING_DELAY_MS));
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }
            let center_x = x + w / 2;
            let center_y = y + h / 2;
            let target_x = center_x;
            let target_y = center_y + offset_y;
            let mut pt = POINT {
                x: target_x,
                y: target_y,
            };
            if !ScreenToClient(hwnd, &mut pt).as_bool() {
                warn!("ScreenToClient failed for hacking method");
            }
            set_cursor_pos_safe(target_x, target_y);
            let l_param = pack_lparam(pt.x, pt.y);
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
            set_cursor_pos_safe(center_x, y);
            send_mouse_click();
        }
    }

    pub fn hacking_jump_mtd(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
        unsafe {
            thread::sleep(Duration::from_millis(HACKING_DELAY_MS));
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }
            let center_x = x + w / 2;
            let center_y = y + h / 2;
            let target_x = center_x;
            let target_y = center_y + offset_y;
            let mut pt = POINT {
                x: target_x,
                y: target_y,
            };
            if !ScreenToClient(hwnd, &mut pt).as_bool() {
                warn!("ScreenToClient failed for hacking method 2");
            }
            set_cursor_pos_safe(target_x, target_y);
            let l_param = pack_lparam(pt.x, pt.y);
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
            send_key_tap(0x39);
        }
    }

    pub fn hacking_esc_mtd(x: i32, y: i32, w: i32, h: i32, offset_y: i32) {
        unsafe {
            thread::sleep(Duration::from_millis(HACKING_DELAY_MS));
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }
            let center_x = x + w / 2;
            let center_y = y + h / 2;
            let target_x = center_x;
            let target_y = center_y + offset_y;
            let mut pt = POINT {
                x: target_x,
                y: target_y,
            };
            if !ScreenToClient(hwnd, &mut pt).as_bool() {
                warn!("ScreenToClient failed for hacking method esc");
            }
            set_cursor_pos_safe(target_x, target_y);
            let l_param = pack_lparam(pt.x, pt.y);
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
            send_key_tap(0x01);
            thread::sleep(Duration::from_millis(1));
            send_key_tap(0x01);
        }
    }

    pub fn tips_skip(x: i32, w: i32, y_abs: i32) {
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

    pub fn restart(x: i32, w: i32, y_abs: i32) {
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

    pub fn no_fall_damage() {
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(NO_FALL_DAMAGE_DELAY_MS));
        send_key_tap(0x01);
    }

    pub fn fast_loadout() {
        unsafe {
            let mut wheel_input = INPUT {
                r#type: INPUT_MOUSE,
                ..Default::default()
            };
            wheel_input.Anonymous.mi.dwFlags = MOUSEEVENTF_WHEEL;
            wheel_input.Anonymous.mi.mouseData = 120;
            if SendInput(&[wheel_input], std::mem::size_of::<INPUT>() as i32) == 0 {
                error!("SendInput failed in fast_loadout (wheel)");
            }
            thread::sleep(Duration::from_millis(MOUSE_CLICK_PRE_DELAY_MS));
            send_mouse_click();
        }
    }

    pub fn hold_item_bug() {
        send_mouse_hold(true);
        thread::sleep(Duration::from_millis(HOLD_ITEM_TAP_DELAY_MS));
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(HOLD_ITEM_TAP_DELAY_MS));
        send_mouse_hold(false);
        thread::sleep(Duration::from_millis(HOLD_ITEM_TAP_DELAY_MS));
        send_key_tap(0x01);
    }

    #[allow(dead_code)]
    pub fn gangsta_grip(digit: u32) {
        send_mouse_hold(true);
        send_key_tap(0x01);
        send_mouse_hold(false);
        send_key_tap(digit as u16 + 1);
        send_key_tap(0x01);
    }

    pub fn quick_exit(x_offset: i32, y_offset: i32) {
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        move_mouse(x_offset + 722, y_offset + 731);
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        send_mouse_click();
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        move_mouse(x_offset + 719, y_offset + 546);
        thread::sleep(Duration::from_millis(QUICK_EXIT_DELAY_MS));
        send_mouse_click();
    }

    pub fn double_click(btn: DoubleClickButton) {
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

            if SendInput(
                &[input_down1, input_up1],
                std::mem::size_of::<INPUT>() as i32,
            ) == 0
            {
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

            if SendInput(
                &[input_down2, input_up2],
                std::mem::size_of::<INPUT>() as i32,
            ) == 0
            {
                error!("SendInput failed in double_click second tap");
            }
        }
    }
}

// safe cursor move
pub fn set_cursor_pos_safe(x: i32, y: i32) -> bool {
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

pub fn move_mouse(x: i32, y: i32) {
    set_cursor_pos_safe(x, y);
}

pub fn send_mouse_click() {
    send_instant_burst_clicks(1);
}

pub fn send_mouse_click_postmessage(click_count: u32) {
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

pub fn send_instant_burst_clicks(count: usize) {
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

pub fn send_mouse_hold(down: bool) {
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

pub fn send_key_tap(scan: u16) {
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

pub fn send_key_state(scan: u16, down: bool) {
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

pub fn rdev_key_to_scancode(key: Key) -> Option<u16> {
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
        Key::CapsLock => Some(0x3A),
        Key::F1 => Some(0x3B),
        Key::F2 => Some(0x3C),
        Key::F3 => Some(0x3D),
        Key::F4 => Some(0x3E),
        Key::F5 => Some(0x3F),
        Key::F6 => Some(0x40),
        Key::F7 => Some(0x41),
        Key::F8 => Some(0x42),
        Key::F9 => Some(0x43),
        Key::F10 => Some(0x44),
        Key::F11 => Some(0x57),
        Key::F12 => Some(0x58),
        Key::ShiftLeft => Some(0x2A),
        Key::ShiftRight => Some(0x36),
        Key::ControlLeft => Some(0x1D),
        Key::ControlRight => Some(0x1D), // Requires E0 prefix ideally, but this works for basic binds
        Key::Alt => Some(0x38),
        Key::AltGr => Some(0x38),
        Key::UpArrow => Some(0xC8),
        Key::DownArrow => Some(0xD0),
        Key::LeftArrow => Some(0xCB),
        Key::RightArrow => Some(0xCD),
        Key::Insert => Some(0xD2),
        Key::Delete => Some(0xD3),
        Key::Home => Some(0xC7),
        Key::End => Some(0xCF),
        Key::PageUp => Some(0xC9),
        Key::PageDown => Some(0xD1),
        _ => None,
    }
}
