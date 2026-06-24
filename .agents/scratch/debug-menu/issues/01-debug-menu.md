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
