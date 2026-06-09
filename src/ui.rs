use crate::features::{BindKey, ConfigKey, DoubleClickButton, FeatureId};
use crate::state::{AppState, AutoClickerMode, ClickMethod, GLOBAL_STATE, InputState, safe_lock};
use crate::update::{UPDATE_STATE, start_update};
use crate::worker::{AUTO_CLICKER_MAX_DELAY_MS, AUTO_CLICKER_MIN_DELAY_MS};
use eframe::egui;
use log::error;
use rdev::Key;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::System::Console::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct KeyBindApp {
    pub state: Arc<Mutex<AppState>>,
    pub prev_ctrl: bool,
    pub prev_shift: bool,
    pub prev_alt: bool,
    pub prev_capslock: bool,
    pub prev_mouse_mid: bool,
    pub prev_mouse4: bool,
    pub prev_mouse5: bool,
}

impl KeyBindApp {
    pub fn new(state: Arc<Mutex<AppState>>) -> Self {
        Self {
            state,
            prev_ctrl: false,
            prev_shift: false,
            prev_alt: false,
            prev_capslock: false,
            prev_mouse_mid: false,
            prev_mouse4: false,
            prev_mouse5: false,
        }
    }
}

impl Drop for KeyBindApp {
    fn drop(&mut self) {
        // Signal rdev::grab thread to shut down gracefully
        GLOBAL_STATE.rdev_shutdown.store(true, Ordering::SeqCst);

        let input_state = InputState;
        let mouse_thread_id = input_state.get_mouse_hook_thread_id();
        if mouse_thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageA(mouse_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        let keyboard_thread_id = input_state.get_keyboard_hook_thread_id();
        if keyboard_thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageA(keyboard_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

pub fn egui_to_rdev_key(key: egui::Key) -> Option<Key> {
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
        ArrowLeft => Some(Key::LeftArrow),
        ArrowRight => Some(Key::RightArrow),
        ArrowUp => Some(Key::UpArrow),
        ArrowDown => Some(Key::DownArrow),
        _ => None,
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
                        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                        .open(&mut is_open)
                        .show(ctx, |ui| {
                            ui.label(format!(
                                "A new update ({}) is available. Do you want to update?",
                                update.new_version
                            ));
                            ui.add_space(8.0);

                            if let Some(body) = &update.body {
                                ui.label("Release Notes:");
                                egui::ScrollArea::vertical()
                                    .max_height(200.0)
                                    .show(ui, |ui| {
                                        render_markdown(ui, body);
                                    });
                                ui.add_space(8.0);
                            }

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
                        if !modifiers.ctrl
                            && !modifiers.shift
                            && !modifiers.alt
                            && (*key == egui::Key::Escape || egui_to_rdev_key(*key).is_some())
                        {
                            return Some(*key);
                        }
                    }
                }
                None
            });

            // check mods via winapi (bypasses egui focus block)
            let ctrl_down =
                unsafe { (GetAsyncKeyState(VK_CONTROL.0 as i32) & 0x8000u16 as i16) != 0 };
            let shift_down =
                unsafe { (GetAsyncKeyState(VK_SHIFT.0 as i32) & 0x8000u16 as i16) != 0 };
            let alt_down = unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) & 0x8000u16 as i16) != 0 };
            let capslock_down =
                unsafe { (GetAsyncKeyState(VK_CAPITAL.0 as i32) & 0x8000u16 as i16) != 0 };
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
                let title = "Nztool OAR v2.4.2";
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
                                    crate::worker::send_mouse_hold(false);
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
                                && s.features[i].bind_key.is_some()
                            {
                                s.features[i].enabled = !s.features[i].enabled;
                                if !s.features[i].enabled {
                                    if s.features[i].id == FeatureId::ShiftToggle {
                                        s.release_shift();
                                    }
                                    if s.features[i].id == FeatureId::KeepItemClicker {
                                        InputState.set_lmb_hold_active(false);
                                        crate::worker::send_mouse_hold(false);
                                    }
                                    if s.features[i].id == FeatureId::ToggleAllMacros {
                                        GLOBAL_STATE
                                            .all_macros_disabled
                                            .store(false, Ordering::SeqCst);
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
                                        ui.selectable_value(
                                            &mut s.auto_clicker_key,
                                            k,
                                            format!("{:?}", k),
                                        );
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
                            let send_input_selected =
                                s.auto_clicker_method == ClickMethod::SendInput;
                            let post_message_selected =
                                s.auto_clicker_method == ClickMethod::PostMessage;
                            if ui
                                .selectable_label(send_input_selected, "SendInput")
                                .clicked()
                            {
                                s.auto_clicker_method = ClickMethod::SendInput;
                                let _ = s.save_config();
                            }
                            if ui
                                .selectable_label(post_message_selected, "PostMessage")
                                .clicked()
                            {
                                s.auto_clicker_method = ClickMethod::PostMessage;
                                let _ = s.save_config();
                            }
                        });

                        // PostMessage click count
                        if s.auto_clicker_method == ClickMethod::PostMessage {
                            ui.horizontal(|ui| {
                                ui.label("Clicks per trigger:");
                                ui.add(
                                    egui::DragValue::new(&mut s.auto_clicker_click_count)
                                        .speed(1.0)
                                        .range(1..=20),
                                );
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
                        if ui
                            .add(egui::DragValue::new(&mut val).range(1..=6969))
                            .changed()
                        {
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
                            .selected_text(if selected.is_empty() {
                                "None".to_string()
                            } else {
                                selected.clone()
                            })
                            .show_ui(ui, |ui| {
                                for preset in &s.presets {
                                    ui.selectable_value(
                                        &mut selected,
                                        preset.clone(),
                                        preset.clone(),
                                    );
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
                        ui.add(
                            egui::TextEdit::singleline(&mut s.preset_name_input)
                                .desired_width(120.0),
                        );
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
                    if ui
                        .checkbox(&mut debug_enabled, "Enable Click Debugging")
                        .changed()
                    {
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
                        let config_path = crate::state::get_config_path();
                        if let Some(parent) = config_path.parent() {
                            let _ = std::process::Command::new("explorer").arg(parent).spawn();
                        }
                    }
                }
            });
        });
    }
}

fn render_markdown(ui: &mut egui::Ui, text: &str) {
    ui.vertical(|ui| {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                ui.add_space(4.0);
                continue;
            }
            if let Some(header) = line.strip_prefix("# ") {
                ui.heading(header);
            } else if let Some(header) = line.strip_prefix("## ") {
                ui.add(egui::Label::new(egui::RichText::new(header).strong().size(16.0)));
            } else if let Some(header) = line.strip_prefix("### ") {
                ui.add(egui::Label::new(egui::RichText::new(header).strong().size(14.0)));
            } else {
                let (is_bullet, content) = if let Some(bullet) = line.strip_prefix("* ") {
                    (true, bullet)
                } else if let Some(bullet) = line.strip_prefix("- ") {
                    (true, bullet)
                } else {
                    (false, line)
                };

                ui.horizontal(|ui| {
                    if is_bullet {
                        ui.label("•");
                    }
                    render_inline_styled_text(ui, content);
                });
            }
        }
    });
}

fn render_inline_styled_text(ui: &mut egui::Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        
        let mut current = text;
        while !current.is_empty() {
            let next_bold = current.find("**");
            let next_code = current.find('`');
            
            match (next_bold, next_code) {
                (Some(b_idx), Some(c_idx)) => {
                    if b_idx < c_idx {
                        if b_idx > 0 {
                            ui.label(&current[..b_idx]);
                        }
                        let remaining = &current[b_idx + 2..];
                        if let Some(end_bold) = remaining.find("**") {
                            ui.strong(&remaining[..end_bold]);
                            current = &remaining[end_bold + 2..];
                        } else {
                            ui.label(&current[b_idx..]);
                            break;
                        }
                    } else {
                        if c_idx > 0 {
                            ui.label(&current[..c_idx]);
                        }
                        let remaining = &current[c_idx + 1..];
                        if let Some(end_code) = remaining.find('`') {
                            ui.add(egui::Label::new(
                                egui::RichText::new(&remaining[..end_code])
                                    .code()
                                    .background_color(egui::Color32::from_rgba_unmultiplied(128, 128, 128, 30))
                            ));
                            current = &remaining[end_code + 1..];
                        } else {
                            ui.label(&current[c_idx..]);
                            break;
                        }
                    }
                }
                (Some(b_idx), None) => {
                    if b_idx > 0 {
                        ui.label(&current[..b_idx]);
                    }
                    let remaining = &current[b_idx + 2..];
                    if let Some(end_bold) = remaining.find("**") {
                        ui.strong(&remaining[..end_bold]);
                        current = &remaining[end_bold + 2..];
                    } else {
                        ui.label(&current[b_idx..]);
                        break;
                    }
                }
                (None, Some(c_idx)) => {
                    if c_idx > 0 {
                        ui.label(&current[..c_idx]);
                    }
                    let remaining = &current[c_idx + 1..];
                    if let Some(end_code) = remaining.find('`') {
                        ui.add(egui::Label::new(
                            egui::RichText::new(&remaining[..end_code])
                                .code()
                                .background_color(egui::Color32::from_rgba_unmultiplied(128, 128, 128, 30))
                        ));
                        current = &remaining[end_code + 1..];
                    } else {
                        ui.label(&current[c_idx..]);
                        break;
                    }
                }
                (None, None) => {
                    ui.label(current);
                    break;
                }
            }
        }
    });
}
