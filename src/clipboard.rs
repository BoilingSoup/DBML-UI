//! System-clipboard access via arboard.
//!
//! The app never touches arboard directly: everything goes through the
//! [`ClipboardBackend`] seam so the interaction paths (copy, cut, paste,
//! the context menu's enable flags) stay unit-testable headlessly — a
//! test stub stands in for the OS, which a headless run cannot touch.
//!
//! Every operation is fallible by design: no display (headless CI, SSH
//! without forwarding), a dead Wayland compositor or X11 errors surface
//! as `Err(String)` and the app turns them into a status-bar toast
//! instead of a panic. The web build has no system clipboard yet: the
//! backend factory reports that, and the same toast path handles it.

/// Text-only view of the system clipboard.
pub trait ClipboardBackend: std::fmt::Debug {
    /// Replace the clipboard contents. A multiline string is stored
    /// verbatim (platform line endings as given).
    fn set_text(&mut self, text: String) -> Result<(), String>;

    /// Current clipboard text, or an error when nothing can be read
    /// (no display, X11 failure — or an empty clipboard on platforms
    /// that report one).
    fn get_text(&mut self) -> Result<String, String>;
}

/// The real arboard-backed clipboard. (arboard::Clipboard is not Debug,
/// so the Debug impl is hand-rolled for the trait bound.)
#[cfg(not(target_arch = "wasm32"))]
pub struct SystemClipboard(arboard::Clipboard);

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for SystemClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemClipboard").finish()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ClipboardBackend for SystemClipboard {
    fn set_text(&mut self, text: String) -> Result<(), String> {
        self.0.set_text(text).map_err(|e| e.to_string())
    }

    fn get_text(&mut self) -> Result<String, String> {
        self.0.get_text().map_err(|e| e.to_string())
    }
}

/// Create the real backend, or explain why there is none (the reason
/// becomes the status-bar toast). The web build reports the missing
/// clipboard through the same `Err` path.
#[cfg(not(target_arch = "wasm32"))]
pub fn system_backend() -> Result<SystemClipboard, String> {
    arboard::Clipboard::new()
        .map(SystemClipboard)
        .map_err(|e| e.to_string())
}

/// The web stand-in: creation always fails upstream, so this type never
/// operates — it exists to keep the factory signature uniform.
#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub struct SystemClipboard;

#[cfg(target_arch = "wasm32")]
impl ClipboardBackend for SystemClipboard {
    fn set_text(&mut self, _text: String) -> Result<(), String> {
        Err("the clipboard isn't available in the web build yet".to_string())
    }

    fn get_text(&mut self) -> Result<String, String> {
        Err("the clipboard isn't available in the web build yet".to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub fn system_backend() -> Result<SystemClipboard, String> {
    Err("the clipboard isn't available in the web build yet".to_string())
}

/// In-memory stand-in for the OS clipboard, shared by the interaction
/// tests (app.rs drives copy/cut/paste through a boxed instance and reads
/// the results back via `App::clipboard_text`, so no downcast is needed).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct FakeClipboard {
    pub text: Option<String>,
    pub fail_set: bool,
    pub fail_get: bool,
    pub sets: Vec<String>,
}

#[cfg(test)]
impl FakeClipboard {
    pub fn with_text(text: &str) -> Self {
        Self {
            text: Some(text.to_string()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
impl ClipboardBackend for FakeClipboard {
    fn set_text(&mut self, text: String) -> Result<(), String> {
        if self.fail_set {
            return Err("no display".to_string());
        }
        self.sets.push(text.clone());
        self.text = Some(text);
        Ok(())
    }

    fn get_text(&mut self) -> Result<String, String> {
        if self.fail_get {
            return Err("no display".to_string());
        }
        self.text.clone().ok_or_else(|| "empty".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_roundtrip() {
        let mut cb = FakeClipboard::with_text("hello");
        assert_eq!(cb.get_text().as_deref(), Ok("hello"));
        cb.set_text("world".into()).unwrap();
        assert_eq!(cb.get_text().as_deref(), Ok("world"));
        assert_eq!(cb.sets, vec!["world".to_string()]);
    }

    #[test]
    fn failures_surface_as_errors() {
        let mut cb = FakeClipboard {
            fail_set: true,
            fail_get: true,
            ..Default::default()
        };
        assert_eq!(cb.set_text("x".into()), Err("no display".to_string()));
        assert!(cb.get_text().is_err());
    }

    #[test]
    fn creating_the_real_backend_never_panics() {
        // machine-dependent (a display may or may not exist); both
        // outcomes are a Result the app handles
        let _ = system_backend();
    }
}
