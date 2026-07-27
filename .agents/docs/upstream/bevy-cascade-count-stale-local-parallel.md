# Upstream filing: `check_dir_light_mesh_visibility` panics (or silently drops cascades) when cascade count changes at runtime

Status: READY TO FILE (Yan files under his own name — repo policy).
Target: bevyengine/bevy issue tracker.
Found: 2026-07-26, Overmatch field crash (F6 shadow knob). Root-caused from vendored source, not inferred.
Local mitigation: cascade count pinned at compile time (`src/settings.rs`, `SHADOW_CASCADES`) — see its doc block, which carries the same mechanism writeup plus the audit table showing this is the only `Local<Parallel<…>>` in bevy 0.19 sized by a config value.

---

## Suggested issue title

`check_dir_light_mesh_visibility`: stale pooled `Local<Parallel<Vec<Vec<Entity>>>>` panics when `num_cascades` grows at runtime, silently drops cascades when it shrinks

## Body (ready to paste)

**Bevy version:** 0.19 (`bevy_light` 0.19.0, `lib.rs:477`). Code path unchanged on inspection of current main at time of writing — worth re-checking before filing.

**What happened**

Rebuilding a directional light's `CascadeShadowConfig` at runtime with a *different* cascade count (via `CascadeShadowConfigBuilder { num_cascades, .. }`) intermittently panics:

```
thread 'Compute Task Pool (N)' panicked at bevy_light-0.19.0/src/lib.rs:477:
index out of bounds: the len is 2 but the index is 2
```

Observed on macOS 26.5 / Apple M4, stepping a dev knob from a 2-cascade config back to a 4-cascade one. It is scheduling-dependent — sometimes many transitions survive before one panics.

**Mechanism (read from source)**

`check_dir_light_mesh_visibility` accumulates per-cascade visible entities into

```rust
mut view_visible_entities_queue: Local<Parallel<Vec<Vec<Entity>>>>
```

- The inner per-cascade length is established in `par_iter().for_each_init`'s **init closure** (`entities.resize(view_frusta.len(), …)`) — which only runs on worker threads that actually receive a chunk **this frame**.
- The collect loop afterwards walks `Parallel::iter_mut()`, which is `self.locals.iter_mut()`: **every thread-local ever borrowed**, including threads that sat out this frame.
- Because the `Local` outlives the frame, a queue still sized to the *previous* cascade count survives on an idle thread's slot and is then indexed with the *new* count.

Consequences:

- **Growing** the count (2 → 4): indexes past the stale `Vec`'s end → the panic above.
- **Shrinking** (4 → 2): no panic — the stale over-长 queue silently contributes nothing for the removed cascades, i.e. cascades are dropped without any signal. Arguably worse: it looks like a content bug, not a lifecycle bug.

Reproduction is inherently flaky because it needs a pooled thread that missed the frame where the count changed — a stress loop toggling `num_cascades` every frame under load hits it quickly.

**Why this matters**

`CascadeShadowConfig` is a public component and shadow-quality settings menus are exactly where users rebuild it at runtime. `maximum_distance` and shadow-map size are safe to change live; the cascade *count* is not, and nothing documents that.

**Possible fixes**

- Resize/clear each thread-local queue at the *collect* site (or `resize` inside the loop body rather than only in the init closure), so a stale slot can never be indexed with a fresh length; or
- Drain/clear the `Parallel` at the start of the system so every frame starts empty; or
- At minimum, document on `CascadeShadowConfig` that changing the cascade count at runtime is unsupported.

**Workaround for users**

Treat the cascade count as a compile-time constant; vary only `maximum_distance` and `DirectionalLightShadowMap::size` at runtime (neither moves a per-cascade array length).
