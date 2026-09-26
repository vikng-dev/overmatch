# Debug menu — consolidate debug visualisations behind a proper UI

Status: needs-triage

## Thought

The debug visualisations are accumulating as hardcoded, always-on toggles. We want a **proper in-game debug menu** to control them individually rather than baking each on/off into code.

## Current debug surface (what the menu should govern)

All dev-only (`#[cfg(debug_assertions)]`):

- **X-ray** — press `X`, makes the tank translucent so inner gizmos show through (`src/debug.rs`, `toggle_xray`).
- **Suspension force arrows** — cyan per-wheel load arrows, currently always drawn (`src/debug.rs`, `draw_suspension_forces`).
- **Avian physics gizmos** — collider wireframes + raycast rays/hit-points/normals, currently always on via `PhysicsDebugPlugin` (`src/lib.rs`). Configurable per-category via `PhysicsGizmos` in `GizmoConfigStore` (colliders, AABBs, contacts, raycasts each have their own colour/toggle).
- **(coming)** drive thrust vectors, lateral-friction vectors, COM marker, per-wheel numeric load readout.

## Goal

One menu to toggle each visualisation independently (and ideally tweak a few live values — e.g. force-arrow scale). Likely `bevy_egui` or a small Bevy-UI panel. Replaces the scattered key toggles / always-on draws.

## Notes

- Keep it dev-only (stripped from release, like the current debug module).
- Don't block current driving work — this is a quality-of-life consolidation for when the debug surface is larger.

## Comments

**2026-09-26 — surface drift since filing.** The "current debug surface" above predates the
`dev_tools` feature. As of `src/debug.rs` today: the debug helpers are gated on `dev_tools`
(default-on, release builds included), not `debug_assertions`; the always-on draws are gone —
one master switch `ShowGizmos` (key `G`, off by default) drives both our belt-contact arrows
(`draw_wheel_forces`, which replaced `draw_suspension_forces`) and Avian's collider gizmos; `X`
(x-ray) and `F` (detach camera) remain separate keys; a ring-buffered impact marker rides the same
switch. `bevy_egui` is now a dependency behind `dev_ui` (the sandbox bins). The goal — one menu
with per-visualisation toggles instead of one master key — still stands.
