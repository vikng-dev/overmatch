//! The avian3d 0.8.0 fork's guard crate (see `Cargo.toml`). The game-side seams are the game's own
//! `src/avian_ext/` and the guards are the game's own test files, included by path; what they name
//! of the game crate is stubbed here under the game's paths.

#[path = "../../../src/avian_ext/mod.rs"]
pub mod avian_ext;

#[cfg(test)]
mod tests;
