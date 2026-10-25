// Probe: parse a DBML file and print the canonical form (or the parse
// errors). usage: cargo run --example parse_probe -- <file.dbml>
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: parse_probe <file.dbml>");
        std::process::exit(2);
    }
    let mut src = std::fs::read_to_string(&args[1]).unwrap();
    // replicate the fixture test's cardinality-artifact normalization
    for (a, b) in [
        ("?<<?>>", "<>"),
        ("?<><?", "<>"),
        ("<<?>>", "<>"),
        ("?<<?", "<"),
        ("?>?", ">"),
        ("?<?", "<"),
        ("?-?", "-"),
        ("?<>", "<>"),
        ("<>? ", "<> "),
        ("?<", "<"),
        ("?>", ">"),
        ("?-", "-"),
        ("<<?", "<"),
        (">? ", "> "),
        ("-? ", "- "),
        ("<?", "<"),
    ] {
        src = src.replace(a, b);
    }
    match dbml_ui::dbml::fmt::format_source(&src) {
        Some(f) => println!("{f}"),
        None => {
            let out = dbml_ui::dbml::parser::parse(&src);
            eprintln!("!! format_source failed; parse errors:");
            for e in &out.errors {
                // show the offending source line for context
                let line = src[..e.offset.min(src.len())].matches('\n').count() + 1;
                let text = src.lines().nth(line - 1).unwrap_or("").to_string();
                eprintln!("  line {line}: {} | {text}", e.message);
            }
        }
    }
}
