//! Persisted preferences and the recent-files list.
//!
//! Hand-rolled `key=value` lines at `$DBML_UI_CONFIG`, else
//! `$XDG_CONFIG_HOME/dbml-ui/config` (default `~/.config/dbml-ui/config`)
//! — no serde dependency for two keys. Saves are best-effort: preferences
//! are cosmetic, so failures are silent.

/// Serializes config file access. Reentrant: tests hold it across a
/// whole test body while production code inside (`save` from a save
/// flow) takes it again on the same thread — and `save`/`load` take it
/// internally, so no caller can write the config file unserialized
/// (test-suite config files were getting cross-written). Hand-rolled:
/// `std::sync::ReentrantLock` is unstable at this toolchain.
pub static CONFIG_ENV_LOCK: ReentrantFlag = ReentrantFlag {
    owner: std::sync::Mutex::new(None),
    signal: std::sync::Condvar::new(),
};

pub struct ReentrantFlag {
    owner: std::sync::Mutex<Option<std::thread::ThreadId>>,
    signal: std::sync::Condvar,
}

impl ReentrantFlag {
    pub fn lock(&self) -> ReentrantGuard<'_> {
        let id = std::thread::current().id();
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        if *owner == Some(id) {
            // same-thread re-entry: the outermost guard releases
            return ReentrantGuard { flag: self, top: false };
        }
        while owner.is_some() {
            owner = self.signal.wait(owner).unwrap_or_else(|e| e.into_inner());
        }
        *owner = Some(id);
        ReentrantGuard { flag: self, top: true }
    }
}

pub struct ReentrantGuard<'a> {
    flag: &'a ReentrantFlag,
    top: bool,
}

impl Drop for ReentrantGuard<'_> {
    fn drop(&mut self) {
        if self.top {
            let mut owner = self.flag.owner.lock().unwrap_or_else(|e| e.into_inner());
            *owner = None;
            self.flag.signal.notify_all();
        }
    }
}





#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Vim modal editing; false = Regular notepad-style editing.
    pub vim: bool,
    /// Format the source canonically before saving.
    pub fmt_on_save: bool,
    /// Most-recent-first opened files (capped).
    pub recent: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // first run opens as the Regular notepad-style editor; vim is
            // one toolbar toggle away and persists once chosen
            vim: false,
            fmt_on_save: true,
            recent: Vec::new(),
        }
    }
}

/// How many recent files to keep.
pub const RECENT_CAP: usize = 8;

impl Config {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn path() -> std::path::PathBuf {
        #[cfg(test)]
        if let Ok(g) = CONFIG_PATH_OVERRIDE.lock() {
            if let Some(p) = g.as_ref() {
                return p.clone();
            }
        }
        if let Ok(p) = std::env::var("DBML_UI_CONFIG") {
            return std::path::PathBuf::from(p);
        }
        let dir = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                std::path::PathBuf::from(home).join(".config")
            });
        dir.join("dbml-ui").join("config")
    }

    /// Parse the key=value store text (shared by the desktop file and
    /// the web build's IndexedDB record).
    pub fn from_text(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("vim=") {
                c.vim = v.trim() != "0";
            } else if let Some(v) = line.strip_prefix("fmt_on_save=") {
                c.fmt_on_save = v.trim() != "0";
            } else if let Some(p) = line.strip_prefix("recent=") {
                let p = p.trim();
                if !p.is_empty() && !c.recent.iter().any(|r| r == p) {
                    c.recent.push(p.to_string());
                }
            }
        }
        c
    }

    pub fn load() -> Config {
        let _g = CONFIG_ENV_LOCK.lock();
        let mut c = Config::default();
        // the web build has no file system: its config rides the
        // IndexedDB record (see web.rs)
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Ok(text) = std::fs::read_to_string(Self::path()) else {
                return c;
            };
            c = Self::from_text(&text);
        }
        c
    }

    /// The key=value store text (the web build persists this string; the
    /// desktop writes it to the config file).
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "vim={}\nfmt_on_save={}\n",
            if self.vim { 1 } else { 0 },
            if self.fmt_on_save { 1 } else { 0 }
        );
        for r in &self.recent {
            out.push_str("recent=");
            out.push_str(r);
            out.push('\n');
        }
        out
    }

    pub fn save(&self) {
        // the web build persists the settings text in its IndexedDB
        // record (alongside the autosaved state)
        #[cfg(target_arch = "wasm32")]
        crate::web::save_config(&self.to_text());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _g = CONFIG_ENV_LOCK.lock();
            let p = Self::path();
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, self.to_text());
        }
    }

    /// Push to the front, dropping duplicates and overflowing entries.
    pub fn push_recent(&mut self, path: &str) {
        self.recent.retain(|r| r != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(RECENT_CAP);
    }
}

/// Test-only config path override. Process env mutation (`set_var`) is
/// not thread-safe on Unix — it can corrupt a concurrent `getenv` read
/// anywhere in the process, which flaked config asserts ~30% of full
/// runs even with every writer serialized by CONFIG_ENV_LOCK. Tests that
/// need a private config set this instead; no env, no cross-thread race.
#[cfg(test)]
static CONFIG_PATH_OVERRIDE: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub fn set_config_path_override(p: std::path::PathBuf) {
    *CONFIG_PATH_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

#[cfg(test)]
pub fn clear_config_path_override() {
    *CONFIG_PATH_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_recent_rules() {
        let _g = super::CONFIG_ENV_LOCK
            .lock();
        let dir = std::env::temp_dir().join(format!("dbml-ui-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config");
        std::env::set_var("DBML_UI_CONFIG", &path);

        let mut c = Config {
            vim: false,
            fmt_on_save: false,
            recent: vec!["a".into(), "b".into()],
        };
        c.push_recent("c");
        c.push_recent("a"); // dedupe moves it to the front
        c.save();
        let loaded = Config::load();
        assert!(!loaded.vim);
        assert_eq!(loaded.recent, vec!["a", "c", "b"]);

        for i in 0..12 {
            c.push_recent(&format!("f{i}"));
        }
        assert_eq!(c.recent.len(), RECENT_CAP);
        assert_eq!(c.recent[0], "f11");

        std::env::remove_var("DBML_UI_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_default_vim() {
        let _g = super::CONFIG_ENV_LOCK
            .lock();
        std::env::set_var(
            "DBML_UI_CONFIG",
            std::env::temp_dir().join("dbml-ui-cfg-missing-none"),
        );
        assert_eq!(Config::load(), Config::default());
        std::env::remove_var("DBML_UI_CONFIG");
    }
}
