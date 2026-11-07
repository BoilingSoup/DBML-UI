//! Non-Linux stubs for the two platform hooks (macOS/Windows builds).
//!
//! The Linux module reads the physical Ctrl state over X11 and raises
//! dialogs with a NetActiveWindow client message — Linux-only tricks.
//! Elsewhere the native dialogs manage their own focus, and the logical
//! modifiers iced reports are authoritative (`ctrl_held` returning None
//! makes every caller fall back to them).

pub fn ctrl_held() -> Option<bool> {
    None
}

pub fn raise_dialog(_title: &str) {}
