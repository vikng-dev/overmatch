//! The gates and upstream tripwires no single module owns: source scans over the whole tree, gates
//! over every tank asset, the dependency graph's glam pin, and pins on engine behaviour the game
//! relies on. Each file's module doc says what it guards and what its failure means. The netcode
//! layer's own are `net::tests`.
//!
//! A source scan walks `src/` when it runs, this directory included, so a scanner whose own tables
//! or fixtures spell what it hunts for names its own path and skips it.

mod bevy_ktx2_uastc_fallback;
mod bevy_shadow_view_render_layers;
mod crew_faces;
mod determinism_deps;
mod doc_citations;
mod flow_boundary;
mod gpu_layout;
mod shape_cast_reachability;
mod ui_ascii;
