//! Guard: a comment in `src/` or `tests/` may not point at something that is no longer there.
//!
//! Two mechanical checks, both over COMMENTS ONLY (string literals are stripped before the comment
//! scan — a `file:line` inside an `assert!` message is code, not prose, and there is precedent for
//! leaving those alone):
//!
//! 1. every backtick-quoted Rust path IN SCOPE must resolve to something the repository still
//!    spells somewhere;
//! 2. no comment may cite a repository file BY LINE NUMBER.
//!
//! Neither is a style preference. Comment rot is not fixed by asking people to update comments:
//! the measured rate at which a code change drags its comment along is 13–20 %, RENAME is among the
//! least likely changes to do so, and no process mandate has ever moved that number. Only mechanical
//! detection has. A hand sweep on 2026-07-28 (`35f9ff3`) resolved every backticked identifier in
//! every comment in `src/` and `tests/` and found zero dangling, then re-pinned the drifted
//! `file:line` citations by symbol. This file is that sweep made permanent; it rides `cargo test`,
//! so pre-push and CI pick it up with no hook change.
//!
//! # Check 1: what is IN SCOPE, and why it is not "everything"
//!
//! The corpus names external symbols in prose constantly — `bevy`, `avian3d`, `lightyear`,
//! `bevy_replicon`, wgpu/winit/Win32/Cocoa APIs, and the internals of all of them. Those will never
//! appear in our sources and can never be resolved here. **A gate that cries wolf gets disabled**,
//! so the scope is drawn where the false-positive rate is zero by construction rather than by
//! apology. Three candidate rules were measured against the tree on 2026-07-29:
//!
//! | rule | segment checks | exception rows needed |
//! |---|---|---|
//! | every backticked path, resolved against this repository | 8 410 | **159** |
//! | …plus the sources of all 819 crates in `Cargo.lock` | 8 410 | 38 |
//! | [what this file does](#the-rule) | 1 353 | **12** |
//!
//! The first is unusable: 159 rows is not an exception list, it is a second copy of the corpus.
//! The second looks better and is worse. It needs a 534 MB scan of `$CARGO_HOME/registry/src` on
//! every test run, its answer changes with the host platform (a Linux-only crate is not unpacked on
//! macOS), it collapses under `cargo vendor`, and — decisively — 546 of this crate's own 3 498
//! definition names ALSO occur somewhere in the dependency graph. Under that rule 16 % of our own
//! symbols would still resolve after being deleted. It buys a shorter list by blinding the gate to
//! one name in six, which is the opposite of the point.
//!
//! ## The rule
//!
//! A backticked span is checked when it parses as a Rust path (`ident`, `a::b::C`, optional
//! trailing `()`) AND it is one of:
//!
//! - **SCREAMING_SNAKE_CASE** — a constant. This is the house's tuning-knob idiom, it churns
//!   hardest (`track/transmission.rs`'s module doc carries a whole ledger of constants that were
//!   removed or reclassified), and the shape is rare enough in other people's prose that only six
//!   foreign names land in it. A single all-caps WORD is deliberately excluded: `HIDDEN`, `VALUE`,
//!   `READY`, `SWAP` are as likely to be prose emphasis as an item.
//! - **qualified by one of our own module names** — `tank::apply_tank_spec`,
//!   `net::client::focus_menu`. The first segment settles ownership with no heuristic at all: the
//!   comment is claiming the path is ours, so it is fair to hold it to that. `AlphaMode::Multiply`
//!   and `bevy_render::view::window::create_surfaces` make no such claim and are not checked.
//!   Every segment of a qualified path is resolved, not just the root, so a surviving type with a
//!   deleted field (`Section::inset`, the sweep's own example) is still caught.
//!
//! Everything else — bare `UpperCamelCase` types and bare `snake_case` functions — is OUT of scope,
//! and that is the cost of this design, paid knowingly. There is no syntax that separates our
//! `TankCommand` from bevy's `Transform`, so covering them means either 159 exceptions or a blind
//! spot in one name out of six. The bare-name case is not unguarded, merely guarded elsewhere:
//! rustdoc resolves `[\`Foo\`]` intra-doc links exactly, and the house style uses them heavily.
//!
//! ## What "resolves" means
//!
//! The universe is THIS REPOSITORY: every identifier-shaped token in [`UNIVERSE_DIRS`] /
//! [`UNIVERSE_FILES`], including string-literal bodies (env var names, cargo features, glTF node
//! names and asset stems are all named as strings, never as Rust tokens) but EXCLUDING the comments
//! in our own `.rs` files. The exclusions matter in both directions:
//!
//! - comments are excluded so a comment cannot vouch for itself, or for its neighbour;
//! - `.agents/`, `upstream/` and the root `*.md` files are excluded because they are the design
//!   record. ADRs and research logs name deleted symbols ON PURPOSE — that is what a record of a
//!   deletion is — and letting them into the universe would launder exactly the names this exists
//!   to catch.
//!
//! Vendored crates under `vendor/` ARE in the universe: they are our tracked source, and a comment
//! about the patch we carry is a comment about code in this tree.
//!
//! [`SELF`] — this file — is excluded from both the universe and the scan, and that exclusion is
//! load-bearing rather than convenient. Its allowlist rows QUOTE the names they exempt, so leaving
//! it in the universe would let the exemption list vouch for every name on it and every row would
//! read as stale; and its own prose necessarily names the same symbols. A ledger of exceptions is
//! not evidence that the exceptions exist.
//!
//! # Check 2: no `file:line` citations into this repository
//!
//! A line number has roughly a one-refactor half-life. `35f9ff3` found four drifted citations in
//! `shadow_proxy.rs` alone — all into the vendored bevy that the same branch had shifted two commits
//! earlier — and a design-doc section where six of eight pointers were past EOF of a file that had
//! become a 63-line facade. Cite the symbol; it survives the refactor and it says what you meant.
//!
//! Only citations that are REPOSITORY-RELATIVE PATHS are flagged (they contain a `/` and name a file
//! that exists under the repo root). A bare basename is not resolved: `server.rs:707`,
//! `registry.rs:175` and `plugin.rs:355` in this tree all mean lightyear's files, not ours, and
//! matching them by basename against `src/net/server.rs` would be a guess. Citations into
//! third-party crates outside the tree (`bevy_render-0.19.0/src/view/mod.rs:1035`) are out of scope:
//! nothing here can re-pin them and they move only on a deliberate dependency bump.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Identifiers that are in scope but cannot resolve, each with the reason it is not a defect.
///
/// This list is meant to stay SHORT and to shrink. Two kinds of row are legitimate: a name owned by
/// somebody else's codebase, and a name this corpus deliberately mentions AFTER deleting it (the
/// "what this replaces" ledgers — narrating a trap that comes back if the reader undoes the change).
/// A third kind, marked OPEN, is a found defect that has not been fixed yet; those rows are debts,
/// not exceptions, and each one names what should replace it.
///
/// Keyed by (file, identifier): an exception is granted where it was argued for, not corpus-wide.
const ALLOWED_IDENTIFIERS: &[(&str, &str, &str)] = &[
    // ---- Owned by another codebase. -------------------------------------------------------
    (
        "src/net/contact_probe.rs",
        "AABB_MARGIN",
        "avian's own broad-phase AABB margin (5 cm). Named to size OUR margin against it.",
    ),
    (
        "src/settings/store.rs",
        "ERROR_ACCESS_DENIED",
        "Win32 error code 5. Named as documentation for the numeric literal we actually match.",
    ),
    (
        "src/settings/store.rs",
        "ERROR_SHARING_VIOLATION",
        "Win32 error code 32. Same.",
    ),
    (
        "src/settings/store.rs",
        "ERROR_LOCK_VIOLATION",
        "Win32 error code 33. Same.",
    ),
    (
        "src/settings/store.rs",
        "ACCESS_DENIED",
        "The Win32 failure a 64-bit game under `Program Files` gets. Prose about the platform.",
    ),
    (
        "src/settings/store.rs",
        "F_FULLFSYNC",
        "macOS `fcntl` command, named as the durability step we deliberately do NOT take.",
    ),
    // ---- Deliberately named after deletion. -----------------------------------------------
    (
        "src/track/transmission.rs",
        "DRAG_SAT_SPEED",
        "The module doc's constant ledger records it as REMOVED (stage B) and says what replaced \
         it. Deleting the row would delete the record of the removal.",
    ),
    (
        "src/track/transmission.rs",
        "STEER_SERVO_BAND",
        "Same ledger: REMOVED because the steering servo became an exact semi-implicit law, so no \
         proportional band exists to tune. The entry also records that its droop was a bug.",
    ),
];

/// Line-number citations that stay, each with the reason it cannot be pinned by symbol.
///
/// Keyed by (citing file, cited path) — NOT by the line number, so re-pinning after a vendor bump
/// does not churn this list.
const ALLOWED_CITATIONS: &[(&str, &str, &str)] = &[
    (
        "src/track/shadow_proxy.rs",
        "vendor/bevy_pbr-0.19.0-scalar-math/src/render/light.rs",
        "The alpha-mode/`MAY_DISCARD` match arm, cited alongside its symbol. Vendored third-party \
         code moves only when we re-vendor, and `35f9ff3` re-pinned this one by line on purpose.",
    ),
    (
        "src/track/shadow_proxy.rs",
        "vendor/bevy_pbr-0.19.0-scalar-math/src/render/pbr_prepass_functions.wgsl",
        "A WGSL fragment-shader body. There is no item path to cite in a shader — the line IS the \
         only handle.",
    ),
];

/// Directories whose files define what a comment is allowed to name. See the module doc for why
/// `.agents/`, `upstream/` and the root `*.md` files are absent.
const UNIVERSE_DIRS: &[&str] = &[
    "src", "tests", "vendor", "assets", "scripts", "build", ".github", ".cargo",
];

/// Root files that carry names nothing else does — cargo features, dependency crate names.
const UNIVERSE_FILES: &[&str] = &["Cargo.toml", "Cargo.lock"];

/// Text extensions read into the universe. Everything else under [`UNIVERSE_DIRS`] (textures,
/// meshes, fonts) is skipped.
const UNIVERSE_EXTS: &[&str] = &[
    "rs", "wgsl", "ron", "toml", "txt", "sh", "py", "yml", "yaml", "lock", "json",
];

/// This file, which neither contributes to the universe nor is scanned — see the module doc.
const SELF: &str = "tests/doc_citations.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// One comment, with the line its first character sits on.
struct Comment {
    line: usize,
    text: String,
}

/// Split Rust source into its comments and its non-comment text.
///
/// The non-comment half keeps STRING AND CHAR LITERAL BODIES: env var names, cargo feature names,
/// glTF node names and asset stems exist in this codebase only as strings, and a comment naming one
/// of them is naming something real. Newlines are preserved so comment line numbers stay exact.
fn partition(src: &str) -> (Vec<Comment>, String) {
    let chars: Vec<char> = src.chars().collect();
    let mut comments = Vec::new();
    let mut code = String::with_capacity(src.len());
    let mut line = 1usize;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            code.push('\n');
            i += 1;
            continue;
        }
        // Line comment.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            comments.push(Comment {
                line,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }
        // Block comment (nesting, as Rust allows).
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let start = i;
            let start_line = line;
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            comments.push(Comment {
                line: start_line,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }
        // Raw string: r"..." / r#"..."#
        if c == 'r' && matches!(chars.get(i + 1), Some('"') | Some('#')) {
            let mut j = i + 1;
            let mut hashes = 0;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) == Some(&'"') {
                let close: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                i = j + 1;
                while i < chars.len() && chars[i..].iter().take(close.len()).ne(close.iter()) {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    code.push(chars[i]);
                    i += 1;
                }
                i = (i + close.len()).min(chars.len());
                code.push(' ');
                continue;
            }
        }
        // Ordinary string: keep the body, drop the escapes.
        if c == '"' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    code.push(' ');
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                }
                code.push(chars[i]);
                i += 1;
            }
            code.push(' ');
            continue;
        }
        // Char literal — only the two shapes that can hide a quote.
        if c == '\'' {
            let escaped = chars.get(i + 1) == Some(&'\\');
            let end = if escaped { i + 3 } else { i + 2 };
            if chars.get(end) == Some(&'\'') {
                code.push(' ');
                i = end + 1;
                continue;
            }
        }
        code.push(c);
        i += 1;
    }
    (comments, code)
}

/// Every maximal identifier-shaped run in `text`.
fn identifiers(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(&text[start..i]);
        } else {
            i += 1;
        }
    }
    out
}

/// `SCREAMING_SNAKE_CASE` with at least one underscore. A single all-caps word is excluded — see
/// the module doc.
fn is_screaming_snake(s: &str) -> bool {
    s.len() >= 2
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.contains('_')
        && !s.ends_with('_')
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// Split a backticked span into path segments, or `None` if it is not a Rust path. A trailing `()`
/// is dropped: `SurfaceCapabilities::default()` names the same item as without it.
fn path_segments(span: &str) -> Option<Vec<&str>> {
    let span = span.strip_suffix("()").unwrap_or(span);
    if span.is_empty() {
        return None;
    }
    let segments: Vec<&str> = span.split("::").collect();
    for seg in &segments {
        let mut bytes = seg.bytes();
        match bytes.next() {
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
            _ => return None,
        }
        if !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
    }
    Some(segments)
}

/// Backtick-delimited spans on a single line of `text`. Unterminated backticks are ignored.
fn backticked(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        while let Some(open) = rest.find('`') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('`') else { break };
            out.push(&after[..close]);
            rest = &after[close + 1..];
        }
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every `.rs` file under `src/` and `tests/`, sorted.
fn scanned_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files);
    walk(&root.join("tests"), &mut files);
    files.retain(|p| p.extension().is_some_and(|e| e == "rs") && rel(root, p) != SELF);
    files.sort();
    files
}

/// The names a comment is allowed to use, plus the module names that mark a path as ours.
fn universe(root: &Path) -> (HashSet<String>, HashSet<String>) {
    let mut names = HashSet::new();
    let mut modules = HashSet::new();
    let mut files = Vec::new();
    for dir in UNIVERSE_DIRS {
        walk(&root.join(dir), &mut files);
    }
    for file in UNIVERSE_FILES {
        files.push(root.join(file));
    }
    for path in &files {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !UNIVERSE_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let relative = rel(root, path);
        if relative == SELF {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        if ext == "rs" {
            let (_, code) = partition(&src);
            let tokens = identifiers(&code);
            if relative.starts_with("src/") {
                // `mod foo;` / `mod foo {` declarations, plus the file's own stem: both are names a
                // comment can legitimately use as a path root.
                for pair in tokens.windows(2) {
                    if pair[0] == "mod" {
                        modules.insert(pair[1].to_owned());
                    }
                }
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                if stem != "mod" {
                    modules.insert(stem.into_owned());
                }
            }
            names.extend(tokens.into_iter().map(str::to_owned));
        } else {
            names.extend(identifiers(&src).into_iter().map(str::to_owned));
        }
    }
    (names, modules)
}

#[test]
fn no_comment_names_an_identifier_that_is_gone() {
    let root = repo_root();
    let (names, modules) = universe(&root);
    assert!(
        names.len() > 10_000 && modules.len() > 50,
        "universe collapsed ({} names, {} modules) — the walk broke, and a collapsed universe \
         would fail every comment in the tree",
        names.len(),
        modules.len()
    );

    let files = scanned_files(&root);
    assert!(
        files.len() > 50,
        "found only {} files — walk broke",
        files.len()
    );

    let mut offenders = Vec::new();
    let mut used = Vec::new();
    let mut checked = 0usize;

    for path in &files {
        let relative = rel(&root, path);
        let src = std::fs::read_to_string(path).expect("readable");
        let (comments, _) = partition(&src);
        for comment in &comments {
            for span in backticked(&comment.text) {
                let Some(segments) = path_segments(span) else {
                    continue;
                };
                let ours = segments.len() > 1 && modules.contains(segments[0]);
                for segment in &segments {
                    if !ours && !is_screaming_snake(segment) {
                        continue;
                    }
                    checked += 1;
                    if names.contains(*segment) {
                        continue;
                    }
                    match ALLOWED_IDENTIFIERS
                        .iter()
                        .find(|(f, n, _)| *f == relative && n == segment)
                    {
                        Some(row) => used.push(*row),
                        None => offenders.push(format!(
                            "  {relative}:{}  `{span}`  — `{segment}` is spelled nowhere in this \
                             repository",
                            comment.line
                        )),
                    }
                }
            }
        }
    }

    assert!(
        checked > 500,
        "only {checked} identifiers reached the check — the scope predicate broke"
    );
    assert!(
        offenders.is_empty(),
        "comments name identifiers that no longer exist:\n{}\n\n\
         Fix the comment: name what the code calls it now, or drop the sentence if the thing it \
         described is gone. If the mention is deliberate — a constant the module doc records as \
         REMOVED, or a symbol owned by a dependency or by the OS — add a row to \
         `ALLOWED_IDENTIFIERS` in this file WITH THE REASON.",
        offenders.join("\n")
    );

    let stale: Vec<_> = ALLOWED_IDENTIFIERS
        .iter()
        .filter(|row| !used.contains(row))
        .map(|(f, n, _)| format!("  {f}  `{n}`"))
        .collect();
    assert!(
        stale.is_empty(),
        "these `ALLOWED_IDENTIFIERS` rows no longer describe anything — delete them:\n{}",
        stale.join("\n")
    );
}

#[test]
fn no_comment_cites_a_file_in_this_repository_by_line_number() {
    let root = repo_root();
    let files = scanned_files(&root);
    assert!(
        files.len() > 50,
        "found only {} files — walk broke",
        files.len()
    );

    let mut offenders = Vec::new();
    let mut used = Vec::new();

    for path in &files {
        let relative = rel(&root, path);
        let src = std::fs::read_to_string(path).expect("readable");
        let (comments, _) = partition(&src);
        for comment in &comments {
            for (cited, line_no) in citations(&comment.text, &root) {
                match ALLOWED_CITATIONS
                    .iter()
                    .find(|(f, p, _)| *f == relative && *p == cited)
                {
                    Some(row) => used.push(*row),
                    None => offenders.push(format!(
                        "  {relative}:{}  cites {cited}:{line_no}",
                        comment.line
                    )),
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "comments cite repository files by line number:\n{}\n\n\
         Cite the SYMBOL instead — the item name survives the refactor that moves the line, and it \
         says what you were pointing at. Line citations in this tree have already rotted twice \
         (see `35f9ff3`). If there is genuinely no symbol to name — a shader body, a vendored \
         match arm — add a row to `ALLOWED_CITATIONS` in this file WITH THE REASON.",
        offenders.join("\n")
    );

    let stale: Vec<_> = ALLOWED_CITATIONS
        .iter()
        .filter(|row| !used.contains(row))
        .map(|(f, p, _)| format!("  {f}  cites {p}"))
        .collect();
    assert!(
        stale.is_empty(),
        "these `ALLOWED_CITATIONS` rows cite nothing any more — delete them:\n{}",
        stale.join("\n")
    );
}

/// Every `<repo-relative path>:<line>` in `text` that names a file under `root`.
///
/// The path must contain a `/`: a bare basename is not resolved against the tree, because in this
/// corpus every bare `foo.rs:123` means a dependency's file, not ours.
fn citations(text: &str, root: &Path) -> Vec<(String, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A path character run, then `:`, then digits.
        let is_path = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-');
        if !is_path(bytes[i]) || (i > 0 && is_path(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_path(bytes[i]) {
            i += 1;
        }
        let path = &text[start..i];
        if i >= bytes.len() || bytes[i] != b':' {
            continue;
        }
        let digits_start = i + 1;
        let mut j = digits_start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == digits_start {
            continue;
        }
        i = j;
        if !path.contains('/') {
            continue;
        }
        if root.join(path).is_file() {
            out.push((path.to_owned(), text[digits_start..j].parse().unwrap_or(0)));
        }
    }
    out
}

/// The scanner itself, on the shapes that have historically fooled hand-rolled versions.
#[test]
fn the_scanner_reads_comments_and_not_code() {
    let src = r####"
//! A doc comment naming `MAX_THING` and `net::client::run`.
fn f() {
    let s = "SPIKE_TRACE";            // a string body is a NAME, not prose
    let msg = "see src/lib.rs:42";    // a citation in a STRING is code, not a comment
    /* block `BLOCK_CONST` */
    let c = '`';
}
"####;
    let (comments, code) = partition(src);

    let spans: Vec<&str> = comments.iter().flat_map(|c| backticked(&c.text)).collect();
    assert!(
        spans.contains(&"MAX_THING") && spans.contains(&"net::client::run"),
        "doc-comment spans missed: {spans:?}"
    );
    assert!(
        spans.contains(&"BLOCK_CONST"),
        "block-comment spans missed: {spans:?}"
    );

    let names = identifiers(&code);
    assert!(
        names.contains(&"SPIKE_TRACE"),
        "string bodies must reach the universe — env vars and glTF node names live only there"
    );
    assert!(
        !names.contains(&"MAX_THING"),
        "a comment must not vouch for its own identifiers"
    );

    // Line numbers survive the multi-line block comment above it.
    let block = comments
        .iter()
        .find(|c| c.text.starts_with("/*"))
        .expect("block comment found");
    assert_eq!(block.line, 6, "block comment line drifted");

    // `src/lib.rs:42` sits in a string literal, so no comment carries it.
    let root = repo_root();
    assert!(
        comments
            .iter()
            .all(|c| citations(&c.text, &root).is_empty()),
        "a citation inside a string literal is not a comment citation"
    );
    // …but the same text inside a comment is caught, and only for a path that exists.
    assert_eq!(
        citations("// see src/lib.rs:42 and no/such/file.rs:9", &root),
        vec![("src/lib.rs".to_owned(), 42)],
        "repo-relative citations are resolved against the tree"
    );
    assert!(
        citations("// lightyear server.rs:707 at 127.0.0.1:5888", &root).is_empty(),
        "bare basenames and host:port pairs are not repository citations"
    );

    assert!(is_screaming_snake("MAX_FN_LINES") && !is_screaming_snake("HIDDEN"));
    assert_eq!(
        path_segments("SurfaceCapabilities::default()"),
        Some(vec!["SurfaceCapabilities", "default"])
    );
    assert_eq!(path_segments("(z, y)"), None);
    assert_eq!(path_segments("video.ron"), None);
}
