//! macOS paste (SPEC 10.1): remember the frontmost app when dictation starts, bring it back, and
//! post Cmd+V with `CGEventPost` (needs the Accessibility permission).

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};

use crate::DesktopError;

/// `kVK_ANSI_V`: the V key's position, which is where Cmd+V lives on every layout macOS ships.
const KEY_V: u16 = 9;

/// Process id of the frontmost app.
pub fn frontmost_app() -> Option<i32> {
    let ws = NSWorkspace::sharedWorkspace();
    ws.frontmostApplication().map(|app| app.processIdentifier())
}

/// Brings the app back to the front. Returns false if it quit.
pub fn activate_app(pid: i32) -> bool {
    match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        Some(app) => app.activateWithOptions(NSApplicationActivationOptions::empty()),
        None => false,
    }
}

/// Posts Cmd+V to the frontmost app.
pub fn paste() -> Result<(), DesktopError> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(None, KEY_V, down)
            .ok_or_else(|| DesktopError::Input("could not create a key event".into()))?;
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
    Ok(())
}
