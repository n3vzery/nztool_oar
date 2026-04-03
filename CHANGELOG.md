# Changelog - Rust OAR Helper

## Latest Update: Grab No Gun Macro
- **New Feature**: Added "Grab No Gun" macro
    - Upon trigger, performs mouse wheel scroll up followed by a single left mouse click
    - Useful for quickly grabbing items without using the weapon slot

## No Fall Damage Fix
- **Fixed Application Freezing**: 
    - Resolved an issue where the application would stop responding when the "No Fall Damage" macro was triggered.
    - The macro now executes in its own dedicated thread, ensuring it doesn't block the main hotkey listener.
- **Timing Adjustment**: 
    - Set the delay between ESC key presses to exactly **30ms** as requested for optimal performance.
- **Improved Reliability**: 
    - Added code comments to clearly identify the ESC key sequence within the macro logic.
