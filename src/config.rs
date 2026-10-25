//! Persisted preferences and the recent-files list.
//!
//! Hand-rolled `key=value` lines at `$DBML_UI_CONFIG`, else
//! `$XDG_CONFIG_HOME/dbml-ui/config` (default `~/.config/dbml-ui/config`)
//! — no serde dependency for two keys. Saves are best-effort: preferences
//! are cosmetic, so failures are silent.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Vim modal editing; false = Regular notepad-style editing.
    pub vim: bool,
    /// Most-recent-first opened files (capped).
    pub recent: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vim: true,
            recent: Vec::new(),
        }
    }
}

/// How many recent files to keep.
pub const RECENT_CAP: usize = 8;

impl Config {
    pub fn path() -> std::path::PathBuf {
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

    pub fn load() -> Config {
        let mut c = Config::default();
        let Ok(text) = std::fs::read_to_string(Self::path()) else {
            return c;
        };
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("vim=") {
                c.vim = v.trim() != "0";
            } else if let Some(p) = line.strip_prefix("recent=") {
                let p = p.trim();
                if !p.is_empty() && !c.recent.iter().any(|r| r == p) {
                    c.recent.push(p.to_string());
                }
            }
        }
        c
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = format!("vim={}\n", if self.vim { 1 } else { 0 });
        for r in &self.recent {
            out.push_str("recent=");
            out.push_str(r);
            out.push('\n');
        }
        let _ = std::fs::write(p, out);
    }

    /// Push to the front, dropping duplicates and overflowing entries.
    pub fn push_recent(&mut self, path: &str) {
        self.recent.retain(|r| r != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(RECENT_CAP);
    }
}

/// Serializes tests that mutate the process-global DBML_UI_CONFIG.
#[cfg(test)]
pub static CONFIG_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_recent_rules() {
        let _g = super::CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dbml-ui-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config");
        std::env::set_var("DBML_UI_CONFIG", &path);

        let mut c = Config {
            vim: false,
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
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(
            "DBML_UI_CONFIG",
            std::env::temp_dir().join("dbml-ui-cfg-missing-none"),
        );
        assert_eq!(Config::load(), Config::default());
        std::env::remove_var("DBML_UI_CONFIG");
    }
}
