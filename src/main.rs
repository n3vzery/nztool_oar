#![windows_subsystem = "windows"]

use eframe::egui;
use rdev::{grab, Event, EventType, Key};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::Duration;
use windows::Win32::System::Threading::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::HiDpi::*;

// Global variable to store the real (physical) state of LMB
static REAL_LMB_DOWN: AtomicBool = AtomicBool::new(false);
// Global variable for autoclicker mode to avoid Mutex contention
static AUTOCLICKER_ACTIVE: AtomicBool = AtomicBool::new(false);
// Thread ID of the mouse hook thread for clean shutdown
static MOUSE_HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);

// Helper to check if the game window is currently focused
fn is_game_focused() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
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
            let mut buffer = [0u8; 260];
            let len = K32GetModuleBaseNameA(h, None, &mut buffer);
            let _ = CloseHandle(h);
            if len > 0 {
                let name = std::str::from_utf8(&buffer[..len as usize]).unwrap_or("");
                return name.eq_ignore_ascii_case("OAR-Win64-Shipping.exe");
            }
        }
        false
    }
}

// Helper to pack coordinates into LPARAM safely for PostMessage
fn pack_lparam(x: i32, y: i32) -> isize {
    let low = (x as u32 & 0xFFFF) as isize;
    let high = ((y as u32 & 0xFFFF) << 16) as isize;
    high | low
}

unsafe extern "system" fn low_level_mouse_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
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

#[derive(PartialEq, Clone, Copy)]
enum FeatureId {
    HackingPostMessage,
    HackingPostMessage2,
    TipsSkip,
    Restart,
    NoFallDamage,
    ShiftToggle,
    AutoClicker,
    GrabNoGun,
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
}

impl AppState {
    fn new() -> Self {
        let mut state = Self {
            features: vec![
                Feature { id: FeatureId::HackingPostMessage, name: "Hacking Device (PostMessage)".into(), rdev_key: None, enabled: false, selecting: false },
                Feature { id: FeatureId::HackingPostMessage2, name: "Hacking Device (PostMessage 2)".into(), rdev_key: None, enabled: false, selecting: false },
                Feature { id: FeatureId::TipsSkip, name: "Tips Skip".into(), rdev_key: None, enabled: false, selecting: false },
                Feature { id: FeatureId::Restart, name: "Restart".into(), rdev_key: None, enabled: false, selecting: false },
                Feature { id: FeatureId::NoFallDamage, name: "No Fall Damage".into(), rdev_key: None, enabled: false, selecting: false },
                Feature { id: FeatureId::ShiftToggle, name: "Shift Toggle".into(), rdev_key: None, enabled: false, selecting: false },
                Feature { id: FeatureId::AutoClicker, name: "Auto Clicker".into(), rdev_key: None, enabled: false, selecting: false },
                Feature { id: FeatureId::GrabNoGun, name: "Grab No Gun".into(), rdev_key: None, enabled: false, selecting: false },
            ],
            monitor_id: "1".into(),
            x_offset: 0,
            y_offset: 0,
            width: 0,
            height: 0,
            shift_held: false,
        };
        state.update_screen_position();
        state
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
}

struct KeyBindApp {
    state: Arc<Mutex<AppState>>,
}

impl Drop for KeyBindApp {
    fn drop(&mut self) {
        let thread_id = MOUSE_HOOK_THREAD_ID.load(Ordering::SeqCst);
        if thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageA(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl KeyBindApp {
    fn start_hotkey_listener(state: Arc<Mutex<AppState>>) {
        // Start the real mouse listener in a separate thread with a message loop
        thread::spawn(|| {
            unsafe {
                // Store current thread ID for clean shutdown
                MOUSE_HOOK_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);

                let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), HINSTANCE::default(), 0).unwrap();
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let _ = UnhookWindowsHookEx(hook);
            }
        });

        let state_clone_ac = state.clone();
        thread::spawn(move || {
            loop {
                // Read the global autoclicker mode and check if the feature is enabled in UI
                let enabled = {
                    let s = state_clone_ac.lock().unwrap();
                    s.features.iter().any(|f| f.id == FeatureId::AutoClicker && f.enabled)
                };
                let active = AUTOCLICKER_ACTIVE.load(Ordering::SeqCst);

                if active && enabled && is_game_focused() {
                    // Use our global variable that knows if the button is PHYSICALLY pressed
                    if REAL_LMB_DOWN.load(Ordering::SeqCst) {
                        send_mouse_click();
                        thread::sleep(Duration::from_millis(30)); // Small delay between clicks
                    } else {
                        thread::sleep(Duration::from_millis(5));
                    }
                } else {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        });

        let state_clone_hk = state.clone();
        thread::spawn(move || {
            // grab is only needed for hotkey processing (Key)
            let callback = move |event: Event| {
                // If it's a mouse event, just let it pass through to the system
                if let EventType::ButtonPress(_) | EventType::ButtonRelease(_) | EventType::MouseMove {..} | EventType::Wheel {..} = event.event_type {
                    return Some(event);
                }

                // Check if game is focused before handling hotkeys
                if !is_game_focused() {
                    return Some(event);
                }

                // Block and wait for lock to ensure no hotkey is missed
                let s = state_clone_hk.lock().unwrap();

                if let EventType::KeyPress(key) = event.event_type {
                    if let Some(feature) = s.features.iter().find(|f| f.enabled && f.rdev_key == Some(key)) {
                        let feature_id = feature.id;
                        let coords = (s.x_offset, s.y_offset, s.width, s.height);
                        let state_clone = state_clone_hk.clone();

                        if feature_id == FeatureId::AutoClicker {
                            // Toggle global atomic variable
                            let current = AUTOCLICKER_ACTIVE.load(Ordering::SeqCst);
                            AUTOCLICKER_ACTIVE.store(!current, Ordering::SeqCst);
                            return None; // Block the bind key itself
                        }

                        // Offload all feature logic to separate threads to avoid blocking the rdev hook thread.
                        // Blocking the hook thread or calling SendInput synchronously can cause hangs/deadlocks.
                        match feature_id {
                            FeatureId::ShiftToggle => {
                                drop(s);
                                thread::spawn(move || {
                                    state_clone.lock().unwrap().toggle_shift();
                                });
                            },
                            FeatureId::NoFallDamage => {
                                thread::spawn(move || {
                                    Self::no_fall_damage();
                                });
                            },
                            FeatureId::HackingPostMessage => {
                                thread::spawn(move || {
                                    Self::hacking_method_post_message(coords.0, coords.1, coords.2, coords.3);
                                });
                            },
                            FeatureId::HackingPostMessage2 => {
                                thread::spawn(move || {
                                    Self::hacking_method2(coords.0, coords.1, coords.2, coords.3);
                                });
                            },
                            FeatureId::TipsSkip => {
                                thread::spawn(move || {
                                    Self::tips_skip(coords.0, coords.1, coords.2, coords.3);
                                });
                            },
                            FeatureId::Restart => {
                                thread::spawn(move || {
                                    Self::restart(coords.0, coords.1, coords.2, coords.3);
                                });
                            },
                            FeatureId::GrabNoGun => {
                                thread::spawn(move || {
                                    Self::grab_no_gun();
                                });
                            },
                            _ => {}
                        }
                        return None;
                    }
                }
                Some(event)
            };

            if let Err(error) = grab(callback) {
                eprintln!("Error in hotkey listener: {:?}", error);
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
            let target_y = center_y - 140;

            // 4. Convert Screen to Client Coordinates for PostMessage
            let mut pt = POINT { x: target_x, y: target_y };
            let _ = ScreenToClient(hwnd, &mut pt);

            // 5. Instant Move (Still screen-relative)
            let _ = SetCursorPos(target_x, target_y);

            // 6. Direct Message Blast (Packed safely)
            let l_param = pack_lparam(pt.x, pt.y);

            // CLICK_COUNT = 100, MK_LBUTTON = 0x0001
            // Added yield_now to prevent message queue overflow while maintaining high speed
            for _ in 0..100 {
                let _ = PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), LPARAM(l_param));
                let _ = PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(l_param));
                thread::yield_now();
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
            let target_y = center_y - 140;

            // 4. Convert Screen to Client Coordinates
            let mut pt = POINT { x: target_x, y: target_y };
            let _ = ScreenToClient(hwnd, &mut pt);

            // 5. Instant Move
            let _ = SetCursorPos(target_x, target_y);

            // 6. Direct Message Blast
            let l_param = pack_lparam(pt.x, pt.y);

            // CLICK_COUNT = 100, MK_LBUTTON = 0x0001
            // Added yield_now to prevent message queue overflow while maintaining high speed
            for _ in 0..100 {
                let _ = PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), LPARAM(l_param));
                let _ = PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(l_param));
                thread::yield_now();
            }

            // 7. Jump (Space scancode = 0x39)
            send_key_tap(0x39);
        }
    }

    fn tips_skip(x: i32, y: i32, w: i32, h: i32) {
        move_mouse(x + w / 2, y + (h as f32 * 0.75) as i32);
        send_mouse_click();
    }

    fn restart(x: i32, y: i32, w: i32, h: i32) {
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(100));
        move_mouse(x + w / 2, y + (h as f32 * 0.45) as i32);
        send_mouse_click();
    }

    fn no_fall_damage() {
        send_key_tap(0x01); // ESC
        thread::sleep(Duration::from_millis(30));
        send_key_tap(0x01); // ESC
    }

    fn grab_no_gun() {
        unsafe {
            // Scroll wheel up (WHEEL_DELTA = 120)
            let mut wheel_input = INPUT::default();
            wheel_input.r#type = INPUT_MOUSE;
            wheel_input.Anonymous.mi.dwFlags = MOUSEEVENTF_WHEEL;
            wheel_input.Anonymous.mi.mouseData = 120;
            let _ = SendInput(&[wheel_input], std::mem::size_of::<INPUT>() as i32);

            // Small delay before mouse click
            thread::sleep(Duration::from_millis(10));

            // Single left mouse click
            send_mouse_click();
        }
    }
}

impl eframe::App for KeyBindApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let mut s = self.state.lock().unwrap();
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
                            let rd_key = egui_to_rdev_key(k);
                            if rd_key.is_some() {
                                s.features[idx].rdev_key = rd_key;
                                s.features[idx].selecting = false;
                            }
                        }
                    }
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Key Bindings App (Rust OAR Helper)");
            ui.add_space(10.0);

            egui::Grid::new("features_grid")
                .num_columns(4)
                .spacing([20.0, 8.0])
                .show(ui, |ui| {
                    for i in 0..s.features.len() {
                        // 1. Feature Name
                        ui.label(format!("{}:", s.features[i].name));

                        // 2. Select Key Button
                        let key_text = if s.features[i].selecting { "Waiting...".into() }
                                       else if let Some(k) = s.features[i].rdev_key { rdev_key_to_name(k) }
                                       else { "Select Key".into() };

                        if ui.button(key_text).clicked() { s.features[i].selecting = true; }

                        // 3. Reset Button
                        if ui.button("Reset").clicked() {
                        if s.features[i].id == FeatureId::ShiftToggle { s.release_shift(); }
                        s.features[i].rdev_key = None; s.features[i].enabled = false; s.features[i].selecting = false;
                    }

                    // 4. Enable/Disable Button
                    let mut color = if s.features[i].enabled { egui::Color32::from_rgb(0, 150, 0) }
                                    else { egui::Color32::from_rgb(150, 0, 0) };
                    if s.features[i].id == FeatureId::ShiftToggle && s.shift_held && s.features[i].enabled { color = egui::Color32::BLUE; }

                    if ui.add(egui::Button::new("Enable/Disable").fill(color)).clicked() {
                        if s.features[i].rdev_key.is_some() {
                            s.features[i].enabled = !s.features[i].enabled;
                            if !s.features[i].enabled && s.features[i].id == FeatureId::ShiftToggle { s.release_shift(); }
                        }
                    }

                        ui.end_row(); // Move to the next row in the grid
                    }
                });

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                ui.label("Monitor ID:");
                if ui.text_edit_singleline(&mut s.monitor_id).changed() { s.update_screen_position(); }
            });
            ui.add_space(5.0);
            ui.label(format!("Monitor Pos: {}x{}, Size: {}x{}", s.x_offset, s.y_offset, s.width, s.height));
            if s.shift_held { ui.colored_label(egui::Color32::LIGHT_BLUE, "SHIFT IS CURRENTLY HELD"); }
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

fn egui_to_rdev_key(key: egui::Key) -> Option<Key> {
    use egui::Key::*;
    match key {
        A => Some(Key::KeyA), B => Some(Key::KeyB), C => Some(Key::KeyC), D => Some(Key::KeyD),
        E => Some(Key::KeyE), F => Some(Key::KeyF), G => Some(Key::KeyG), H => Some(Key::KeyH),
        I => Some(Key::KeyI), J => Some(Key::KeyJ), K => Some(Key::KeyK), L => Some(Key::KeyL),
        M => Some(Key::KeyM), N => Some(Key::KeyN), O => Some(Key::KeyO), P => Some(Key::KeyP),
        Q => Some(Key::KeyQ), R => Some(Key::KeyR), S => Some(Key::KeyS), T => Some(Key::KeyT),
        U => Some(Key::KeyU), V => Some(Key::KeyV), W => Some(Key::KeyW), X => Some(Key::KeyX),
        Y => Some(Key::KeyY), Z => Some(Key::KeyZ),
        Num0 => Some(Key::Num0), Num1 => Some(Key::Num1), Num2 => Some(Key::Num2), Num3 => Some(Key::Num3),
        Num4 => Some(Key::Num4), Num5 => Some(Key::Num5), Num6 => Some(Key::Num6), Num7 => Some(Key::Num7),
        Num8 => Some(Key::Num8), Num9 => Some(Key::Num9),
        F1 => Some(Key::F1), F2 => Some(Key::F2), F3 => Some(Key::F3), F4 => Some(Key::F4),
        F5 => Some(Key::F5), F6 => Some(Key::F6), F7 => Some(Key::F7), F8 => Some(Key::F8),
        F9 => Some(Key::F9), F10 => Some(Key::F10), F11 => Some(Key::F11), F12 => Some(Key::F12),
        Space => Some(Key::Space), Enter => Some(Key::Return), Escape => Some(Key::Escape),
        Tab => Some(Key::Tab), Backspace => Some(Key::Backspace), Insert => Some(Key::Insert),
        Delete => Some(Key::Delete), Home => Some(Key::Home), End => Some(Key::End),
        PageUp => Some(Key::PageUp), PageDown => Some(Key::PageDown),
        ArrowUp => Some(Key::UpArrow), ArrowDown => Some(Key::DownArrow),
        ArrowLeft => Some(Key::LeftArrow), ArrowRight => Some(Key::RightArrow),
        _ => None,
    }
}

fn rdev_key_to_name(key: Key) -> String {
    use Key::*;
    match key {
        KeyA => "A".to_string(), KeyB => "B".to_string(), KeyC => "C".to_string(), KeyD => "D".to_string(),
        KeyE => "E".to_string(), KeyF => "F".to_string(), KeyG => "G".to_string(), KeyH => "H".to_string(),
        KeyI => "I".to_string(), KeyJ => "J".to_string(), KeyK => "K".to_string(), KeyL => "L".to_string(),
        KeyM => "M".to_string(), KeyN => "N".to_string(), KeyO => "O".to_string(), KeyP => "P".to_string(),
        KeyQ => "Q".to_string(), KeyR => "R".to_string(), KeyS => "S".to_string(), KeyT => "T".to_string(),
        KeyU => "U".to_string(), KeyV => "V".to_string(), KeyW => "W".to_string(), KeyX => "X".to_string(),
        KeyY => "Y".to_string(), KeyZ => "Z".to_string(),
        Num0 => "0".to_string(), Num1 => "1".to_string(), Num2 => "2".to_string(), Num3 => "3".to_string(),
        Num4 => "4".to_string(), Num5 => "5".to_string(), Num6 => "6".to_string(), Num7 => "7".to_string(),
        Num8 => "8".to_string(), Num9 => "9".to_string(),
        F1 => "F1".to_string(), F2 => "F2".to_string(), F3 => "F3".to_string(), F4 => "F4".to_string(),
        F5 => "F5".to_string(), F6 => "F6".to_string(), F7 => "F7".to_string(), F8 => "F8".to_string(),
        F9 => "F9".to_string(), F10 => "F10".to_string(), F11 => "F11".to_string(), F12 => "F12".to_string(),
        Space => "Space".to_string(), Return => "Enter".to_string(), Escape => "Escape".to_string(),
        Tab => "Tab".to_string(), Backspace => "Backspace".to_string(), Insert => "Insert".to_string(),
        Delete => "Delete".to_string(), Home => "Home".to_string(), End => "End".to_string(),
        PageUp => "PageUp".to_string(), PageDown => "PageDown".to_string(),
        UpArrow => "↑".to_string(), DownArrow => "↓".to_string(), LeftArrow => "←".to_string(), RightArrow => "→".to_string(),
        Alt => "Alt".to_string(), ControlLeft => "Ctrl".to_string(), ControlRight => "Ctrl".to_string(), ShiftLeft => "Shift".to_string(), ShiftRight => "Shift".to_string(),
        _ => format!("{:?}", key),
    }
}

fn main() -> eframe::Result {
    unsafe { let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([450.0, 400.0]),
        ..Default::default()
    };
    let state = Arc::new(Mutex::new(AppState::new()));
    KeyBindApp::start_hotkey_listener(state.clone());
    eframe::run_native("Key Bindings App", options, Box::new(|_| Ok(Box::new(KeyBindApp { state }))))
}
