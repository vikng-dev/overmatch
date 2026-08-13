//! Guard: `src/` contains no shape-cast call site. An INVERTED tripwire — it fires when the first
//! one appears.
//!
//! parry3d 0.27.0's GJK shape cast (`minkowski_ray_cast`) takes a stagnation exit: when the upper
//! TOI bound stops moving under float cancellation against a large shape's support coordinates, the
//! last-chance return hands back the unrefined LOWER bound. Hits come back short, one-sided, by an
//! amount that scales with the target collider's extent — measured 0.25 mm at 5 m half-extent
//! against 139–172 mm at 500 m. Raycasts do not reach it: cuboid slabs, heightfield traversal and
//! trimesh BVH descent are analytic, and the GJK raycast shapes in this tree (tree trunks,
//! tank-part hulls) are metre-scale in their local frames.
//!
//! ADR-0015 answers the defect with a map-authoring rule — *prefer tiling large static colliders to
//! ≤10 m extents* — which is currently DORMANT and unenforced: the kalinovo terrain is one 1500 m
//! heightfield and the fallback slab is 1000 m, both raycast-only. The workaround that once made
//! the rule live (witness-geometry distance reconstruction in the raycast-era suspension probe) was
//! deleted with that suspension, not because parry shipped a fix, and parry 0.27.0 still ships the
//! defect.
//!
//! **So the rule has no live consumer to guard, and the first shape cast to land inherits the
//! defect immediately.** If this test fails: the ≤10 m rule is live again. Re-arm it — size the
//! collider the new cast hits, or reconstruct distance from the hit's witness geometry
//! (`point1`/`normal1` stay exact when the TOI is wrong) — and update ADR-0015's dormancy
//! annotation before adding the call site to this scan's expectations.
//!
//! No allowlist. A shape cast is not something to grandfather in a table; it is a decision to take
//! deliberately, with the defect in hand.
//!
//! Only *code* is scanned — comments are stripped first, so prose about parry (this file's own
//! neighbours in the design record included) is documentation, not a call site.

use std::path::{Path, PathBuf};

/// The tokens that constitute a shape-cast call site: avian's spatial-query entry points, its
/// caster component, and the parry-facing option and result types. Any one of them means a cast
/// through `minkowski_ray_cast`.
const SHAPE_CAST_TOKENS: &[&str] = &[
    "cast_shape",
    "cast_shapes",
    "shape_hits",
    "ShapeCaster",
    "ShapeCastConfig",
    "ShapeCastOptions",
    "ShapeHitData",
];

/// Blank out `//`-line and `/* */`-block comments, preserving everything else byte-for-byte (so
/// line numbers in a failure message still point at the source). String literals are left intact:
/// these tokens are Rust identifiers, not prose the UI renders, and a system name or trace field
/// spelling one is worth the look.
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

/// Every shape-cast token in `src`, as `(line number, token)`.
fn hits(src: &str) -> Vec<(usize, &'static str)> {
    let mut out = Vec::new();
    for (n, line) in strip_comments(src).lines().enumerate() {
        for token in SHAPE_CAST_TOKENS {
            if line.contains(token) {
                out.push((n + 1, *token));
            }
        }
    }
    out
}

#[test]
fn src_contains_no_shape_cast_call_site() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rs_files(&root.join("src"), &mut files);
    files.sort();

    // An inverted tripwire passes when it finds nothing, so a broken walk would pass loudest.
    assert!(
        files.len() > 50,
        "found only {} files under src/ — the walk broke, and this gate proves nothing",
        files.len()
    );

    let mut found = Vec::new();
    for path in &files {
        let rel = relative(&root, path);
        let src = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        for (line, token) in hits(&src) {
            found.push(format!("{rel}:{line}: {token}"));
        }
    }

    assert!(
        found.is_empty(),
        "a shape cast has entered the sim. parry3d 0.27.0's GJK stagnation exit returns an \
         unrefined lower bound against large shapes, so this cast's distance is short by an error \
         that scales with the collider it hits (139–172 mm at 500 m half-extent). ADR-0015's \
         map-authoring rule — tile large static colliders to 10 m extents — was written for this \
         exact moment and has been dormant since the last shape-cast consumer was deleted. Re-arm \
         it: bound the extent of whatever this cast hits, or reconstruct the distance from the \
         hit's witness geometry, then update ADR-0015 and this file.\n{}",
        found.join("\n")
    );
}

#[test]
fn the_scanner_finds_a_cast_and_ignores_prose() {
    let src = "\
//! A doc comment discussing ShapeCaster and cast_shape in prose.
use avian3d::prelude::ShapeCaster;
/* block /* nested */ still comment: ShapeCastConfig */
let hit = query.cast_shape(&collider, origin, rot, dir, &config, &filter); // shape_hits
let x = 1;
";
    let found = hits(src);
    assert_eq!(
        found,
        vec![(2, "ShapeCaster"), (4, "cast_shape")],
        "the scanner must catch code and only code, at the right lines: {found:?}"
    );
    assert!(
        strip_comments(src).contains("let x = 1;"),
        "code after a nested block comment survives"
    );
    assert_eq!(
        strip_comments(src).lines().count(),
        src.lines().count(),
        "line numbers survive"
    );
}
