//! Ex command parsing (`:w`, `:q`, `:e <file>`, ...).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExCommand {
    /// `:w [path]` — write (optionally to a path).
    Write(Option<String>),
    /// `:q` — quit (refuses when there are unsaved changes unless force).
    Quit {
        force: bool,
    },
    /// `:wq` / `:x` — write and quit.
    WriteQuit,
    /// `:o <file>` / `:e <file>` — open/edit a file (force overrides unsaved
    /// changes).
    Open {
        path: String,
        force: bool,
    },
    /// Recognized but implemented in a later milestone.
    Export(String),
    Unknown(String),
}

/// Parse an ex command line (without the leading ':').
pub fn parse_ex(input: &str) -> ExCommand {
    let input = input.trim();
    if input.is_empty() {
        return ExCommand::Unknown(String::new());
    }
    let (head, rest) = match input.find(char::is_whitespace) {
        Some(i) => (&input[..i], input[i..].trim_start()),
        None => (input, ""),
    };
    let (cmd, force) = match head.strip_suffix('!') {
        Some(c) => (c, true),
        None => (head, false),
    };
    match cmd {
        "w" | "write" => ExCommand::Write(if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        }),
        "q" | "quit" => ExCommand::Quit { force },
        "wq" | "x" => ExCommand::WriteQuit,
        "o" | "open" | "e" | "edit" => {
            if rest.is_empty() {
                ExCommand::Unknown(format!(":{cmd} requires a file name"))
            } else {
                ExCommand::Open {
                    path: rest.to_string(),
                    force,
                }
            }
        }
        "export" => ExCommand::Export(rest.to_string()),
        _ => ExCommand::Unknown(format!("not an editor command: {cmd}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_commands() {
        assert_eq!(parse_ex("w"), ExCommand::Write(None));
        assert_eq!(
            parse_ex("w out.dbml"),
            ExCommand::Write(Some("out.dbml".into()))
        );
        assert_eq!(parse_ex("write"), ExCommand::Write(None));
        assert_eq!(parse_ex("q"), ExCommand::Quit { force: false });
        assert_eq!(parse_ex("q!"), ExCommand::Quit { force: true });
        assert_eq!(parse_ex("quit!"), ExCommand::Quit { force: true });
        assert_eq!(parse_ex("wq"), ExCommand::WriteQuit);
        assert_eq!(parse_ex("x"), ExCommand::WriteQuit);
        assert_eq!(
            parse_ex("e foo.dbml"),
            ExCommand::Open {
                path: "foo.dbml".into(),
                force: false
            }
        );
        assert_eq!(
            parse_ex("open  spaced.dbml"),
            ExCommand::Open {
                path: "spaced.dbml".into(),
                force: false
            }
        );
        assert_eq!(
            parse_ex("e! x.dbml"),
            ExCommand::Open {
                path: "x.dbml".into(),
                force: true
            }
        );
        assert_eq!(
            parse_ex("export diagram.svg"),
            ExCommand::Export("diagram.svg".into())
        );
        assert_eq!(
            parse_ex("zzz"),
            ExCommand::Unknown("not an editor command: zzz".into())
        );
    }
}
