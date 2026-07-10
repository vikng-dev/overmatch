//! Guard: nothing outside the netcode layer may name the netcode layer.
//!
//! `net` is no longer a cargo feature. The PVP client IS the product, so lightyear compiles into
//! every build, and single-player returns as a *runtime* mode inside that client rather than as a
//! compile variant. The retired `--no-default-features` gate used to prove — by refusing to compile
//! — that nothing outside `net` reached for a lightyear type. That proof was real but expensive: it
//! cost a second full resolution of the dependency graph on every gate run and in every build cache,
//! and it only ever proved the `#[cfg]` gates were placed correctly, which is circular once the gates
//! are gone.
//!
//! So the guarantee moves here, and it splits in two:
//!
//! - the **dynamic** half — the sim must still *run* with no netcode mounted — is proved by
//!   `src/headless_test.rs`, which boots `SimPlugin` alone, no lightyear plugins, and drives a tank.
//!   That is strictly stronger than "compiles without lightyear in scope".
//! - the **static** half is this test: no module outside `net` names a lightyear type or a
//!   `crate::net` item.
//!
//! Deny-by-default. Every `.rs` file under `src/` is scanned unless it is listed in
//! [`NET_AWARE_FILES`], so a new module is guarded the moment it is created. Reaching for netcode
//! from one is then a deliberate act: add the file below, with the reason it earns the coupling.
//!
//! Only *code* is scanned — comments are stripped first. The house style discusses lightyear's
//! behaviour freely in prose (`tank.rs`, `camera.rs`, `aim.rs`, `driving.rs` and `command.rs` all
//! name it in doc comments), and that prose is documentation, not a dependency.

use std::path::{Path, PathBuf};

/// The files permitted to name the netcode layer in code, each with the reason it earns the
/// coupling. Everything else under `src/` is scanned and must stay clean. `src/net/` is matched as a
/// prefix: it *is* the layer.
const NET_AWARE_FILES: &[&str] = &[
    // The netcode layer itself.
    "src/net/",
    // The composition root: declares `pub mod net` and mounts `NetClientPlugin`.
    "src/lib.rs",
    // The two net bins: thin shells over `net::client::run()` / `net::server::run()`.
    "src/main.rs",
    "src/bin/overmatch-server.rs",
    // The divergence instrument reads rollback/prediction state by design — `LocalTimeline`,
    // `Rollback`, `ConfirmedHistory`, `VisualCorrection`, `net::render_error::RenderErrorOffset`.
    // A passive observer of the netcode, never a sim dependency: it writes no sim state.
    "src/trace.rs",
    // The world-anchored nameplate prefixes `[BOT]` from the replicated `net::protocol::NetBot`
    // marker. View layer, not sim.
    "src/hud.rs",
];

/// The tokens that constitute naming the netcode layer from outside it: the crate itself, and any
/// path into our own `net` module.
const NETCODE_TOKENS: &[&str] = &["lightyear", "crate::net"];

/// Whether `rel` (a `/`-separated repo-relative path) is allowed to name netcode. `src/net/` matches
/// as a directory prefix; every other entry is an exact file.
fn is_net_aware(rel: &str) -> bool {
    NET_AWARE_FILES
        .iter()
        .any(|allowed| match allowed.strip_suffix('/') {
            Some(dir) => rel.starts_with(dir) && rel.as_bytes().get(dir.len()) == Some(&b'/'),
            None => rel == *allowed,
        })
}

/// Blank out `//`-line and `/* */`-block comments, preserving everything else byte-for-byte (so line
/// numbers in a failure message still point at the source). String literals are left intact: a
/// netcode token inside one would be just as much of a dependency as one in an expression, and no
/// false positive is possible — the tokens are Rust paths, not prose the UI ever renders.
///
/// Rust's block comments nest, so track depth rather than scanning for the first `*/`.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut block_depth = 0usize;

    while i < bytes.len() {
        let rest = &src[i..];
        if block_depth > 0 {
            if rest.starts_with("/*") {
                block_depth += 1;
                out.push_str("  ");
                i += 2;
            } else if rest.starts_with("*/") {
                block_depth -= 1;
                out.push_str("  ");
                i += 2;
            } else {
                let c = src[i..].chars().next().expect("i is a char boundary");
                // Keep newlines so line numbers survive; blank the rest of the comment.
                out.push(if c == '\n' { '\n' } else { ' ' });
                i += c.len_utf8();
            }
        } else if rest.starts_with("/*") {
            block_depth = 1;
            out.push_str("  ");
            i += 2;
        } else if rest.starts_with("//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
        } else {
            let c = src[i..].chars().next().expect("i is a char boundary");
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// Every `.rs` file under `dir`, recursively.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The repo-relative, `/`-separated path of `path`.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("src file is under the manifest dir")
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[test]
fn sim_layer_does_not_name_the_netcode_layer() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rs_files(&root.join("src"), &mut files);
    files.sort();

    let mut violations = Vec::new();
    for path in files {
        let rel = relative(&root, &path);
        if is_net_aware(&rel) {
            continue;
        }
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        for (n, line) in strip_comments(&src).lines().enumerate() {
            for token in NETCODE_TOKENS {
                if line.contains(token) {
                    violations.push(format!("{rel}:{}: {token}  ⟶  {}", n + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "these files name the netcode layer but are not part of it. The sim must stay runnable with \
         no netcode mounted (single-player is a runtime mode, not a compile variant), so sim code \
         may not depend on lightyear or on `crate::net`. Either lift the dependency out, or — if the \
         file is genuinely view/instrumentation that observes the netcode — add it to \
         NET_AWARE_FILES in tests/net_boundary.rs with the reason it earns the coupling.\n{}",
        violations.join("\n")
    );
}

#[test]
fn net_aware_allowlist_has_no_stale_entries() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let missing: Vec<&str> = NET_AWARE_FILES
        .iter()
        .copied()
        .filter(|entry| !root.join(entry.trim_end_matches('/')).exists())
        .collect();

    assert!(
        missing.is_empty(),
        "NET_AWARE_FILES names paths that no longer exist — the allowlist has rotted and is now \
         silently excusing nothing (or, worse, a renamed file is unguarded): {missing:?}"
    );
}

#[test]
fn allowlist_matches_whole_path_segments() {
    assert!(
        is_net_aware("src/net/client.rs"),
        "the layer itself is exempt"
    );
    assert!(
        is_net_aware("src/net/deep/nested.rs"),
        "the exemption is recursive"
    );
    assert!(is_net_aware("src/trace.rs"), "exact-file entries match");
    assert!(
        !is_net_aware("src/tank.rs"),
        "an unlisted sim module is scanned"
    );
    assert!(
        !is_net_aware("src/network_util.rs"),
        "`src/net/` must not match a file that merely starts with those bytes"
    );
    assert!(
        !is_net_aware("src/trace_helpers.rs"),
        "an exact entry must not match by prefix"
    );
}

#[test]
fn strip_comments_blanks_prose_but_keeps_code() {
    let src = "\
//! doc naming lightyear in prose
use crate::net::protocol::NetBot; // trailing lightyear mention
/* block /* nested */ still comment: lightyear */
let x = 1;
";
    let stripped = strip_comments(src);
    assert!(
        !stripped
            .lines()
            .next()
            .expect("first line")
            .contains("lightyear"),
        "doc comments must be blanked"
    );
    assert!(
        stripped.contains("use crate::net::protocol::NetBot;"),
        "code before a trailing comment survives"
    );
    assert_eq!(
        stripped
            .lines()
            .nth(1)
            .expect("second line")
            .matches("lightyear")
            .count(),
        0,
        "a trailing comment is blanked"
    );
    assert!(
        !stripped
            .lines()
            .nth(2)
            .expect("third line")
            .contains("lightyear"),
        "nested block comments must be blanked to their true end"
    );
    assert!(
        stripped.contains("let x = 1;"),
        "code after a nested block comment survives"
    );
    assert_eq!(
        stripped.lines().count(),
        src.lines().count(),
        "line numbers survive"
    );
}
