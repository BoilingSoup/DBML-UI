//! Session records, two kinds:
//!
//! - a crash snapshot (`clean 0`): every open tab's buffer (including
//!   unsaved edits), cursor, vim mode, viewport scroll, and backing file.
//!   Written atomically (tmp + rename) a couple of seconds after the last
//!   edit, and restored only after the wgpu device-loss restart (screen
//!   blank / suspend kills the GPU device and wgpu 0.19 panics with no
//!   error path) — the restart comes back with every tab and edit intact
//!   instead of dropping unsaved work.
//! - a clean-exit record (`clean 1`): just the open files' paths, tab
//!   order, active tab and split — no buffer text. Written on a manual
//!   exit; the next start reopens the files from disk (paths that fail
//!   skip silently).
//!
//! Hand-rolled length-prefixed text, no serde: buffer text is arbitrary
//! UTF-8, so sections carry byte lengths rather than trusting newlines.

const MAGIC: &str = "dbml-ui session 1";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionTab {
    /// Backing file, if any (`None` = an unsaved/Untitled diagram).
    pub file: Option<String>,
    /// The editor buffer text at snapshot time.
    pub text: String,
    /// Cursor (row, col) in chars.
    pub cursor: (usize, usize),
    /// `editor::Mode` as 0 = Normal, 1 = Insert, 2 = Visual.
    pub mode: u8,
    /// Editor viewport scroll, in lines / px (horizontal).
    pub scroll: f32,
    pub hscroll: f32,
    /// False only for a start-screen tab.
    pub doc_open: bool,
    /// Dragged table positions (cells): table title → (x, y). Empty
    /// when nothing was ever moved — restore then auto-arranges.
    pub table_pos: Vec<(String, i32, i32)>,
    /// Dragged/resized sticky rects (cells): name → (x, y, w, h).
    pub note_pos: Vec<(String, i32, i32, i32, i32)>,
    /// Raise order across tables and stickies, most recently grabbed
    /// last: (true = Table, false = Note), name.
    pub item_z: Vec<(bool, String)>,
    /// The TableGroups' own raise order, most recently grabbed last
    /// (the group z tier — groups still paint under every table).
    pub group_z: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Session {
    pub active: usize,
    pub split: f32,
    pub tabs: Vec<SessionTab>,
    /// `true` = a clean-exit record (file paths only, buffers empty);
    /// `false` = a crash snapshot. Absent in pre-`clean` files, which were
    /// always crash snapshots.
    pub clean: bool,
}

/// `$DBML_UI_SESSION`, else the config dir's `session` file (next to the
/// config key=value store).
pub fn path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("DBML_UI_SESSION") {
        return std::path::PathBuf::from(p);
    }
    let dir = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            std::path::PathBuf::from(home).join(".config")
        });
    dir.join("dbml-ui").join("session")
}

/// Atomic write (tmp + rename): a process dying mid-write can never leave
/// a half-formed session behind to be restored.
pub fn save(s: &Session) -> std::io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, encode(s))?;
    std::fs::rename(&tmp, &path)
}

pub fn load() -> Option<Session> {
    let text = std::fs::read_to_string(path()).ok()?;
    decode(&text)
}

pub fn encode(s: &Session) -> String {
    let mut out = String::new();
    out.push_str(MAGIC);
    out.push('\n');
    out.push_str(&format!("active {}\n", s.active));
    out.push_str(&format!("split {}\n", s.split));
    out.push_str(&format!("clean {}\n", u8::from(s.clean)));
    out.push_str(&format!("tabs {}\n", s.tabs.len()));
    for t in &s.tabs {
        match &t.file {
            Some(f) => out.push_str(&format!("file {}\n", f.len())),
            None => out.push_str("file -1\n"),
        }
        if let Some(f) = &t.file {
            out.push_str(f);
        }
        out.push_str(&format!("text {}\n", t.text.len()));
        out.push_str(&t.text);
        out.push_str(&format!(
            "cursor {} {} {} {} {} {}\n",
            t.cursor.0, t.cursor.1, t.mode, t.scroll, t.hscroll, u8::from(t.doc_open)
        ));
        // diagram layout: dragged positions and the raise order. Names
        // go LAST on the line (they may contain spaces — quoted DBML
        // names); the leading fields are fixed-count integers.
        for (name, x, y) in &t.table_pos {
            out.push_str(&format!("tpos {x} {y} {name}\n"));
        }
        for (name, x, y, w, h) in &t.note_pos {
            out.push_str(&format!("npos {x} {y} {w} {h} {name}\n"));
        }
        for (is_table, name) in &t.item_z {
            out.push_str(&format!("z {} {name}\n", if *is_table { 'T' } else { 'N' }));
        }
        // the groups' own raise order (the group z tier)
        for name in &t.group_z {
            out.push_str(&format!("gz {name}\n"));
        }
    }
    out
}

pub fn decode(text: &str) -> Option<Session> {
    let mut rest = text.strip_prefix(MAGIC)?.strip_prefix('\n')?;
    let mut session = Session::default();
    let mut next_tab: Option<SessionTab> = None;

    let take_line = |rest: &mut &str| -> Option<String> {
        let end = rest.find('\n')?;
        let line = rest[..end].to_string();
        *rest = &rest[end + 1..];
        Some(line)
    };
    // exact-length section read: `len` byte-count prefixes, content may
    // contain any bytes of UTF-8 (newlines included)
    let take_block = |rest: &mut &str, len: usize| -> Option<String> {
        if len > rest.len() {
            return None;
        }
        let (head, tail) = rest.split_at(len);
        *rest = tail;
        Some(head.to_string())
    };

    loop {
        let line = take_line(&mut rest)?;
        let (key, val) = line.split_once(' ')?;
        match key {
            "active" => session.active = val.parse().ok()?,
            "split" => session.split = val.parse().ok()?,
            "clean" => session.clean = val == "1",
            "tabs" => {
                let n: usize = val.parse().ok()?;
                session.tabs = Vec::with_capacity(n);
            }
            "file" => {
                // a new tab section begins; commit the previous one
                if let Some(t) = next_tab.take() {
                    session.tabs.push(t);
                }
                let len: i64 = val.parse().ok()?;
                let file = if len < 0 {
                    None
                } else {
                    Some(take_block(&mut rest, len as usize)?)
                };
                next_tab = Some(SessionTab {
                    file,
                    text: String::new(),
                    cursor: (0, 0),
                    mode: 0,
                    scroll: 0.0,
                    hscroll: 0.0,
                    doc_open: true,
                    ..Default::default()
                });
            }
            "text" => {
                let t = next_tab.as_mut()?;
                t.text = take_block(&mut rest, val.parse().ok()?)?;
            }
            "cursor" => {
                let t = next_tab.as_mut()?;
                let mut nums = val.split(' ');
                t.cursor = (nums.next()?.parse().ok()?, nums.next()?.parse().ok()?);
                t.mode = nums.next()?.parse().ok()?;
                t.scroll = nums.next()?.parse().ok()?;
                t.hscroll = nums.next()?.parse().ok()?;
                t.doc_open = nums.next()? == "1";
            }
            // diagram layout lines (all optional; older sessions without
            // them decode to empty — restore then auto-arranges)
            "tpos" => {
                let t = next_tab.as_mut()?;
                let mut parts = val.splitn(3, ' ');
                let x = parts.next()?.parse().ok()?;
                let y = parts.next()?.parse().ok()?;
                t.table_pos.push((parts.next()?.to_string(), x, y));
            }
            "npos" => {
                let t = next_tab.as_mut()?;
                let mut parts = val.splitn(5, ' ');
                let x = parts.next()?.parse().ok()?;
                let y = parts.next()?.parse().ok()?;
                let w = parts.next()?.parse().ok()?;
                let h = parts.next()?.parse().ok()?;
                t.note_pos.push((parts.next()?.to_string(), x, y, w, h));
            }
            "z" => {
                let t = next_tab.as_mut()?;
                let (kind, name) = val.split_once(' ')?;
                t.item_z.push((kind == "T", name.to_string()));
            }
            "gz" => {
                let t = next_tab.as_mut()?;
                t.group_z.push(val.to_string());
            }
            _ => return None,
        }
        if rest.is_empty() {
            break;
        }
    }
    if let Some(t) = next_tab.take() {
        session.tabs.push(t);
    }
    (!session.tabs.is_empty()).then_some(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips_diagram_layout() {
        let s = Session {
            active: 0,
            split: 0.5,
            clean: false,
            tabs: vec![SessionTab {
                file: Some("/tmp/x.dbml".into()),
                text: "Table users { id integer }\n".into(),
                cursor: (0, 0),
                mode: 1,
                scroll: 0.0,
                hscroll: 0.0,
                doc_open: true,
                table_pos: vec![
                    ("users".into(), 12, -3),
                    ("order items".into(), 40, 7), // quoted name with a space
                ],
                note_pos: vec![("idea".into(), 2, 9, 14, 5)],
                item_z: vec![(false, "idea".into()), (true, "users".into())],
                group_z: vec!["staff".into(), "order group".into()], // quoted name with a space
            }],
        };
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    #[test]
    fn session_round_trips_multiline_text_and_files() {
        let s = Session {
            active: 1,
            split: 0.42,
            clean: true,
            tabs: vec![
                SessionTab {
                    file: Some("/tmp/a b.dbml".into()),
                    text: "Table t {\n  id — integer [pk]\n}\n".into(),
                    cursor: (2, 7),
                    mode: 1,
                    scroll: 3.0,
                    hscroll: 12.5,
                    doc_open: true,
                ..Default::default()
                },
                SessionTab {
                    file: None,
                    text: String::new(),
                    cursor: (0, 0),
                    mode: 0,
                    scroll: 0.0,
                    hscroll: 0.0,
                    doc_open: false,
                ..Default::default()
                },
            ],
        };
        let decoded = decode(&encode(&s)).expect("decodes");
        assert_eq!(decoded, s);
    }

    /// A clean-exit record carries paths with no buffer text, and still
    /// round-trips.
    #[test]
    fn clean_record_round_trips_without_text() {
        let s = Session {
            active: 0,
            split: 0.5,
            clean: true,
            tabs: vec![SessionTab {
                file: Some("/tmp/x.dbml".into()),
                text: String::new(),
                cursor: (0, 0),
                mode: 0,
                scroll: 0.0,
                hscroll: 0.0,
                doc_open: true,
                ..Default::default()
            }],
        };
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    /// Sessions written before the clean flag existed decode as crash
    /// snapshots (clean = false), so a plain upgrade keeps the restore
    /// semantics they were written with.
    #[test]
    fn pre_clean_format_decodes_as_a_crash_snapshot() {
        let old = "dbml-ui session 1\nactive 0\nsplit 0.4\ntabs 1\nfile -1\ntext 10\nTable t {}cursor 0 0 0 0 0 1\n";
        let s = decode(old).expect("old format still decodes");
        assert!(!s.clean);
        assert_eq!(s.tabs[0].text, "Table t {}");
    }

    #[test]
    fn truncated_sessions_reject_cleanly() {
        let s = Session {
            active: 0,
            split: 0.4,
            clean: false,
            tabs: vec![SessionTab {
                file: None,
                text: "Table t {}".into(),
                cursor: (0, 0),
                mode: 0,
                scroll: 0.0,
                hscroll: 0.0,
                doc_open: true,
                ..Default::default()
            }],
        };
        let full = encode(&s);
        // every truncation either decodes to None or a shorter-but-valid
        // session — never panics
        for cut in 0..full.len() {
            let _ = decode(&full[..cut]);
        }
        assert!(decode(&full).is_some());
        assert!(decode("garbage").is_none());
    }
}
