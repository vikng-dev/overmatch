# #176: `slab_allocator` "Use-after-free" errors in the online client

Labels: MEASURED, DERIVED, ASSUMED, CLAIMED.

## Verdict

**Not a despawn.** Every error is one zero-vertex shell-trail mesh reaching bevy_render's mesh
allocator. A mass despawn logs none (MEASURED: 0 at the disconnect in both reproductions). Bots
fire in volleys, about one every 3 s, and each volley logs 40–72 errors (MEASURED).

### The chain (pre-fix line numbers, commit 8f116cc)

1. `src/vfx/trail.rs:278` (`attach_trail`) added every new trail's mesh **empty**
   (`empty_ribbon_mesh`, `:565-571`): position/normal/uv/color with zero vertices and an empty
   `U32` index vector.
2. `src/vfx/trail.rs:523` (`update_trails`) wrote the ribbon into that mesh every frame, and
   `ribbon_stations` empties it for the first frames of flight, while every station is inside
   `TRAIL_START_ARC` = 10 m (`:43`, `:694-695`), or when fewer than two stations remain (`:715-718`).
3. `src/vfx/trail.rs:293` → `src/lib.rs:337-341`: the `TRAIL_CAP` = 8 ring (`:40`) despawns the
   oldest trail when a new one attaches. A 28-bot volley attaches 28 trails in one frame, so 20 are
   evicted in the same frame. Their meshes were already `Added`; the handle drop becomes `Unused`
   only in the next frame's `track_assets`, so each one is extracted empty once (MEASURED: the next
   frame reports exactly 20 `removed`).
4. bevy_render 0.19.1 extracts every `Added`/`Modified` mesh, drawn or not (`render_asset.rs:311-351`).
   `allocate_meshes` skips allocating a zero-size vertex buffer (`mesh/allocator.rs:375`), but its
   copy loop still copies it (`:423-426`, index copy `:453-460`). `copy_element_data` finds no key and
   logs the error (`slab_allocator.rs:974-975`). That is 2 lines per empty indexed mesh per
   extraction. Nothing is uploaded or freed; the log line is the whole effect. The fix upstream is
   bevyengine/bevy#24960, which ships in v0.20.0 as a `WARN`
   (`upstream/bevy-empty-mesh-allocator-use-after-free.md`).

The vendored `bevy_pbr` is not involved. The allocator lives in `bevy_render` and is keyed by mesh
**asset** id, not by entity, so #173's removal batching cannot reorder anything here.

### Why the issue read it as a despawn

In `runs/online-1/bots28/bots28.log` (83fee31, under the #174 reconnect loop), both bursts of 56
fall in the frame right after a 4.6 s join stall. The client's `ConnectionTimedOut` fires in that
same frame, and the despawn of the replicated tanks follows it. The errors belong to the trails of
the first volley the client sees after the stall, not to the despawn (DERIVED from the timeline; the
reproduction below separates the two).

## Fix

`src/vfx/trail.rs`: a ribbon holds a `Mesh3d` only while it has stations. `attach_trail` spawns the
trail with no mesh. `update_trails` computes the stations first. With none, it removes the `Mesh3d`
if there is one and never writes the asset. With some, it refills the existing mesh in place, or
builds the first one (`ribbon_mesh`) and inserts it. An evicted trail never creates a mesh.
Regression test: `vfx::trail::tests::a_ribbon_holds_a_mesh_only_while_it_has_stations`. Run against
a mutant that writes empty geometry into an existing mesh, it fails at its first assertion (MEASURED).

## Reproduction

Loopback, `online.zsh` (this folder): a local `overmatch-server` with `OVERMATCH_BOT=28` and one
hidden offscreen scripted client (`SPIKE_SIM_WINDOWED=1 SPIKE_FRAME_COST_OFFSCREEN=1`, idle). The
driver waits until all 29 replicas hold sim bodies, waits 20 s, SIGKILLs the server (the client
times out about 3 s later and despawns every replicated tank), waits 12 s, then kills the client.
Both binaries are dev builds of this bay with `--features dev_ui,bitprobe`, run under the machine
lock with `OVERMATCH_MUTE=1`. No window or dialog appeared.

The instrument is `diag176.rs` (this folder), mounted temporarily with `mod diag176;` in
`src/lib.rs` and `crate::diag176::plugin(app)` at the top of `render_cost::client_plugin`:

- In the render world, before `allocate_and_free_meshes`, it predicts each frame's error count from
  `ExtractedAssets<RenderMesh>`: per extracted zero-size mesh, 1, plus 1 if it has indices.
- In the main world (`Last`), it logs each `Added`/`Modified` zero-vertex mesh with its attribute
  set and the entities that hold it.

| Run | Build | `Use-after-free` lines | Predicted by the instrument | At the mass despawn | Ribbon meshes built |
|---|---|---|---|---|---|
| before-1 | 8f116cc + instrument | **408** | 408 | 0 | (not counted) |
| before-2 | 8f116cc + instrument | **376** | 376 | 0 | (not counted) |
| after-1 | fix + instrument | **0** | 0 | 0 | (not counted) |
| after-2 | fix + instrument | **0** | 0 | 0 | 57 |

All MEASURED. A prediction matching the count exactly means every error is a zero-vertex mesh. Every
zero-vertex mesh in both before-runs has the ribbon's layout: `Vertex_Position`, `Vertex_Normal`,
`Vertex_Uv`, `Vertex_Color`, indices `Some(0)`, usage `MAIN_WORLD | RENDER_WORLD`.

- **before-2 timeline:** the 29 replicas attach at 09:28:09. Errors arrive in bursts of
  40/40/56/56/56/72/56, one burst every 3 s. The disconnect and despawn at 09:28:33 add none.
- **after-2:** the same 7 volleys, now 8 ribbon meshes built per volley (the `TRAIL_CAP`
  survivors) plus the startup prewarm ribbon: 57 in all. There are 0 zero-vertex meshes and 0
  `ERROR` lines.

The logs are in `runs/`, which is gitignored.

## Latent, not fixed

`src/track/view.rs:663` (`attach_shadow_proxies`) also adds an empty mesh: a track shadow ribbon
with no attributes, filled in by its first fit. In both before-runs it was never extracted empty:
`drive_track_views` fills it in the same frame, and the instrument logged no attribute-less mesh
(MEASURED). A proxy whose first fit fails (the `ProxyStep::Overdue` path, `:958`) would log one error
per side, once. It is left as is because it did not reproduce.
