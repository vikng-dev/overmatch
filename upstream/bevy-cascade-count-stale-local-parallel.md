# Upstream: `check_dir_light_mesh_visibility` panics when cascade count grows at runtime

Status: **DO NOT FILE — already fixed upstream.** Kept as the mechanism record + vendored-patch rationale.
Found: 2026-07-26, Overmatch field crash (F6 shadow knob). Root-caused from vendored source, not inferred.
Verified + extended: 2026-07-27 by a research agent with a minimal repro (six workaround modes, worst-case
thread-starvation pattern, 40 grow transitions per run) and a real-game validation.

## Upstream status (checked 2026-07-27)

- Issues: bevyengine/bevy **#24804** (+ duplicate #24837), both closed.
- Fix: **PR #24807** "Prevent panic in `check_dir_light_mesh_visibility`", merged 2026-07-01
  (merge commit `306aaed7ee55`), milestone **0.19.1**. Seven-line insertion: before each view's
  par_iter, every thread slot of the `Parallel` is resized to `view_frusta.len()`.
- crates.io still ships only bevy_light **0.19.0** — 0.19.1 has not been released.

## Local resolution

`vendor/bevy_light-0.19.0-cascade-count/` = registry 0.19.0 + the PR #24807 backport, wired via
`[patch.crates-io]` (same pattern as the existing bevy_pbr/bevy_reflect vendor entries).
Validated: repro survives 40 worst-case grow transitions in all modes (naive in-place swap included);
real game survived 40 grow cycles that crashed the unpatched binary on cycle 1. `Cargo.lock` diff is
2 lines (bevy_light registry → path); **glam/parry/avian untouched by construction** — the Avian
determinism landmine does not apply. With the patch, a plain in-place `CascadeShadowConfig` rebuild
is safe and zero-gap (one-frame cascade re-split pop is the only visible effect), so cascade count
can be a live setting like `ShadowDistance`. Drop the vendor entry when bevy 0.19.1 ships (a
0.19.0-versioned patch entry against 0.19.1 is stale-but-harmless: cargo warns "patch not used").

## Mechanism (verified against bevy_light-0.19.0 + bevy_utils source)

`check_dir_light_mesh_visibility` (bevy_light `lib.rs:336`, **main world**, PostUpdate
`SimulationLightSystems::CheckLightVisibility`) accumulates per-cascade visible entities into

```rust
mut view_visible_entities_queue: Local<Parallel<Vec<Vec<Entity>>>>
```

- `Parallel` is a `thread_local::ThreadLocal<RefCell<T>>`; `iter_mut()` walks **every thread slot
  ever created**, including threads that sat out this frame.
- The per-cascade `resize(view_frusta.len(), ..)` lives only in `par_iter().for_each_init`'s
  **init closure** — it runs only on compute-pool threads that receive a chunk this frame.
- The collect loop indexes every slot (`thread_entity_queue[view_dest_index]`, `lib.rs:477`) — a
  slot left at a smaller cascade count by an idle thread is indexed with the new count →
  `index out of bounds`.
- The `Local` belongs to the **system, keyed per OS thread** — no entity lifecycle reaches it.
  Empirically tested and all crash: light despawn+respawn (same frame and after 5 light-free
  frames), `shadows_enabled=false` staging, `Visibility::Hidden` staging. Mutating bounds /
  `maximum_distance` with count fixed is safe (control: 40 transitions clean). On stock 0.19.0
  there is **no in-app runtime sequence that avoids it**.
- **Shrinking is benign** (correction to the original writeup): every collect pass append-drains
  every index it visits, so slots beyond the new count are always already empty — no silent
  cascade dropping. Only **growing** is broken, matching the field observation (4→2 fine,
  2→4 crashed).

Reproduction trick (from the upstream repro, reused in ours): ~100 meshes size every pool thread's
local at the small count, then starve all but one mesh out of the par_iter on the grow frame —
makes the "flaky" crash deterministic on the first grow.

## Artifacts

- Minimal repro testbed: scratchpad `cascade_repro/` (`MODE=naive|respawn|respawn_gap|disable|hidden|bounds`),
  plus `pr24807.diff` and run logs — session-scratch, regenerate from this doc if needed.
- `src/settings.rs` `SHADOW_CASCADES` doc block carries the same mechanism story + the audit table
  showing this is the only `Local<Parallel<…>>` in bevy 0.19 sized by a config value.
