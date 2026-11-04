//! The game's damage-model authoring data; game state and presentation only.
//!
//! Writes `$OUT_DIR/armaments_index.rs`: every round file (`assets/rounds/<key>.round.ron`) and
//! every gun file (`assets/guns/<key>.gun.ron`) as `(file stem, include_str!(absolute path))`, one
//! `&[(&str, &str)]` per directory in byte order of the stem (`LC_ALL=C` order). The crate parses
//! its round and gun registries from that index, so every table is embedded and nothing is read
//! from disk at run time.
//!
//! Also admits the game damage-model crew-line unit and fire-cue age from required sourced data,
//! writing their constant interfaces to `$OUT_DIR/ballistics_authored.rs`. Camera and UI styling
//! are admitted into `camera_authored.rs` and `ui_style_authored.rs` by the same build gate.
//!
//! A file in either directory that is not `<stem><suffix>` with a non-empty stem fails the build,
//! naming the file: a stray file there is an authoring slip, never an ignored one. Dotfiles are
//! skipped (a platform's folder metadata is not data).

#[path = "src/ballistics/authored.rs"]
mod authored;
#[path = "src/presentation_authored.rs"]
mod presentation_authored;
#[path = "src/provenance.rs"]
mod provenance;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// `(constant, directory relative to the manifest, file suffix)` per index.
const INDEXES: [(&str, &str, &str); 2] = [
    ("ROUND_FILES", "assets/rounds", ".round.ron"),
    ("GUN_FILES", "assets/guns", ".gun.ron"),
];

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    println!("cargo:rerun-if-changed=build.rs");
    write_authored_data(&manifest, &out);
    let mut text = String::from("// Written by build.rs from assets/rounds/ and assets/guns/.\n");
    for (constant, directory, suffix) in INDEXES {
        let directory = manifest.join(directory);
        println!("cargo:rerun-if-changed={}", directory.display());
        writeln!(text, "pub(crate) const {constant}: &[(&str, &str)] = &[").unwrap();
        for (stem, path) in entries(&directory, suffix) {
            println!("cargo:rerun-if-changed={}", path.display());
            let path = path
                .to_str()
                .unwrap_or_else(|| panic!("{} is not UTF-8", path.display()));
            writeln!(text, "    ({stem:?}, include_str!({path:?})),").unwrap();
        }
        writeln!(text, "];").unwrap();
    }
    let target = out.join("armaments_index.rs");
    std::fs::write(&target, text)
        .unwrap_or_else(|err| panic!("writing {}: {err}", target.display()));
}

/// Required authored scalars keep constant interfaces without handwritten copies of their values.
fn write_authored_data(manifest: &Path, out: &Path) {
    let read = |file: &str| {
        let path = manifest.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
    };
    let hp = authored::crew_line_unit(&read(authored::CREW_LINE_FILE))
        .unwrap_or_else(|error| panic!("{error}"));
    let age = authored::fire_cue_age(&read(authored::FIRE_CUES_FILE))
        .unwrap_or_else(|error| panic!("{error}"));
    for (name, text) in [
        (
            "ballistics_authored.rs",
            format!(
                "pub(super) const HP_PER_REFERENCE_MM: u64 = {hp};\npub(super) const STALE_FIRE_TICKS: u32 = {age};\n"
            ),
        ),
        (
            "camera_authored.rs",
            presentation_authored::camera_constants(&read(presentation_authored::CAMERA_FILE))
                .unwrap_or_else(|error| panic!("{error}")),
        ),
        (
            "ui_style_authored.rs",
            presentation_authored::style_constants(&read(presentation_authored::STYLE_FILE))
                .unwrap_or_else(|error| panic!("{error}")),
        ),
    ] {
        let target = out.join(name);
        std::fs::write(&target, text)
            .unwrap_or_else(|error| panic!("writing {}: {error}", target.display()));
    }
}

/// Every `<stem><suffix>` file in `directory` as `(stem, absolute path)`, in byte order of the stem.
fn entries(directory: &Path, suffix: &str) -> Vec<(String, PathBuf)> {
    let listing = std::fs::read_dir(directory)
        .unwrap_or_else(|err| panic!("reading {}: {err}", directory.display()));
    let mut entries = Vec::new();
    for entry in listing {
        let entry = entry.unwrap_or_else(|err| panic!("reading {}: {err}", directory.display()));
        let name = entry.file_name();
        let name = name
            .to_str()
            .unwrap_or_else(|| panic!("{}: a file name that is not UTF-8", directory.display()));
        if name.starts_with('.') {
            continue;
        }
        let stem = name
            .strip_suffix(suffix)
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| {
                panic!(
                    "{}/{name}: every file here is `<key>{suffix}` — the stem is the key",
                    directory.display()
                )
            });
        entries.push((stem.to_owned(), entry.path()));
    }
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    entries
}
