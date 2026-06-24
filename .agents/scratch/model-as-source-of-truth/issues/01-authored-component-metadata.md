# Model as source of truth — author sim/gameplay metadata on Blender objects

Status: needs-triage

## Thought

We want the **Blender model to be the single source of truth** for everything model-specific — not just geometry, but sim/gameplay data embedded on the relevant objects: gun caliber on the gun, engine HP on the hull, hull mass/COM, per-wheel suspension params, etc. Authoring these in Blender (as custom properties on objects) and having them arrive in Bevy as real components would replace the hardcoded constants currently scattered across the code.

## Current state (what this would replace/extend)

- We bind structure by **node name** in `tank.rs` (`on_tank_ready` matches `"Turret"`, `"Wheel_*"`, …) — "name = the structural contract" (per `.agents/CONTEXT.md` / ADR-0002 reactive attachment).
- Per ADR-0005 the model currently contributes **geometry only**; all data lives in code constants (`HULL_*`, `MUZZLE_SPEED`, suspension stiffness, etc.).
- This thought flips that: model-specific *data* moves into the model alongside the geometry.

## Leads to investigate (VERIFY against Bevy 0.19 — community tooling often lags engine versions; we are on bleeding-edge 0.19 + Avian 0.7)

- **glTF "extras" / custom properties** — Blender object custom properties export into glTF `extras`; Bevy's glTF loader exposes them (`GltfExtras` component). Lowest-level, no extra deps.
- **Blender→Bevy component authoring addons** — e.g. *Blenvy* (formerly `Blender_bevy_components_workflow`) and the `bevy_gltf_components` / `bevy_registry_export` family: author real Bevy components on objects in Blender, auto-spawn them on load via the type registry. This is the "proper rigging system" the thought is about.
- Consider reflection/registry requirements (components must be `Reflect` + registered) and how this interacts with our reactive name-binding.

## Open questions for the discussion

- Scope: which data genuinely belongs *on the model* (caliber, HP, hub params) vs. stays code/data (tuning knobs we iterate on rapidly)?
- Does this supersede name-based binding, or layer on top of it?
- Bevy 0.19 / Avian 0.7 compatibility of the tooling — the deciding constraint given how new our stack is.
- Authoring ergonomics vs. a dependency + workflow we'd be committing to (lock-in → likely an ADR).
