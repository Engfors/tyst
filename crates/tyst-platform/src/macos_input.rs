//! macOS paste (SPEC 10.1): remember the frontmost app when dictation starts, bring it back, and
//! post Cmd+V with `CGEventPost` (needs the Accessibility permission).
//!
//! Without that permission `CGEventPost` drops the keystroke silently, and macOS keeps showing an
//! old grant as switched on after the app is replaced by a new build. So callers ask [`trusted`]
//! first and report a refusal instead of claiming they pasted.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};

use crate::DesktopError;

/// `kVK_ANSI_V`: the V key's position, which is where Cmd+V lives on every layout macOS ships.
const KEY_V: u16 = 9;

/// Process id of the frontmost app.
pub fn frontmost_app() -> Option<i32> {
    let ws = NSWorkspace::sharedWorkspace();
    ws.frontmostApplication().map(|app| app.processIdentifier())
}

/// Brings the app back to the front and waits (up to half a second) until it is, so the paste
/// keystroke does not land in Tyst. Returns false if it quit.
pub fn activate_app(pid: i32) -> bool {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else { return false };
    if !app.activateWithOptions(NSApplicationActivationOptions::empty()) {
        return false;
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while frontmost_app() != Some(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// Gives up being the active app, so the app that was in front before (the meeting app) gets the
/// keyboard back. Must run on the main thread.
pub fn deactivate() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).deactivate();
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
}

/// Whether Tyst may post keystrokes (System Settings › Privacy & Security › Accessibility). With
/// `prompt`, macOS shows its dialog that leads there when the answer is no.
pub fn trusted(prompt: bool) -> bool {
    if !prompt {
        return unsafe { AXIsProcessTrusted() } != 0;
    }
    use objc2_foundation::{NSDictionary, NSNumber, NSString};
    // `kAXTrustedCheckOptionPrompt`.
    let key = NSString::from_str("AXTrustedCheckOptionPrompt");
    let yes = NSNumber::new_bool(true);
    let options = NSDictionary::from_slices(&[&*key], &[&*yes]);
    unsafe { AXIsProcessTrustedWithOptions(objc2::rc::Retained::as_ptr(&options).cast()) != 0 }
}

/// Posts Cmd+V to the frontmost app. Check [`trusted`] first: without the permission the
/// keystroke is dropped and this still returns `Ok`.
pub fn paste() -> Result<(), DesktopError> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(None, KEY_V, down)
            .ok_or_else(|| DesktopError::Input("could not create a key event".into()))?;
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
    Ok(())
}
