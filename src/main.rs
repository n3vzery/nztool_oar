#![windows_subsystem = "windows"]

use eframe::egui;
use inputbot::KeybdKey;
use rdev::{grab, Event, EventType, Key};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::HiDpi::*;

// Global variable to store the real (physical) state of LMB
static REAL_LMB_DOWN: AtomicBool = AtomicBool::new(false);

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

struct Feature {
    name: String,
    key: Option<KeybdKey>,
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
    autoclicker_active: bool,
}

impl AppState {
    fn new() -> Self {
        let mut state = Self {
            features: vec![
                Feature { name: "Hacking Device (PostMessage)".into(), key: None, rdev_key: None, enabled: false, selecting: false },
                Feature { name: "Hacking Device (PostMessage 2)".into(), key: None, rdev_key: None, enabled: false, selecting: false },
                Feature { name: "Tips Skip".into(), key: None, rdev_key: None, enabled: false, selecting: false },
                Feature { name: "Restart".into(), key: None, rdev_key: None, enabled: false, selecting: false },
                Feature { name: "No Fall Damage".into(), key: None, rdev_key: None, enabled: false, selecting: false },
                Feature { name: "Shift Toggle".into(), key: None, rdev_key: None, enabled: false, selecting: false },
                Feature { name: "Auto Clicker".into(), key: None, rdev_key: None, enabled: false, selecting: false },
            ],
            monitor_id: "1".into(),
            x_offset: 0,
            y_offset: 0,
            width: 0,
            height: 0,
            shift_held: false,
            autoclicker_active: false,
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

impl KeyBindApp {
    fn start_hotkey_listener(state: Arc<Mutex<AppState>>) {
        // Start the real mouse listener in a separate thread with a message loop
        thread::spawn(|| {
            unsafe {
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
                let (active, enabled) = {
                    let s = state_clone_ac.lock().unwrap();
                    let enabled = s.features.iter().any(|f| f.name == "Auto Clicker" && f.enabled);
                    (s.autoclicker_active, enabled)
                };

                if active && enabled {
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

                // ... remaining hotkey logic ...
                let s = match state_clone_hk.try_lock() {
                    Ok(s) => s,
                    Err(_) => return Some(event),
                };

                if let EventType::KeyPress(key) = event.event_type {
                    if let Some(feature) = s.features.iter().find(|f| f.enabled && f.rdev_key == Some(key)) {
                        let feature_name = feature.name.clone();
                        let coords = (s.x_offset, s.y_offset, s.width, s.height);
                        let state_clone = state_clone_hk.clone();

                        if feature_name == "Auto Clicker" {
                            drop(s);
                            let mut s_lock = state_clone.lock().unwrap();
                            s_lock.autoclicker_active = !s_lock.autoclicker_active;
                            return None; // Block the bind key itself
                        }

                        thread::spawn(move || {
                            match feature_name.as_str() {
                                "Hacking Device (PostMessage)" => Self::hacking_method_post_message(coords.0, coords.1, coords.2, coords.3),
                                "Hacking Device (PostMessage 2)" => Self::hacking_method2(coords.0, coords.1, coords.2, coords.3),
                                "Tips Skip" => Self::tips_skip(coords.0, coords.1, coords.2, coords.3),
                                "Restart" => Self::restart(coords.0, coords.1, coords.2, coords.3),
                                "No Fall Damage" => Self::no_fall_damage(),
                                "Shift Toggle" => {
                                    let mut s = state_clone.lock().unwrap();
                                    s.toggle_shift();
                                }
                                _ => {}
                            }
                        });
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

    fn hacking_method_post_message(_x: i32, _y: i32, _w: i32, _h: i32) {
        unsafe {
            // 1. Wait (DELAY_MS = 50)
            thread::sleep(Duration::from_millis(50));

            // 2. Get game window
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }

            // 3. Screen center calculation
            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let screen_height = GetSystemMetrics(SM_CYSCREEN);
            let center_x = screen_width / 2;
            let center_y = screen_height / 2;

            // 4. Target position (OFFSET_Y = -140)
            let target_x = center_x;
            let target_y = center_y - 140;

            // 5. Instant Move
            let _ = SetCursorPos(target_x, target_y);

            // 6. Direct Message Blast
            let l_param = ((target_y as u32) << 16) | (target_x as u32 & 0xFFFF);
            
            // CLICK_COUNT = 100, MK_LBUTTON = 0x0001
            for _ in 0..100 {
                let _ = PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), LPARAM(l_param as isize));
                let _ = PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(l_param as isize));
            }

            // 7. Move to top and click once
            let _ = SetCursorPos(center_x, 0);
            send_mouse_click();
        }
    }

    fn hacking_method2(_x: i32, _y: i32, _w: i32, _h: i32) {
        unsafe {
            // 1. Wait (DELAY_MS = 50)
            thread::sleep(Duration::from_millis(50));

            // 2. Get game window
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return;
            }

            // 3. Screen center calculation
            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let screen_height = GetSystemMetrics(SM_CYSCREEN);
            let center_x = screen_width / 2;
            let center_y = screen_height / 2;

            // 4. Target position (OFFSET_Y = -140)
            let target_x = center_x;
            let target_y = center_y - 140;

            // 5. Instant Move
            let _ = SetCursorPos(target_x, target_y);

            // 6. Direct Message Blast
            let l_param = ((target_y as u32) << 16) | (target_x as u32 & 0xFFFF);
            
            // CLICK_COUNT = 100, MK_LBUTTON = 0x0001
            for _ in 0..100 {
                let _ = PostMessageA(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), LPARAM(l_param as isize));
                let _ = PostMessageA(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(l_param as isize));
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
        send_key_tap(0x01);
        thread::sleep(Duration::from_millis(50));
        send_key_tap(0x01);
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
                    let (kb_key, rd_key) = egui_to_inputbot_key(k);
                    if kb_key.is_some() || rd_key.is_some() {
                        s.features[idx].key = kb_key;
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
                                       else if let Some(k) = s.features[i].key { format!("{:?}", k) } 
                                       else { "Select Key".into() };
                        
                        if ui.button(key_text).clicked() { s.features[i].selecting = true; }

                        // 3. Reset Button
                        if ui.button("Reset").clicked() {
                            if s.features[i].name == "Shift Toggle" { s.release_shift(); }
                            s.features[i].key = None; s.features[i].enabled = false; s.features[i].selecting = false;
                        }

                        // 4. Enable/Disable Button
                        let mut color = if s.features[i].enabled { egui::Color32::from_rgb(0, 150, 0) } 
                                        else { egui::Color32::from_rgb(150, 0, 0) };
                        if s.features[i].name == "Shift Toggle" && s.shift_held && s.features[i].enabled { color = egui::Color32::BLUE; }

                        if ui.add(egui::Button::new("Enable/Disable").fill(color)).clicked() {
                            if s.features[i].key.is_some() {
                                s.features[i].enabled = !s.features[i].enabled;
                                if !s.features[i].enabled && s.features[i].name == "Shift Toggle" { s.release_shift(); }
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

fn egui_to_inputbot_key(key: egui::Key) -> (Option<KeybdKey>, Option<Key>) {
    use egui::Key::*;
    match key {
        A => (Some(KeybdKey::AKey), Some(Key::KeyA)), B => (Some(KeybdKey::BKey), Some(Key::KeyB)),
        C => (Some(KeybdKey::CKey), Some(Key::KeyC)), D => (Some(KeybdKey::DKey), Some(Key::KeyD)),
        E => (Some(KeybdKey::EKey), Some(Key::KeyE)), F => (Some(KeybdKey::FKey), Some(Key::KeyF)),
        G => (Some(KeybdKey::GKey), Some(Key::KeyG)), H => (Some(KeybdKey::HKey), Some(Key::KeyH)),
        I => (Some(KeybdKey::IKey), Some(Key::KeyI)), J => (Some(KeybdKey::JKey), Some(Key::KeyJ)),
        K => (Some(KeybdKey::KKey), Some(Key::KeyK)), L => (Some(KeybdKey::LKey), Some(Key::KeyL)),
        M => (Some(KeybdKey::MKey), Some(Key::KeyM)), N => (Some(KeybdKey::NKey), Some(Key::KeyN)),
        O => (Some(KeybdKey::OKey), Some(Key::KeyO)), P => (Some(KeybdKey::PKey), Some(Key::KeyP)),
        Q => (Some(KeybdKey::QKey), Some(Key::KeyQ)), R => (Some(KeybdKey::RKey), Some(Key::KeyR)),
        S => (Some(KeybdKey::SKey), Some(Key::KeyS)), T => (Some(KeybdKey::TKey), Some(Key::KeyT)),
        U => (Some(KeybdKey::UKey), Some(Key::KeyU)), V => (Some(KeybdKey::VKey), Some(Key::KeyV)),
        W => (Some(KeybdKey::WKey), Some(Key::KeyW)), X => (Some(KeybdKey::XKey), Some(Key::KeyX)),
        Y => (Some(KeybdKey::YKey), Some(Key::KeyY)), Z => (Some(KeybdKey::ZKey), Some(Key::KeyZ)),
        Num0 => (Some(KeybdKey::Numrow0Key), Some(Key::Num0)), Num1 => (Some(KeybdKey::Numrow1Key), Some(Key::Num1)),
        Num2 => (Some(KeybdKey::Numrow2Key), Some(Key::Num2)), Num3 => (Some(KeybdKey::Numrow3Key), Some(Key::Num3)),
        Num4 => (Some(KeybdKey::Numrow4Key), Some(Key::Num4)), Num5 => (Some(KeybdKey::Numrow5Key), Some(Key::Num5)),
        Num6 => (Some(KeybdKey::Numrow6Key), Some(Key::Num6)), Num7 => (Some(KeybdKey::Numrow7Key), Some(Key::Num7)),
        Num8 => (Some(KeybdKey::Numrow8Key), Some(Key::Num8)), Num9 => (Some(KeybdKey::Numrow9Key), Some(Key::Num9)),
        F1 => (Some(KeybdKey::F1Key), Some(Key::F1)), F2 => (Some(KeybdKey::F2Key), Some(Key::F2)),
        F3 => (Some(KeybdKey::F3Key), Some(Key::F3)), F4 => (Some(KeybdKey::F4Key), Some(Key::F4)),
        F5 => (Some(KeybdKey::F5Key), Some(Key::F5)), F6 => (Some(KeybdKey::F6Key), Some(Key::F6)),
        F7 => (Some(KeybdKey::F7Key), Some(Key::F7)), F8 => (Some(KeybdKey::F8Key), Some(Key::F8)),
        F9 => (Some(KeybdKey::F9Key), Some(Key::F9)), F10 => (Some(KeybdKey::F10Key), Some(Key::F10)),
        F11 => (Some(KeybdKey::F11Key), Some(Key::F11)), F12 => (Some(KeybdKey::F12Key), Some(Key::F12)),
        Space => (Some(KeybdKey::SpaceKey), Some(Key::Space)), Enter => (Some(KeybdKey::EnterKey), Some(Key::Return)),
        Escape => (Some(KeybdKey::EscapeKey), Some(Key::Escape)), Tab => (Some(KeybdKey::TabKey), Some(Key::Tab)),
        Backspace => (Some(KeybdKey::BackspaceKey), Some(Key::Backspace)), Insert => (Some(KeybdKey::InsertKey), Some(Key::Insert)),
        Delete => (Some(KeybdKey::DeleteKey), Some(Key::Delete)), Home => (Some(KeybdKey::HomeKey), Some(Key::Home)),
        End => (Some(KeybdKey::EndKey), Some(Key::End)), PageUp => (Some(KeybdKey::PageUpKey), Some(Key::PageUp)),
        PageDown => (Some(KeybdKey::PageDownKey), Some(Key::PageDown)), ArrowUp => (Some(KeybdKey::UpKey), Some(Key::UpArrow)),
        ArrowDown => (Some(KeybdKey::DownKey), Some(Key::DownArrow)), ArrowLeft => (Some(KeybdKey::LeftKey), Some(Key::LeftArrow)),
        ArrowRight => (Some(KeybdKey::RightKey), Some(Key::RightArrow)),
        _ => (None, None),
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
