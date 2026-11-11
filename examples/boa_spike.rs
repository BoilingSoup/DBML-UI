//! The Boa spike: run the vendored dbml-parse bundle inside the Boa JS
//! engine and dump the diagnostics contract — the fidelity check vs the
//! node driver (`parity/driver/theirs-dump-many.ts`). If the same bundle
//! under Boa produces byte-identical diagnostics to Node on the corpus,
//! the battery and the seeds, the diagnostics-oracle embed is proven.
//!
//! Usage: boa_spike <cases.json> [bundle.js]
//!   cases.json = [{name, path}] — the same shape the driver takes; the
//!   file contents are read here and inlined (no fs inside the engine).

use boa_engine::{Context, Source};
use std::fs;
use std::time::Instant;

/// The batch driver, verbatim from theirs-dump-many.ts's dump shape:
/// same contract JSON out, so the compare tooling is unchanged.
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: boa_spike <cases.json> [bundle.js]");
        std::process::exit(2);
    }
    let cases_path = &args[1];
    let bundle_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "/tmp/dbml-parse-bundle.js".to_string());

    let bundle = fs::read_to_string(&bundle_path).expect("bundle read");
    let cases: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(cases_path).expect("cases read")).expect("cases json");
    let inlined: Vec<serde_json::Value> = cases
        .as_array()
        .expect("cases array")
        .iter()
        .map(|c| {
            let content = fs::read_to_string(c["path"].as_str().expect("path")).expect("case read");
            serde_json::json!({ "name": c["name"], "content": content })
        })
        .collect();
    eprintln!("cases: {}", inlined.len());

    let mut context = Context::default();

    // Boa is a bare engine: no host objects. Their bundle touches
    // `console` at load time — provide the no-op shim first.
    context
        .eval(Source::from_bytes(
            b"var console = { log(){}, warn(){}, error(){}, info(){}, debug(){}, trace(){} };\
              var process = { env: {}, argv: [], platform: 'linux', version: 'v0.0.0', cwd: function(){ return '/'; }, exit: function(){} };",
        ))
        .expect("host shims");

    let t = Instant::now();
    context
        .eval(Source::from_bytes(bundle.as_bytes()))
        .expect("bundle eval");
    eprintln!("bundle eval: {:?}", t.elapsed());

    let t = Instant::now();
    context
        .eval(Source::from_bytes(DRIVER.as_bytes()))
        .expect("driver eval");
    eprintln!("driver eval: {:?}", t.elapsed());

    // the cases JSON object literal = valid JS — no string-escaping seam;
    // the try/catch surfaces the JS-side error message + stack instead of
    // an opaque Opaque(Object) JsError
    let call = format!(
        "JSON.stringify((function(){{ try {{ return {{ ok: __dumpAll({}) }}; }} catch (e) {{ return {{ threw: String(e && e.message || e), stack: e && e.stack || null }}; }}}})())",
        serde_json::to_string(&inlined).expect("cases stringify")
    );
    let t = Instant::now();
    let result = context
        .eval(Source::from_bytes(call.as_bytes()))
        .expect("batch run");
    eprintln!("batch run: {:?}", t.elapsed());

    let s = result
        .as_string()
        .expect("string result")
        .to_std_string_escaped();
    let parsed: serde_json::Value = serde_json::from_str(&s).expect("result json");
    if let Some(err) = parsed.get("threw") {
        eprintln!("JS THREW: {}\nstack: {}", err, parsed.get("stack").cloned().unwrap_or(serde_json::Value::Null));
        std::process::exit(1);
    }
    println!(
        "{}",
        serde_json::to_string(&parsed["ok"]).expect("reserialize")
    );
}
