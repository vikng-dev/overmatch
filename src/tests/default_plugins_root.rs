//! Guard: one constructor names Bevy's `DefaultPlugins`.
//!
//! `crate::default_plugins` roots the asset server at `assets::asset_root()`. A composition root
//! that mounts `DefaultPlugins` itself gets Bevy's own base-path chain instead, which for a
//! hand-launched `target/debug` binary is `target/debug/assets`, where no assets are, while the
//! bake and every direct reader of `asset_root()` open the checkout's `assets/`.
//!
//! Clippy's `disallowed-types` cannot hold this: it lints a single-name `use` and a type position,
//! never a path expression, and every root writes `DefaultPlugins` as an expression under
//! `use bevy::prelude::*` (MEASURED with clippy 1.98 on a probe crate re-exporting a unit struct the
//! way `bevy::prelude` does: the `use` and the type position linted, the glob-imported expression,
//! the method call on it and the fully qualified expression did not).
//!
//! Only code is scanned (comments stripped). String literals are read as code, so this file, whose
//! constants spell the name, skips itself.

use std::path::PathBuf;

use super::flow_boundary::{collect_rs_files, names, strip_comments};

const SELF: &str = "src/tests/default_plugins_root.rs";
const CONSTRUCTOR_FILE: &str = "src/lib.rs";
const CONSTRUCTOR: &str = "pub fn default_plugins()";
const NAME: &str = "DefaultPlugins";

#[test]
fn only_the_constructor_names_default_plugins() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rs_files(&root.join("src"), &mut files);
    files.retain(|path| *path != root.join(SELF));
    files.sort();
    assert!(
        files
            .iter()
            .any(|path| *path == root.join(CONSTRUCTOR_FILE)),
        "the scan must see the constructor's file"
    );
    let mut outside = Vec::new();
    for path in &files {
        let code = strip_comments(&std::fs::read_to_string(path).expect("readable source"));
        let code = if *path == root.join(CONSTRUCTOR_FILE) {
            let start = code
                .find(CONSTRUCTOR)
                .expect("the constructor is in its file");
            let end = start + code[start..].find("\n}\n").expect("the constructor ends");
            assert!(
                names(&code[start..end], NAME),
                "the constructor no longer names {NAME}"
            );
            format!("{}{}", &code[..start], &code[end..])
        } else {
            code
        };
        if names(&code, NAME) {
            outside.push(
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
    }
    assert!(
        outside.is_empty(),
        "{NAME} outside `crate::default_plugins` (build the root on the constructor): {outside:?}"
    );
}
