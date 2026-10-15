//! Error messages and hints in the words of the MCP tools. The readers and the core write them
//! for the command line (`openreadout trace FILE --sweep N`, `pass --overwrite`); an agent
//! calling tools needs the tool and argument names instead (`openreadout_trace` with
//! `sweep: N`, `overwrite: true`).

/// Rewrite the command lines quoted in `s` (between backticks) and bare `--flag`s.
pub fn for_tools(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('`') {
        let Some(len) = rest[start + 1..].find('`') else {
            break;
        };
        out.push_str(&bare_flags(&rest[..start]));
        let quoted = &rest[start + 1..start + 1 + len];
        match quoted.strip_prefix("openreadout ") {
            Some(cmd) => out.push_str(&command(cmd)),
            None => {
                out.push('`');
                out.push_str(quoted);
                out.push('`');
            }
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(&bare_flags(rest));
    out
}

/// `--overwrite` becomes `overwrite: true`; another `--some-flag` becomes `some_flag`.
fn bare_flags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("--") {
        let flag: String = rest[i + 2..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let at_word_start = i == 0 || rest[..i].ends_with([' ', '(', '"']);
        out.push_str(&rest[..i]);
        if flag.is_empty() || !at_word_start {
            out.push_str("--");
            out.push_str(&flag);
        } else if flag == "overwrite" {
            out.push_str("`overwrite: true`");
        } else {
            out.push('`');
            out.push_str(&flag.replace('-', "_"));
            out.push('`');
        }
        rest = &rest[i + 2 + flag.len()..];
    }
    out.push_str(rest);
    out
}

/// One quoted command line (after `openreadout `) as a tool call.
fn command(cmd: &str) -> String {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let Some(&first) = words.first() else {
        return "`openreadout`".into();
    };
    let (tool, mut args, rest) = match (first, words.get(1).copied()) {
        ("analyze", Some(kind)) => {
            let assay = match kind {
                "assay-wells" => Some("wells"),
                "assay-curve" => Some("curve"),
                "assay-qc" => Some("qc"),
                "dose-response" | "kinetics" | "growth" => Some(kind),
                _ => None,
            };
            match assay {
                Some(a) => ("openreadout_assay".to_string(), vec![format!("analysis: \"{a}\"")], &words[2..]),
                None => (format!("openreadout_{}", kind.replace('-', "_")), Vec::new(), &words[2..]),
            }
        }
        ("self", Some("formats")) => return "the `openreadout://formats` resource".into(),
        ("help", _) => return "the tool's input schema".into(),
        ("self" | "mcp" | "--version", _) => return format!("`openreadout {cmd}`"),
        (c, _) if c.starts_with('-') => return format!("`openreadout {cmd}`"),
        (c, _) => (format!("openreadout_{}", c.replace('-', "_")), Vec::new(), &words[1..]),
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let name = w.strip_prefix("--").or_else(|| (w == "-o").then_some("output"));
        match name {
            Some("json" | "jsonl") => {}
            Some(n) => {
                let n = n.replace('-', "_");
                match rest.get(i + 1) {
                    Some(v) if !v.starts_with('-') => {
                        args.push(format!("{n}: {v}"));
                        i += 1;
                    }
                    _ => args.push(format!("{n}: true")),
                }
            }
            None => {}
        }
        i += 1;
    }
    if args.is_empty() {
        format!("`{tool}`")
    } else {
        format!("`{tool}` with `{}`", args.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::for_tools;

    #[test]
    fn command_lines_become_tool_calls() {
        assert_eq!(
            for_tools("Indices are zero-based; `openreadout info FILE --json` lists the images."),
            "Indices are zero-based; `openreadout_info` lists the images."
        );
        assert_eq!(
            for_tools("Read one with `openreadout trace FILE --trace N --sweep M`."),
            "Read one with `openreadout_trace` with `trace: N, sweep: M`."
        );
        assert_eq!(
            for_tools("x.png exists; pass --overwrite to replace it"),
            "x.png exists; pass `overwrite: true` to replace it"
        );
        assert_eq!(
            for_tools("Use `openreadout analyze growth` or `openreadout analyze nmr-peaks`."),
            "Use `openreadout_assay` with `analysis: \"growth\"` or `openreadout_nmr_peaks`."
        );
        assert_eq!(
            for_tools("`openreadout self formats` lists them; `openreadout info --view structure`"),
            "the `openreadout://formats` resource lists them; `openreadout_info` with `view: structure`"
        );
        assert_eq!(for_tools("a range 1--2 and `x --y`"), "a range 1--2 and `x --y`");
    }
}
