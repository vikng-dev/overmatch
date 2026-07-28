//! Guard: no function body in `src/` grows past [`MAX_FN_LINES`] without an explicit, justified
//! exception.
//!
//! # Why a FUNCTION gate and not a file gate
//!
//! Deliberately not a file-size rule. House doctrine is DEEP MODULES
//! (`.agents/skills/codebase-design/SKILL.md`): a large implementation behind a small interface is
//! the goal, not the smell. `src/track/transmission.rs` is 3 000 lines and 33 public items and is
//! CORRECT as one file — splitting it would scatter the drivetrain and manufacture exactly the
//! bouncing-between-small-modules friction `improve-codebase-architecture` says to hunt for. A
//! survey of vendor guidance (2026-07-28) found NO model vendor publishes a source-file line limit;
//! every published number — 200-line CLAUDE.md, 500-line SKILL.md, 32 KiB AGENTS.md — governs files
//! that are ALWAYS in context. Source files are read on demand. The codebase should be navigable,
//! not small.
//!
//! What the same survey did find is that the real defect was function-scale: two functions had
//! reached 1 122 and 1 080 lines at nesting depth 6 and 8. Both were invisible to a file rule
//! (their files were ~50 % tests) and both are caught here at ~300.
//!
//! # What counts
//!
//! Physical lines from the body's opening brace to its closing brace, comments included. Comments
//! count on purpose: a 1 000-line body narrated by 75 prose paragraphs is a design document with
//! executable statements interleaved, and that is precisely the shape this exists to catch.
//!
//! # Tests are exempt
//!
//! A long test is a linear scenario script, which is a different thing from a long code path — it
//! has no branches to get lost in and no state to thread. Test-only modules are listed in
//! [`TEST_ONLY_FILES`] (the ones gated by a `#[cfg(test)]` at their DECLARATION site, which is not
//! visible in the file itself), and everything after an in-file `#[cfg(test)]` is skipped.
//! The explicit-manifest shape follows `tests/ui_ascii.rs`, which does the same for font coverage.

use std::path::{Path, PathBuf};

/// The ceiling, in physical lines of function body.
///
/// 300 rather than a rounder 250: at 250 this gate would have been born with nine exceptions, and a
/// gate that ships with a long allowlist teaches everyone that the allowlist is normal. 300 catches
/// every function that has actually gone wrong here while admitting the composition roots and the
/// two phases that were deliberately left whole after review.
const MAX_FN_LINES: usize = 300;

/// Whole files that exist only under `#[cfg(test)]` — gated at their DECLARATION site (`mod` line in
/// the parent), so nothing inside the file itself marks them as tests.
const TEST_ONLY_FILES: &[&str] = &[
    "src/headless_test.rs",
    "src/track/transmission/tests.rs",
    "src/net/shot_loss.rs",
    "src/net/grip_battery.rs",
];

/// Functions allowed past [`MAX_FN_LINES`], each with the reason it is not a defect.
///
/// This list is meant to stay SHORT and to shrink. Adding a row is a real decision: it says the
/// function's length is inherent to what it does, not accumulated. "I did not want to refactor it"
/// is not a reason. Every row below was reviewed on 2026-07-28.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "src/net/client.rs",
        "run",
        "Composition root. A table of plugin registrations is supposed to be long and flat — it \
         has no branches and no state, and breaking it up would hide the wiring it exists to show.",
    ),
    (
        "src/track/transmission.rs",
        "run_shift_decision",
        "Reviewed and deliberately left whole when `regenerative` was split 1 122 -> 277. Its arms \
         share six mutable locals plus the `dwell_blocks` closure, and the PRIORITY ORDERING \
         between them is the logic; helpers would thread all six back and forth.",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Blank out comments, string literals and char literals so their braces and `fn` spellings cannot
/// be mistaken for code. Newlines are preserved so line numbers stay exact.
fn blank_noncode(src: &str) -> String {
    let bytes: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Line comment.
        if c == '/' && bytes.get(i + 1) == Some(&'/') {
            while i < bytes.len() && bytes[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        // Block comment (nesting, as Rust allows).
        if c == '/' && bytes.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            out.push_str("  ");
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == '/' && bytes.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if bytes[i] == '*' && bytes.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                } else {
                    out.push(if bytes[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            continue;
        }
        // Raw string: r"..." / r#"..."#
        if c == 'r' && matches!(bytes.get(i + 1), Some('"') | Some('#')) {
            let mut j = i + 1;
            let mut hashes = 0;
            while bytes.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if bytes.get(j) == Some(&'"') {
                for _ in i..=j {
                    out.push(' ');
                }
                i = j + 1;
                let close: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                while i < bytes.len() && bytes[i..].iter().take(close.len()).ne(close.iter()) {
                    out.push(if bytes[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
                for _ in 0..close.len().min(bytes.len().saturating_sub(i)) {
                    out.push(' ');
                    i += 1;
                }
                continue;
            }
        }
        // Ordinary string.
        if c == '"' {
            out.push(' ');
            i += 1;
            while i < bytes.len() {
                if bytes[i] == '\\' {
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if bytes[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(if bytes[i] == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            continue;
        }
        // Char literal — only the two shapes that can hide a brace.
        if c == '\'' {
            let escaped = bytes.get(i + 1) == Some(&'\\');
            let end = if escaped { i + 3 } else { i + 2 };
            if bytes.get(end) == Some(&'\'') {
                for _ in i..=end {
                    out.push(' ');
                }
                i = end + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// One measured function body.
struct FnBody {
    name: String,
    line: usize,
    lines: usize,
}

/// Find every `fn` with a body and measure it. `src` must already be [`blank_noncode`]'d.
fn measure_fns(src: &str, stop_at: usize) -> Vec<FnBody> {
    let chars: Vec<char> = src.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + 2 < chars.len() && i < stop_at {
        // A `fn` token: preceded by non-identifier, followed by whitespace then a name.
        let is_boundary = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        if !(is_boundary && chars[i] == 'f' && chars[i + 1] == 'n' && chars[i + 2].is_whitespace())
        {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        let name_start = j;
        while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
            j += 1;
        }
        if j == name_start {
            i += 1;
            continue;
        }
        let name: String = chars[name_start..j].iter().collect();
        // Walk to the body's opening brace, tracking parameter/generic/index nesting. A `;` at
        // depth zero means a bodiless declaration (trait method, `fn` type) — skip it.
        let mut depth: i32 = 0;
        let mut body_start = None;
        while j < chars.len() {
            match chars[j] {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                '<' => depth += 1,
                '>' if j > 0 && chars[j - 1] != '-' && chars[j - 1] != '=' => depth -= 1,
                ';' if depth <= 0 => break,
                '{' if depth <= 0 => {
                    body_start = Some(j);
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        let Some(start) = body_start else {
            i += 1;
            continue;
        };
        let mut brace = 0;
        let mut k = start;
        while k < chars.len() {
            if chars[k] == '{' {
                brace += 1;
            } else if chars[k] == '}' {
                brace -= 1;
                if brace == 0 {
                    break;
                }
            }
            k += 1;
        }
        let line_of = |idx: usize| chars[..idx].iter().filter(|c| **c == '\n').count() + 1;
        found.push(FnBody {
            name,
            line: line_of(i),
            lines: line_of(k) - line_of(start) + 1,
        });
        i = k.max(i + 1);
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_function_exceeds_the_ceiling_without_a_stated_reason() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    assert!(
        files.len() > 50,
        "found only {} files — walk broke",
        files.len()
    );

    let mut offenders = Vec::new();
    let mut used_rows = Vec::new();

    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        if TEST_ONLY_FILES.contains(&rel.as_str()) {
            continue;
        }
        let src = std::fs::read_to_string(path).expect("readable");
        let clean = blank_noncode(&src);
        // Everything from the first in-file `#[cfg(test)]` onward is test scaffolding.
        let stop_at = clean.find("#[cfg(test)]").unwrap_or(clean.len());
        for body in measure_fns(&clean, stop_at) {
            if body.lines <= MAX_FN_LINES {
                continue;
            }
            match ALLOWED
                .iter()
                .find(|(f, n, _)| *f == rel && *n == body.name)
            {
                Some(row) => used_rows.push(*row),
                None => offenders.push(format!(
                    "  {rel}:{}  fn {}  — {} lines (ceiling {MAX_FN_LINES})",
                    body.line, body.name, body.lines
                )),
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "function bodies past the {MAX_FN_LINES}-line ceiling:\n{}\n\n\
         Split it into private helpers named for the one thing each does — the module's public \
         surface need not change (see `.agents/skills/codebase-design/SKILL.md`: depth is a \
         property of the interface, not the implementation).\n\
         If the length is genuinely inherent — a composition root, or one phase whose parts share \
         too much mutable state to separate — add a row to `ALLOWED` in this file WITH THE REASON. \
         \"I did not want to refactor it\" is not a reason.",
        offenders.join("\n")
    );

    // A stale exception is worse than no exception: it silently permits a regression in a function
    // someone already fixed. Every row must still be earning its place.
    let stale: Vec<_> = ALLOWED
        .iter()
        .filter(|row| !used_rows.contains(row))
        .map(|(f, n, _)| format!("  {f}  fn {n}"))
        .collect();
    assert!(
        stale.is_empty(),
        "these `ALLOWED` rows no longer exceed the ceiling — delete them:\n{}",
        stale.join("\n")
    );
}

/// The measurement itself, on shapes that have historically fooled hand-rolled scanners.
#[test]
fn the_scanner_measures_bodies_not_braces_in_prose() {
    let src = r####"
fn one() {
    let s = "a { brace in a string";
    // a { brace in a comment
    /* and { one here */
    let c = '{';
}
trait T { fn declared_only(&self); }
fn generic<T: Into<u8>>(x: T) -> u8 { x.into() }
fn raw() { let r = r#"a { raw brace"#; }
"####;
    let clean = blank_noncode(src);
    let fns = measure_fns(&clean, clean.len());
    let by = |name: &str| {
        fns.iter().find(|f| f.name == name).unwrap_or_else(|| {
            panic!(
                "missing {name} in {:?}",
                fns.iter().map(|f| &f.name).collect::<Vec<_>>()
            )
        })
    };
    // Body spans the `{` on the `fn one()` line through its closing `}` six lines later.
    assert_eq!(
        by("one").lines,
        6,
        "braces in prose must not close the body"
    );
    assert_eq!(by("generic").lines, 1, "generic bounds are not a body");
    assert_eq!(by("raw").lines, 1, "raw strings must not close the body");
    assert!(
        !fns.iter().any(|f| f.name == "declared_only"),
        "a bodiless trait method is not a function body"
    );
}
