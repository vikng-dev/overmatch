# Review lenses

A **lens** is one core principle turned into a review checklist. The principle itself lives in
`AGENTS.md`; the lens holds only what a reviewer needs to catch a breach of it: the finding classes,
what is not a finding, and the fix each class takes.

| Lens | Principle in `AGENTS.md` |
|---|---|
| [`error-handling`](error-handling.md) | Every failure has one deliberate outcome |
| [`code-branching`](code-branching.md) | Commit to one path |
| [`base-truths`](base-truths.md) | Tanks are data; behaviour emerges from base truths |

## When a lens runs

- **Diff mode**, at each batch boundary and before a release: one reviewer per lens over every
  commit landed since the last boundary.
- **Sweep mode**, over the whole tree, area by area (the areas below): the inventory that a sweep
  lane then fixes.

## How a reviewer works

- One reviewer, one lens. It reads `AGENTS.md`, its lens and the code under review, and nothing the
  orchestrator concluded: an unanchored read is the point.
- Read-only: it edits nothing, builds nothing, runs no binaries.
- A search pattern in a lens is an instrument for finding candidates; the finding is the reviewer's
  reading of the site, never the pattern's hit.
- Done means every changed hunk (diff mode) or every file of the area (sweep mode) was read against
  every finding class of the lens.

## Severity

- **P1**: the breach changes game behaviour now, or hides a failure that shipped data or a live
  input reaches today.
- **P2**: no effect today, but the next tank, asset, input or build profile reaches it.
- **P3**: the outcome is right but unstated (a legitimate absence nobody named), or a cleanup the
  lens's rule implies.

## Report

Group by area. Each finding: severity, `file:line` and symbol, the class, what goes wrong and for
whom, and the fix the lens names. End with one line per area: counts by severity.

When a breach slips past a lens and is found later, the miss becomes a line in that lens.

## Areas

| Area | Paths |
|---|---|
| load | `spec.rs`, `assets.rs`, `bake*`, `provenance.rs`, `fingerprint.rs`, `geometry_lod*`, `substances.rs`, `ground_types.rs`, `map.rs`, `world.rs`, `settings*`, `keyed_map.rs`, `build.rs`, `tools/` |
| ballistics | `ballistics*`, `rounds*`, `damage*`, `damage_cam/`, `guns.rs`, `shooting.rs`, `firecontrol.rs`, `aim.rs`, `exact.rs`, `shot_trace.rs`, `trace*` |
| destruction | `buildings*`, `building_surface.rs`, `debris.rs`, `collider_rehome.rs`, `trees*`, `scatter.rs`, `terrain_grid.rs`, `terrain_lod.rs`, `terrain_blend*` |
| driving | `track*`, `track_prints*`, `tank*`, `tank_surface.rs` (not `track_sandbox/`) |
| net | `net/`, `state.rs`, `flow.rs`, `command.rs`, `platform.rs`, `quit.rs`, `lib.rs`, `main.rs` |
| view | `view.rs`, `camera.rs`, `hud.rs`, `drive_hud.rs`, `crew_ui.rs`, `sight*`, `overlay.rs`, `vfx/`, `sfx/`, `hull_marks*`, `render_*`, `scene_depth.rs`, `terrain_shadow.rs`, `ui_font.rs`, `branding.rs`, `lod_showcase.rs` |
| dev | `dev_capture*`, `sandbox*`, `track_sandbox/`, `debug.rs`, `bitprobe.rs`, `frame_cost.rs`, `cost.rs`, `headless_test.rs`, `bin/`, `scripts/` |

Paths are under `src/` unless named otherwise. A file no row names belongs to the area of the module
that uses it; `src/tests/` and `#[cfg(test)]` code belong to the area they test.
