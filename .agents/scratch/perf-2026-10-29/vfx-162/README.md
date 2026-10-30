# Effect materials and ribbon allocations, #162: parity evidence

This covers the effect sprites (muzzle and impact dressing) and smoke ribbons only. No game state and no damage-model behaviour changed.

## What changed

- **Sprites.** There is one `VfxBillboardMaterial` per family (atlas, LUT, alpha mode). Each sprite's `VfxParams` lanes live on its `Billboard` and reach the shader through one 112-slot `ShaderBuffer` (the lane table). The fragment indexes that table with the sprite's `MeshTag`, which is its ring slot. `upload_billboard_lanes` writes the changed slots at most once per frame, so ageing modifies no material and rebuilds no bind group.
- **Draws.** Each sprite keeps its own draw (`NoAutomaticBatching`). Bevy's indirect batches assign instance slots with `atomicAdd` (`mesh_preprocess.wgsl`), so a batch would draw a family in an unspecified order instead of the back-to-front sort. The issue requires draw order to stay as it is.
- **Ribbons.** A rebuild refills the mesh's own attribute and index vectors in place, with a reused station scratch. The per-frame position rewrite stays because the strip faces the camera.

## Capture

`capture.sh` runs the scripted fire trigger that `run-fire-capture.sh` drives (`SPIKE_AUTO_FIRE=1`). Runs are hidden, muted, use a fixed 1/60 s step and a fixed view-RNG seed (`OVERMATCH_CAPTURE_STEP_S` now seeds `ViewRng`), at 1280x720, and record no frame time. `compare.py` compares every RGBA channel with no tolerance. Frames were kept under `target/parity-162/`, not committed.

Dense frames: every 60 Hz frame through the first 0.3 s of the 60 s, 63 s and 66 s main-gun shots, 54 frames per run.

## Findings (all MEASURED unless labelled)

1. **The before binary does not reproduce itself.** Its runs fall into at least two simulation outcomes. Two before runs from different outcomes have 0 of 54 frames identical and differ up to 153/255, with the tank pose already different before any shot. The after binaries repeat run to run: 51 of 54 identical, differing up to 20/255 in one frame.
2. **The shipped after build draws different random picks.** Bevy 0.19 stores resources as entities and keeps global observers in an `EntityHashMap`. The new `BillboardFamilies` resource is created at plugin build, which shifts the observer entity ids. That reorders the `FireShell` observers (muzzle dressing, weapon report sound), and they share `ViewRng`. Every effect is still drawn by the same laws, with different sheet, roll, size and frame picks than the before run. Production seeds `ViewRng` from the wall clock, so no player-visible behaviour changes. Both after builds, batched and not batched, draw the same picks as each other.
3. **Proof build.** This is the shipped code, except that the resource is created in `Startup`, which keeps observer ids as before. It is compared with the before run whose pre-shot frames match it exactly (62.90–62.97 s and 65.90–65.97 s, 0 px):
   - 15 of 54 frames are bit-identical.
   - 32 frames differ by at most 4/255; all but two of them by at most 2/255, in at most 327 px.
   - 7 frames differ by up to 35/255 in up to 13,446 px. They are the frames after each 88 shell lands (63.083–63.133 s and 66.083–66.117 s), and the difference sits on the impact cluster. By 63.15 s and 66.133 s it is at most 3/255.
   - **Cause** (DERIVED from source): the transparent phase is a retained `IndexMap` sorted stably by distance, so sprites at exactly the same distance keep their insertion order. The impact layers spawn at one point. The old code modified every sprite's material every frame, which respecialized and re-inserted every sprite each frame. Tied alpha-blended sprites therefore composite in a different order until they drift apart.
   - `second-shot-before-proof-diff.png` shows the 63 s shot: before, proof, and the difference ×6.

## Tests

- `a_family_shares_one_material`
- `ageing_billboards_modifies_no_material`: 0 material `Modified` events, 6 table writes in 6 frames, and the table bytes equal each sprite's lanes at its `MeshTag` slot.
- `billboard_ring_caps_and_keeps_slots_unique`
- `ground_scars_are_ring_capped`: scar slots lie in 96..112.
- `a_warm_ribbon_rebuild_allocates_nothing`: 0 allocations over 8 warm rebuilds, counted by the test binary's per-thread counting allocator (`src/tests/alloc_count.rs`).
