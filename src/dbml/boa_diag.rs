//! The diagnostics engine: the vendored dbml-parse compiler (dbdiagram's)
//! running inside Boa, the Rust JS interpreter. This IS the app's
//! diagnostics source — parse, binder and interpret findings all come
//! from their engine, so the squiggles match dbdiagram on every input by
//! construction (the spike: 987/987 files byte-identical to node,
//! parity/parity.md THE BOA SPIKE).
//!
//! The native parser stays the structural backbone: it builds the model
//! (canvas, completion, formatter, exporters) and runs synchronously per
//! keystroke. Their engine is slow under Boa (~0.4-1.1s per file), so it
//! runs on a worker thread whose inbox drains to the latest request —
//! typing floods collapse into one parse, and the results arrive one
//! generation late at worst.

use boa_engine::{Context, Source};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// The compiled engine bundle (the vendored dbml-parse, esbuild'd to a
/// self-contained IIFE exposing the `DBML` global). Regenerate with:
/// esbuild index.ts --bundle --format=iife --global-name=DBML
///   --alias:@=. --outfile=parity/bundle/dbml-parse.iife.js
const BUNDLE: &str = include_str!("../../parity/bundle/dbml-parse.iife.js");

/// The host shims their code touches at load/run time (Boa is a bare
/// engine — no node globals).
const SHIMS: &str = "var console = { log(){}, warn(){}, error(){}, info(){}, debug(){}, trace(){} };\
var process = { env: {}, argv: [], platform: 'linux', version: 'v0.0.0', cwd: function(){ return '/'; }, exit: function(){} };";

/// The batch driver, verbatim from `theirs-dump-many.ts`'s dump shape —
/// the same contract the parity harness verifies (987/987).
const DRIVER: &str = r#"
function __dumpAll(cases) {
  const dump = (ds) => ds.map((d) => ({
    type: d.type,
    text: d.text,
    row: d.startRow,
    column: d.startColumn ?? null,
    endRow: d.endRow ?? null,
    endColumn: d.endColumn ?? null,
    category: d.category ?? null,
    explanation: d.explanation ?? null,
    code: d.code ?? null,
    quickFixes: (d.quickFixes ?? []).map((q) => ({
      title: q.title ?? null,
      shortTitle: q.shortTitle ?? null,
      isPreferred: q.isPreferred ?? null,
      newText: (q.edits ?? []).map((e) => e.newText).join(' | '),
      edits: (q.edits ?? []).map((e) => ({ start: e.start, end: e.end, newText: e.newText })),
    })),
  }));
  const layout = new DBML.MemoryProjectLayout();
  const compiler = new DBML.Compiler(layout);
  const provider = new DBML.DBMLDiagnosticsProvider(compiler);
  const out = [];
  for (const c of cases) {
    const fp = DBML.Filepath.from('/corpus/' + c.name);
    layout.setSource(fp, c.content);
    const parse = compiler.parseFile(fp).getValue();
    const diags = [
      ...dump(provider.provideErrors(fp)),
      ...dump(provider.provideWarnings(fp)),
      ...dump(provider.provideInfos(fp)),
    ];
    out.push({ name: c.name, parseOk: parse !== undefined, diagnostics: diags });
  }
  return JSON.stringify(out);
}
"#;

/// One finding from their engine, translated into the app's coordinate
/// space: byte offsets on the source, ready for
/// `canvas::editor::error_marks` and the diagnostics panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoaDiag {
    pub offset: usize,
    /// The finding's end offset (their endColumn) when the offending
    /// node spans past the anchor.
    pub end: Option<usize>,
    pub message: String,
    pub severity: Severity,
    pub fixes: Vec<BoaFix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Their quick fix, translated into our coordinate space (the first
/// edit — their multi-edit fixes join into one replacement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoaFix {
    pub title: String,
    pub short_title: String,
    pub start: usize,
    pub end: usize,
    pub new_text: String,
    pub is_preferred: bool,
}

/// Their text offsets count UTF-16 code units; ours count bytes.
fn utf16_offset_to_byte(src: &str, u: usize) -> usize {
    let mut units = 0usize;
    for (i, ch) in src.char_indices() {
        if units >= u {
            return i;
        }
        units += ch.len_utf16();
    }
    src.len()
}

/// (row, utf16 col) — their positions — to a byte offset in `src`.
/// Their columns count UTF-16 code units; ours count bytes.
fn offset_at(src: &str, row: usize, utf16_col: usize) -> usize {
    let mut line_start = 0;
    for _ in 0..row {
        match src[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return src.len(),
        }
    }
    let line = &src[line_start..];
    let line_end = line.find('\n').map(|i| line_start + i).unwrap_or(src.len());
    let mut units = 0usize;
    let mut off = line_start;
    for ch in src[line_start..line_end].chars() {
        if units >= utf16_col {
            break;
        }
        units += ch.len_utf16();
        off += ch.len_utf8();
    }
    off
}

/// The synchronous engine: one Boa context running the bundled compiler.
pub struct Engine {
    context: Context,
}

impl Engine {
    pub fn new() -> Result<Self, String> {
        let mut context = Context::default();
        context
            .eval(Source::from_bytes(SHIMS.as_bytes()))
            .map_err(|e| format!("shims: {e}"))?;
        context
            .eval(Source::from_bytes(BUNDLE.as_bytes()))
            .map_err(|e| format!("bundle: {e}"))?;
        context
            .eval(Source::from_bytes(DRIVER.as_bytes()))
            .map_err(|e| format!("driver: {e}"))?;
        Ok(Self { context })
    }

    /// The diagnostics for one source document, theirs' positions
    /// translated to our byte offsets. Blocks for ~0.4-1.1s — the
    /// worker thread is the caller-facing entry point; this direct form
    /// exists for the tests.
    pub fn parse(&mut self, src: &str) -> Vec<BoaDiag> {
        let cases = serde_json::json!([{ "name": "doc", "content": src }]);
        let call = format!("__dumpAll({})", cases);
        let Ok(result) = self.context.eval(Source::from_bytes(call.as_bytes())) else {
            return Vec::new();
        };
        let Some(js) = result.as_string() else {
            return Vec::new();
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&js.to_std_string_escaped())
        else {
            return Vec::new();
        };
        let Some(items) = parsed.as_array() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for item in items {
            for d in item["diagnostics"].as_array().unwrap_or(&Vec::new()) {
                let row = d["row"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
                let col = d["column"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
                let offset = offset_at(src, row, col);
                let end = match (d["endRow"].as_u64(), d["endColumn"].as_u64()) {
                    (Some(er), Some(ec)) => {
                        let e = offset_at(
                            src,
                            er.saturating_sub(1) as usize,
                            ec.saturating_sub(1) as usize,
                        );
                        (e > offset).then_some(e)
                    }
                    _ => None,
                };
                let severity = match d["type"].as_str() {
                    Some("warning") => Severity::Warning,
                    Some("info") => Severity::Info,
                    _ => Severity::Error,
                };
                let fixes = d["quickFixes"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|q| {
                        let edits = q["edits"].as_array().cloned().unwrap_or_default();
                        let (start, end, new_text) = match edits.first() {
                            Some(e) => (
                                utf16_offset_to_byte(src, e["start"].as_u64().unwrap_or(0) as usize),
                                utf16_offset_to_byte(src, e["end"].as_u64().unwrap_or(0) as usize),
                                e["newText"].as_str().unwrap_or("").to_string(),
                            ),
                            None => (0, 0, String::new()),
                        };
                        BoaFix {
                            title: q["title"].as_str().unwrap_or("").to_string(),
                            short_title: q["shortTitle"].as_str().unwrap_or("").to_string(),
                            start,
                            end,
                            new_text,
                            is_preferred: q["isPreferred"].as_bool().unwrap_or(false),
                        }
                    })
                    .collect();
                out.push(BoaDiag {
                    offset,
                    end,
                    message: d["text"].as_str().unwrap_or("").to_string(),
                    severity,
                    fixes,
                });
            }
        }
        out
    }
}

impl BoaDiag {
    pub fn to_diagnostic(&self) -> super::validate::Diagnostic {
        super::validate::Diagnostic {
            offset: self.offset,
            end: self.end,
            message: self.message.clone(),
            severity: match self.severity {
                Severity::Error => super::validate::Severity::Error,
                Severity::Warning => super::validate::Severity::Warning,
                Severity::Info => super::validate::Severity::Info,
            },
            category: None,
            explanation: Vec::new(),
            quick_fixes: self
                .fixes
                .iter()
                .map(|f| super::validate::QuickFix {
                    title: f.title.clone(),
                    short_title: f.short_title.clone(),
                    span: f.start..f.end,
                    new_text: f.new_text.clone(),
                    is_preferred: f.is_preferred,
                })
                .collect(),
        }
    }
}

pub struct Request {
    pub gen: u64,
    pub src: String,
}

pub struct Reply {
    pub gen: u64,
    pub diags: Vec<BoaDiag>,
}

/// The worker: one thread, one Boa context, the inbox drains to the
/// LATEST request (typing floods collapse into one parse), replies carry
/// the generation so the app can drop stale results.
fn spawn_worker() -> (Sender<Request>, Mutex<Receiver<Reply>>) {
    let (tx, rx_in) = channel::<Request>();
    let (tx_out, rx_out) = channel::<Reply>();
    std::thread::spawn(move || {
        let Ok(mut engine) = Engine::new() else {
            return; // no engine, no diagnostics: the marks stay empty
        };
        loop {
            let Ok(mut req) = rx_in.recv() else { return };
            while let Ok(newer) = rx_in.try_recv() {
                req = newer;
            }
            let diags = engine.parse(&req.src);
            if tx_out.send(Reply { gen: req.gen, diags }).is_err() {
                return;
            }
        }
    });
    (tx, Mutex::new(rx_out))
}

/// The process-wide worker. The editor is a single instance; the engine
/// boots once, lazily, off the UI thread.
pub fn worker() -> &'static (Sender<Request>, Mutex<Receiver<Reply>>) {
    static WORKER: OnceLock<(Sender<Request>, Mutex<Receiver<Reply>>)> = OnceLock::new();
    WORKER.get_or_init(spawn_worker)
}

/// Kick a diagnostics request (non-blocking). `gen` = the caller's
/// monotonically increasing document generation.
pub fn request(gen: u64, src: String) {
    let _ = worker().0.send(Request { gen, src });
}

/// The reply as the app-facing diagnostics.
impl Reply {
    pub fn diagnostics(&self) -> Vec<super::validate::Diagnostic> {
        self.diags.iter().map(|d| d.to_diagnostic()).collect()
    }
}

/// Poll for the newest finished reply, if any.
pub fn try_recv() -> Option<Reply> {
    let (_, rx) = worker();
    let guard = rx.lock().ok()?;
    guard.try_recv().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_table_is_clean() {
        let mut e = Engine::new().expect("engine");
        assert!(e.parse("Table t {\n  id int\n}\n").is_empty());
    }

    #[test]
    fn missing_column_survives_their_recovery() {
        // the user's errs.dbml shape: the broken settings bracket keeps
        // the column REGISTERED in their recovery, so the later refs
        // resolve — ours' native parse flagged phantom missing columns
        let mut e = Engine::new().expect("engine");
        let src = "Table posts {\n  id int\n  user_id integer [not null\n}\nRef r: posts.user_id > users.id\n";
        let diags = e.parse(src);
        assert!(
            !diags.iter().any(|d| d.message.contains("'user_id' does not exist")),
            "their recovery keeps user_id bound: {diags:?}"
        );
    }
}
