# Changelog - Rust OAR Helper

## Version 2.0.3 - April 12, 2026

### 🎮 New Features

#### Modifier Keys Binding Support
You can now bind **Ctrl**, **Shift**, **Alt**, and **Caps Lock** keys to your macros!

**How to use:**
1. Click "Select Key" next to any feature
2. Press Ctrl, Shift, Alt, or Caps Lock
3. The key will be instantly bound - no need to press any other keys

### 🔧 Code Quality Improvements

#### Refactored Global State
- Consolidated 13 separate static variables into a single `GlobalInputState` struct
- Improved code organization and maintainability
- Cleaner API through `InputState` wrapper

#### Enhanced Error Handling
- Added `safe_lock()` helper to handle poisoned mutex gracefully
- Replaced all `.lock().unwrap()` calls with safe error recovery
- Application no longer panics on mutex poisoning

#### Improved Windows Hook Threads
- Better graceful shutdown with proper error checking
- Enhanced `GetMessageW` loop with BOOL return value handling
- Added shutdown flag checking to prevent hanging threads

#### Retry Logic for Windows API
- Added `set_cursor_pos_safe()` with 3 retry attempts
- 5ms delay between retries for cursor positioning
- Better reliability for mouse operations

#### Code Quality
- Fixed all 19 clippy warnings
- Removed unnecessary type casts
- Simplified nested if statements
- Improved struct initialization patterns
- All comments in English for better maintainability

### 🐛 Bug Fixes
- **Fixed:** Modifier keys now work correctly when nztool window is active
- **Fixed:** UI updates immediately when pressing modifier keys during key selection (60 FPS)
- **Improved:** Key selection is now more responsive

---

## Previous Updates

### Grab No Gun Macro
- **New Feature**: Added "Grab No Gun" macro
    - Upon trigger, performs mouse wheel scroll up followed by a single left mouse click
    - Useful for quickly grabbing items without using the weapon slot

### No Fall Damage Fix
- **Fixed Application Freezing**: 
    - Resolved an issue where the application would stop responding when the "No Fall Damage" macro was triggered.
    - The macro now executes in its own dedicated thread, ensuring it doesn't block the main hotkey listener.
- **Timing Adjustment**: 
    - Set the delay between ESC key presses to exactly **30ms** as requested for optimal performance.
- **Improved Reliability**: 
    - Added code comments to clearly identify the ESC key sequence within the macro logic.
