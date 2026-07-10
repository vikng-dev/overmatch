//! Guard: every string literal that can reach a Bevy `Text` must be printable ASCII.
//!
//! Bevy's default font is an ASCII-only FiraMono subset with **no fallback**, so a non-ASCII glyph
//! (`…`, `—`, `°`, …) in a rendered string draws tofu on screen. This test scans the files that
//! spawn `Text` and asserts their rendered string literals are ASCII.
//!
//! It must NOT flag comments, nor the log/panic/assert strings the house style deliberately fills
//! with em dashes: those never reach `Text`. So the scanner strips line/block comments and the
//! argument region of every diagnostic macro (`info!`, `error!`, `assert_eq!`, …) and of `.expect(`,
//! then checks what remains. `format!` is intentionally NOT stripped — it is the one macro whose
//! output is routinely handed to `Text::new` (e.g. the connect-status and view-death prompts).

use std::path::PathBuf;

/// The files that spawn `Text`. A rendered non-ASCII glyph can only originate here.
const TEXT_FILES: &[&str] = &[
    "src/hud.rs",
    "src/crew_ui.rs",
    "src/sight.rs",
    "src/state.rs",
    "src/sandbox.rs",
    "src/net/client.rs",
    "src/net/death_screen.rs",
    "src/net/debug_hud.rs",
    "src/net/hit_feel.rs",
    "src/track_sandbox/mod.rs",
];

/// Macros whose string arguments are diagnostics, never rendered — skipped by the scanner.
const DIAGNOSTIC_MACROS: &[&str] = &[
    "info",
    "warn",
    "error",
    "debug",
    "trace",
    "log",
    "dbg",
    "println",
    "eprintln",
    "print",
    "eprint",
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
];

/// One offending literal: the 1-based line it opened on and its (truncated) content.
#[derive(Debug, PartialEq)]
struct Offender {
    line: usize,
    text: String,
}

/// Scan Rust source and return every non-ASCII string literal that is NOT inside a comment and NOT
/// inside a diagnostic-macro / `.expect(` argument region. A hand-rolled tokenizer, because that is
/// exactly the boundary the rule draws (rendered vs. diagnostic) and no lighter heuristic respects
/// it: these files are full of em dashes in `info!`/`assert!` that are perfectly legal.
fn scan(src: &str) -> Vec<Offender> {
    let mut offenders = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1usize;

    // Delimiter depth, and the depth to which we skip diagnostic string checks (set when we enter a
    // diagnostic macro / expect call, cleared when its opening delimiter closes).
    let mut depth = 0i32;
    let mut skip_until: Option<i32> = None;

    // Pending token state for macro / expect detection: `armed` is set once the tokens seen so far
    // are `<diagnostic-macro> !` or `. expect`, i.e. the next opening delimiter starts a skip region.
    let mut prev_ident = String::new();
    let mut armed = false;
    let mut dot_seen = false; // the char before the current ident run was `.`

    while i < chars.len() {
        let c = chars[i];

        // Line comment.
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // Block comment (Rust block comments nest).
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            let mut nest = 1;
            i += 2;
            while i < chars.len() && nest > 0 {
                if chars[i] == '\n' {
                    line += 1;
                } else if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
                    nest += 1;
                    i += 1;
                } else if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                    nest -= 1;
                    i += 1;
                }
                i += 1;
            }
            continue;
        }
        // String literal (raw strings are absent from these files; plain `"..."` with escapes).
        if c == '"' {
            let start_line = line;
            let mut lit = String::new();
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                if d == '\\' && i + 1 < chars.len() {
                    lit.push(d);
                    lit.push(chars[i + 1]);
                    if chars[i + 1] == '\n' {
                        line += 1;
                    }
                    i += 2;
                    continue;
                }
                if d == '"' {
                    i += 1;
                    break;
                }
                if d == '\n' {
                    line += 1;
                }
                lit.push(d);
                i += 1;
            }
            let skipping = skip_until.is_some();
            if !skipping && !lit.is_ascii() {
                let text: String = lit.chars().take(60).collect();
                offenders.push(Offender {
                    line: start_line,
                    text,
                });
            }
            // A string literal is a significant token: it disarms any pending macro detection.
            armed = false;
            prev_ident.clear();
            dot_seen = false;
            continue;
        }
        // Char literal or lifetime: `'a'`, `'\n'`, or `'static`. Consume a char literal so a quote
        // char (`'"'`) can't be mistaken for a string; leave lifetimes to fall through harmlessly.
        if c == '\'' {
            // char literal: '\?x' or 'x' followed by closing '.
            if i + 2 < chars.len() && chars[i + 1] == '\\' {
                // escaped char literal: skip to closing quote
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            if i + 2 < chars.len() && chars[i + 2] == '\'' {
                i += 3;
                continue;
            }
            // lifetime: treat the apostrophe as ordinary punctuation.
            i += 1;
            continue;
        }

        if c == '\n' {
            line += 1;
        }

        // Identifier run.
        if c.is_alphanumeric() || c == '_' {
            let mut ident = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                ident.push(chars[i]);
                i += 1;
            }
            // `. expect (` arms an expect-region.
            if dot_seen && ident == "expect" {
                armed = true;
            } else {
                armed = false;
            }
            prev_ident = ident;
            dot_seen = false;
            continue;
        }

        // Punctuation.
        match c {
            '!' => {
                // `<ident>!` where ident is a diagnostic macro arms a macro-region.
                if DIAGNOSTIC_MACROS.contains(&prev_ident.as_str()) {
                    armed = true;
                } else {
                    armed = false;
                }
                prev_ident.clear();
            }
            '(' | '[' | '{' => {
                if skip_until.is_none() && armed {
                    // This delimiter opens the diagnostic argument region; skip until it closes.
                    skip_until = Some(depth);
                }
                depth += 1;
                armed = false;
                prev_ident.clear();
                dot_seen = false;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                if Some(depth) == skip_until {
                    skip_until = None;
                }
                armed = false;
                prev_ident.clear();
                dot_seen = false;
            }
            '.' => {
                dot_seen = true;
                armed = false;
                prev_ident.clear();
            }
            c if c.is_whitespace() => { /* keep pending state across whitespace */ }
            _ => {
                armed = false;
                prev_ident.clear();
                dot_seen = false;
            }
        }
        i += 1;
    }

    offenders
}

#[test]
fn rendered_strings_are_ascii() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut failures = Vec::new();
    for rel in TEXT_FILES {
        let path = root.join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        for off in scan(&src) {
            failures.push(format!(
                "{rel}:{} contains non-ASCII: {:?}",
                off.line, off.text
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "rendered UI strings must be printable ASCII (Bevy's default font has no non-ASCII \
         fallback — it draws tofu). Offenders:\n{}",
        failures.join("\n")
    );
}

/// The scanner must flag a rendered non-ASCII literal but ignore comments and diagnostic strings —
/// otherwise it would either miss real tofu or fight the house style. Pin both directions.
#[test]
fn scanner_flags_rendered_but_not_comments_or_logs() {
    let sample = r#"
// a comment with an em dash — and an ellipsis … must be ignored
/* block comment — also ignored … */
fn demo() {
    info!("log line — never rendered, ignored …");
    error!("multi\nline — {}", x);
    assert_eq!(a, b, "assert — ignored …");
    let _ = foo.expect("expect — ignored …");
    let ok = format!("rendered {} ...", n);          // ascii, fine
    commands.spawn(Text::new("TOFU…"));              // BAD: rendered non-ascii
    let label = "BROKEN —";                          // BAD: plain literal, rendered path
}
"#;
    let offenders = scan(sample);
    let lines: Vec<usize> = offenders.iter().map(|o| o.line).collect();
    // Exactly the two rendered offenders, nothing from comments / logs / asserts / expect.
    assert!(
        offenders.iter().any(|o| o.text.contains("TOFU")),
        "must flag the Text::new tofu literal; got {offenders:?}"
    );
    assert!(
        offenders.iter().any(|o| o.text.contains("BROKEN")),
        "must flag the plain rendered literal; got {offenders:?}"
    );
    assert_eq!(
        offenders.len(),
        2,
        "must flag ONLY rendered literals, not comments/logs/asserts/expect; got {offenders:?} at lines {lines:?}"
    );
}
