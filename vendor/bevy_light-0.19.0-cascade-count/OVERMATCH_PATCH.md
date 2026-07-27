# Overmatch Bevy Light patch

This directory is the published `bevy_light` 0.19.0 crate with one source change: the backport of
upstream PR #24807 ("Prevent panic in `check_dir_light_mesh_visibility`", merged 2026-07-01,
milestone 0.19.1) in `src/lib.rs`.

`check_dir_light_mesh_visibility` accumulates per-cascade visible entities into a
`Local<Parallel<Vec<Vec<Entity>>>>`. The per-cascade `resize(view_frusta.len(), ..)` runs only in
the `par_iter` init closure, i.e. only on compute-pool threads that receive a chunk this frame,
while the collect loop indexes **every thread slot ever created**. A slot left at a smaller cascade
count by an idle thread is indexed with the new count → `index out of bounds` panic. On stock
0.19.0 there is no in-app runtime sequence that avoids it when the cascade count grows. The
backport resizes every thread slot before each view's par_iter, exactly as merged upstream.

This patch is what makes the runtime ShadowCascades setting safe. The full mechanism record,
upstream status, and validation evidence live in
`.agents/docs/upstream/bevy-cascade-count-stale-local-parallel.md`. Remove this vendored crate when
upgrading to bevy 0.19.1 or later (a 0.19.0-versioned patch entry against a newer bevy is
stale-but-harmless: cargo warns "patch not used").
