//! Overmatch's asset tooling, never shipped (`tools/Cargo.toml`).
//!
//! Each stage reads authored input under `tools/` and writes the file the game reads under
//! `assets/`. The output's schema is the game library's, so the game and the tool cannot disagree
//! about a field. A stage's output is canonical: the same input bakes to the same bytes on every
//! platform, and each stage's tests bake twice and compare.
//!
//! Run a stage through the build queue, in the agent feature set so the game library compiles once:
//! `scripts/serial-cargo run -p overmatch-tools --features overmatch/dev_ui,overmatch/bitprobe --bin
//! <stage>`.

use std::path::{Path, PathBuf};

pub mod buildings;

/// The repository's root: the game's manifest directory, one above this package's.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools/ sits under the repository root")
        .to_path_buf()
}
