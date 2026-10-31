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
//! - the **static** half is this test: no module outside `net` names a lightyear type or reaches
//!   into the private `net` implementation through either `crate::net` or `overmatch::net`.
//!
//! Deny-by-default. Every `.rs` file under `src/` is scanned unless it is listed in
//! [`NET_AWARE_FILES`], so a new module is guarded the moment it is created. Reaching for netcode
//! from one is then a deliberate act: add the file below, with the reason it earns the coupling.
//!
//! Only *code* is scanned — comments are stripped first. The house style discusses lightyear's
//! behaviour freely in prose (`tank.rs`, `camera.rs`, `aim.rs`, `track/sim.rs` and `command.rs` all
//! name it in doc comments), and that prose is documentation, not a dependency.

use std::path::{Path, PathBuf};

/// The files permitted to name the netcode layer in code, each with the reason it earns the
/// coupling. Everything else under `src/` is scanned and must stay clean. `src/net/` is matched as a
/// prefix: it *is* the layer.
const NET_AWARE_FILES: &[&str] = &[
    // The netcode layer itself.
    "src/net/",
    // The library facade declares the private `net` module and mounts `NetClientPlugin`.
    "src/lib.rs",
    // The divergence instrument reads net state by design — `LocalTimeline`, `Rollback`,
    // `ConfirmedHistory`, interpolation timelines. A passive observer of the netcode, never a sim
    // dependency: it writes no sim state.
    "src/trace.rs",
    // The world-anchored nameplate prefixes `[BOT]` from the replicated `net::protocol::NetBot`
    // marker. View layer, not sim.
    "src/hud.rs",
    // Test code: the kill cam's composed schedule mounts the network client's death screen and spawn
    // map, the readers of its gate, to prove the gate is written before them in the frame. The kill
    // cam itself names no netcode.
    "src/damage_cam/pen_cam/game/killcam/tests.rs",
    // Test code: the guard for the vendored child-collider pose patch runs lightyear_avian3d's
    // per-frame child pass on the same fixture it steps Avian's on, because both crates carry the
    // patch and the saving needs both. It mounts no netcode and the sim names none.
    "src/tests/avian_child_pose_churn.rs",
    // Test code: the crossing fixtures run the march under the networked composition's child
    // collider placement too, mounting lightyear_avian3d's after-loop child pass alone, because
    // where a moving body's colliders stand when the march casts differs between the two. It
    // mounts no netcode and the sim names none.
    "src/ballistics/recording_tests.rs",
];

/// The tokens that constitute naming the netcode layer from outside it: the dependency itself,
/// an in-crate reach into the implementation, or an executable reaching through the public crate
/// instead of using its root runtime interface.
const NETCODE_TOKENS: &[&str] = &["lightyear", "crate::net", "overmatch::net"];

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

/// The scan's controls run first: the allowlist matches whole path segments, and the comment
/// stripper blanks prose (nested blocks included) while keeping code and line numbers.
#[test]
fn sim_layer_does_not_name_the_netcode_layer() {
    for (rel, aware, why) in [
        ("src/net/client.rs", true, "the layer itself is exempt"),
        ("src/net/deep/nested.rs", true, "the exemption is recursive"),
        ("src/trace.rs", true, "exact-file entries match"),
        ("src/tank.rs", false, "an unlisted sim module is scanned"),
        (
            "src/network_util.rs",
            false,
            "`src/net/` must not match a file that merely starts with those bytes",
        ),
        (
            "src/trace_helpers.rs",
            false,
            "an exact entry must not match by prefix",
        ),
    ] {
        assert_eq!(is_net_aware(rel), aware, "{rel}: {why}");
    }
    let src = "\
//! doc naming lightyear in prose
use crate::net::protocol::NetBot; // trailing lightyear mention
let run = overmatch::net::client::run;
/* block /* nested */ still comment: lightyear */
let x = 1;
";
    let stripped = strip_comments(src);
    let lines: Vec<&str> = stripped.lines().collect();
    assert_eq!(lines.len(), src.lines().count(), "line numbers survive");
    assert!(!lines[0].contains("lightyear"), "doc comments are blanked");
    assert_eq!(
        lines[1].trim_end(),
        "use crate::net::protocol::NetBot;",
        "code before a trailing comment survives; the comment is blanked"
    );
    assert!(
        lines[2].contains("let run = overmatch::net::client::run;"),
        "an external implementation reach survives for the scanner"
    );
    assert!(
        !lines[3].contains("lightyear"),
        "nested block comments are blanked to their true end"
    );
    assert!(
        lines[4].contains("let x = 1;"),
        "code after a nested block comment survives"
    );

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
         NET_AWARE_FILES in src/net/tests/net_boundary.rs with the reason it earns the \
         coupling.\n{}",
        violations.join("\n")
    );
}
