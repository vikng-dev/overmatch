//! Guard: every string literal that can reach a Bevy `Text` must stay within the bundled UI font's
//! glyph coverage.
//!
//! The client now ships Barlow Condensed (`assets/fonts/`, loaded by `ui_font`), which covers the
//! full printable ASCII range plus the small typographic set the UI actually uses ([`TYPOGRAPHIC_SET`]:
//! `… — – ° × ± ≤`). Every one of those was verified present in BOTH shipped weights' cmaps
//! (SemiBold and Regular). A glyph outside that coverage still has no fallback — it draws tofu on
//! screen — so this test scans the files that spawn `Text` and flags any rendered string literal
//! containing a character that is neither ASCII nor in the verified set. Anything more exotic than
//! this set must earn its place with a fresh cmap check against the shipped `.ttf`s before it can be
//! added here (see the rule in `.agents/AGENTS.md`).
//!
//! **The typographic allowance is per-file, because the font isn't uniform.** Only the Barlow-threaded
//! client files ([`BARLOW_TEXT_FILES`]) render through `ui_font::UiFonts`; the dev sandboxes
//! ([`ASCII_ONLY_TEXT_FILES`]: `sandbox.rs`, `track_sandbox/mod.rs`) keep Bevy's default ASCII-only
//! font (AGENTS.md), whose cmap does NOT carry `… — –` &c. So the sandbox files are held to pure
//! printable ASCII, and only the Barlow files get the typographic extension.
//!
//! It must NOT flag comments, nor the log/panic/assert strings the house style deliberately fills
//! with em dashes: those never reach `Text`. So the scanner strips line/block comments and the
//! argument region of every diagnostic macro (`info!`, `error!`, `assert_eq!`, …) and of `.expect(`,
//! then checks what remains. `format!` is intentionally NOT stripped — it is the one macro whose
//! output is routinely handed to `Text::new` (e.g. the connect-status and view-death prompts).

use std::path::PathBuf;

/// Characters beyond printable ASCII that the bundled Barlow Condensed weights are verified to
/// render (both SemiBold and Regular cmaps checked). A rendered literal may contain these; anything
/// else non-ASCII is flagged. Extending this list requires re-verifying the new codepoint against
/// the shipped `.ttf` cmaps first — the coverage claim is what keeps it from being tofu.
const TYPOGRAPHIC_SET: &[char] = &[
    '\u{2026}', // … HORIZONTAL ELLIPSIS
    '\u{2014}', // — EM DASH
    '\u{2013}', // – EN DASH
    '\u{00B0}', // ° DEGREE SIGN
    '\u{00D7}', // × MULTIPLICATION SIGN
    '\u{00B1}', // ± PLUS-MINUS SIGN
    '\u{2264}', // ≤ LESS-THAN OR EQUAL TO
];

/// Whether a character is inside the font's verified coverage for a given file. Printable ASCII is
/// always covered; the [`TYPOGRAPHIC_SET`] glyphs are covered ONLY when `allow_typographic` — i.e.
/// only for the Barlow-threaded files, not the ASCII-only-font sandboxes.
fn is_font_covered(c: char, allow_typographic: bool) -> bool {
    c.is_ascii() || (allow_typographic && TYPOGRAPHIC_SET.contains(&c))
}

/// The Barlow-threaded files that spawn `Text` through `ui_font::UiFonts` — printable ASCII PLUS the
/// verified [`TYPOGRAPHIC_SET`]. A rendered out-of-coverage glyph can only originate here or in
/// [`ASCII_ONLY_TEXT_FILES`].
const BARLOW_TEXT_FILES: &[&str] = &[
    "src/hud.rs",
    "src/crew_ui.rs",
    "src/sight.rs",
    "src/state.rs",
    "src/net/client.rs",
    "src/net/death_screen.rs",
    "src/net/debug_hud.rs",
    "src/net/hit_feel.rs",
];

/// The dev-sandbox files that spawn `Text`. These keep Bevy's default ASCII-only font (AGENTS.md), so
/// their rendered literals must be pure printable ASCII — the typographic set is NOT available to them.
const ASCII_ONLY_TEXT_FILES: &[&str] = &["src/sandbox.rs", "src/track_sandbox/mod.rs"];

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

/// Scan Rust source and return every string literal with an out-of-coverage character (see
/// [`is_font_covered`]) that is NOT inside a comment and NOT inside a diagnostic-macro / `.expect(`
/// argument region. A hand-rolled tokenizer, because that is exactly the boundary the rule draws
/// (rendered vs. diagnostic) and no lighter heuristic respects it: these files are full of em dashes
/// in `info!`/`assert!` that are perfectly legal.
fn scan(src: &str, allow_typographic: bool) -> Vec<Offender> {
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
            if !skipping && lit.chars().any(|c| !is_font_covered(c, allow_typographic)) {
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
            armed = dot_seen && ident == "expect";
            prev_ident = ident;
            dot_seen = false;
            continue;
        }

        // Punctuation.
        match c {
            '!' => {
                // `<ident>!` where ident is a diagnostic macro arms a macro-region.
                armed = DIAGNOSTIC_MACROS.contains(&prev_ident.as_str());
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
fn rendered_strings_within_font_coverage() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut failures = Vec::new();
    // Two regimes: the Barlow files may use the typographic set; the ASCII-only-font sandboxes may not.
    for (files, allow_typographic) in [(BARLOW_TEXT_FILES, true), (ASCII_ONLY_TEXT_FILES, false)] {
        for rel in files {
            let path = root.join(rel);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            for off in scan(&src, allow_typographic) {
                failures.push(format!(
                    "{rel}:{} contains an out-of-coverage character: {:?}",
                    off.line, off.text
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "rendered UI strings must stay within the bundled Barlow Condensed coverage — printable \
         ASCII plus the verified typographic set ({TYPOGRAPHIC_SET:?}); anything else has no glyph \
         and draws tofu. A new codepoint needs a fresh cmap check before it joins TYPOGRAPHIC_SET. \
         Offenders:\n{}",
        failures.join("\n")
    );
}

/// **Barlow regime** (`allow_typographic = true`). The scanner must flag an out-of-coverage rendered
/// literal but ignore comments, diagnostic strings, AND rendered literals that stay within the
/// verified typographic set — otherwise it would either miss real tofu, fight the house style, or
/// reject the em dashes/ellipses the Barlow files are now allowed to draw. Pin all three directions.
#[test]
fn scanner_flags_uncovered_but_not_comments_logs_or_typographic_set() {
    let sample = r#"
// a comment with an em dash — and an ellipsis … must be ignored
/* block comment — also ignored … */
fn demo() {
    info!("log line — never rendered, ignored …");
    error!("multi\nline — {}", x);
    assert_eq!(a, b, "assert — ignored …");
    let _ = foo.expect("expect — ignored …");
    let ok = format!("rendered {} ...", n);          // ascii, fine
    let typo = Text::new("RANGE 1200 m — ± 5°, ≤ 4×"); // OK: all within TYPOGRAPHIC_SET
    commands.spawn(Text::new("TOFU字"));              // BAD: kanji is out of coverage
    let label = "BROKEN €";                          // BAD: euro sign is out of coverage
}
"#;
    let offenders = scan(sample, true);
    let lines: Vec<usize> = offenders.iter().map(|o| o.line).collect();
    // Exactly the two out-of-coverage offenders — nothing from comments / logs / asserts / expect,
    // and NOT the typographic-set line (which is allowed in the Barlow regime).
    assert!(
        offenders.iter().any(|o| o.text.contains("TOFU")),
        "must flag the kanji-bearing Text::new literal; got {offenders:?}"
    );
    assert!(
        offenders.iter().any(|o| o.text.contains("BROKEN")),
        "must flag the euro-bearing rendered literal; got {offenders:?}"
    );
    assert!(
        !offenders.iter().any(|o| o.text.contains("RANGE")),
        "must NOT flag a literal that stays within the verified typographic set; got {offenders:?}"
    );
    assert_eq!(
        offenders.len(),
        2,
        "must flag ONLY out-of-coverage rendered literals, not comments/logs/asserts/expect/typographic-set; got {offenders:?} at lines {lines:?}"
    );
}

/// **ASCII-only regime** (`allow_typographic = false`, the sandbox files). Now the SAME typographic
/// literal that the Barlow regime allows must ALSO be flagged — the sandbox font has no glyph for it —
/// while comments, diagnostics, and pure-ASCII rendered literals are still ignored. This proves the
/// per-file split actually tightens the sandbox files rather than silently allowing the typographic set
/// everywhere.
#[test]
fn scanner_ascii_only_regime_flags_typographic_set_too() {
    let sample = r#"
// a comment with an em dash — and an ellipsis … must be ignored
fn demo() {
    info!("log line — never rendered, ignored …");
    let _ = foo.expect("expect — ignored …");
    let ok = format!("rendered {} ...", n);          // ascii, fine
    let typo = Text::new("RANGE 1200 m — ± 5°, ≤ 4×"); // BAD here: no typographic glyphs in the default font
    commands.spawn(Text::new("TOFU字"));              // BAD: kanji is out of coverage
    let label = "BROKEN €";                          // BAD: euro sign is out of coverage
}
"#;
    let offenders = scan(sample, false);
    let lines: Vec<usize> = offenders.iter().map(|o| o.line).collect();
    assert!(
        offenders.iter().any(|o| o.text.contains("RANGE")),
        "ASCII-only regime MUST flag the typographic-set literal; got {offenders:?}"
    );
    assert!(
        offenders.iter().any(|o| o.text.contains("TOFU")),
        "must still flag the kanji-bearing literal; got {offenders:?}"
    );
    assert!(
        offenders.iter().any(|o| o.text.contains("BROKEN")),
        "must still flag the euro-bearing literal; got {offenders:?}"
    );
    assert_eq!(
        offenders.len(),
        3,
        "ASCII-only regime flags the typographic + kanji + euro literals, but still not comments/logs/expect/pure-ASCII; got {offenders:?} at lines {lines:?}"
    );
}
