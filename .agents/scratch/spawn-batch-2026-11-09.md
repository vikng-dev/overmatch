# Construction collider spawn batches

Subject: the video game's server physics and construction collapse, game state only.

Base: `c5b55927283dc409c46bc62b10ccc7baa361001c`; branch: `lane/spawn-batch`.

Both colliders are required by the current geometry contract. The occupied sensor retains the
full material volume on `Layer::Armor`, with no contact filters. The contact hull sets joint
faces back by `LOOSE_GAP_M` and uses `piece_layers()`. The full volume preserves continuous query
geometry; the inset prevents coincident released faces from acquiring artificial contact energy.
The piece body owns the occupied mass, inertia and centre of mass. Neither collider supplies an
additional body mass. Combining the two shapes would change a fact the job requires retaining.

Each group now queues one structural install, using Bevy's typed spawn/insert batches for its
occupied and contact archetypes. It supplies `ColliderOf` immediately, initializes the body's
collider list before batch hooks append to it, and declares its known collider ancestry at spawn.
Avian avoids identical hierarchy attachments and aggregates only mass outputs that depend on
descendants. Patch mechanics and guards are inventoried in
[`OVERMATCH_PATCH.md`](../../vendor/avian3d-0.7.0-child-pose/OVERMATCH_PATCH.md).

APIs were checked against the locked Bevy **0.19.1** source: `system/commands/mod.rs`,
`world/spawn_batch.rs`, `world/mod.rs`, `relationship/mod.rs`, `observer/runner.rs` and
`world/command_queue.rs`; Avian **0.7.0** was read from the vendored backend, hierarchy, mass and
collider-tree source. Batch operations retain per-entity hooks. The BVH has a per-leaf insertion
API; its shared proxy mapping must be updated for each distinct collider.

The following are **MEASURED operation counts**, except the explicitly DERIVED command rows.
This local census mounted the production construction, crumble and offline-physics plugins,
advanced one fixed tick per update, and removed every buried/anchored support with a zero-impulse
deposit derived from its material pool. Gravity was zero to isolate installation. Both revisions
released one group of 962 chunks. These are counts at that production boundary, not claimed
whole-server command totals or hosted CPU measurements.

| Operation | Main | Lane |
|---|---:|---:|
| Fresh colliders / collider insertion hooks | 1,924 | 1,924 |
| Game departure command applications, including body reservation (DERIVED) | 1,926 | 2 |
| Bevy child-membership command applications (DERIVED) | 3,848 | 3,848 |
| Tree leaf insertions in the collapse update, all owners | 5,773 | 1,979 |
| Of those, insertions into the new group's tree | 3,849 | 1,924 |
| Body mass-helper updates during the spawn flush | 2 | 1 |
| Collider-change mass requests on the following prepare pass | 1,924 | 0 |

The game-command row is `2 + 2 * chunks` on main: reserve the body, insert its bundle, then
spawn each collider. The lane reserves the body and applies one complete group command. Bevy's
`ChildOf` hook still queues `and_modify` and `or_insert_with` for every collider, hence two
applications per collider in both revisions. Batching does not erase these engine commands.

Main inserted every fresh shape into the standalone tree, then hierarchy observers attached it
twice. The first collider also received a redundant attachment when its body's ancestor marker
arrived. The lane inserts each new leaf directly into its body's tree once. Its other 55 tree
insertions are transient standalone proxies while old element colliders are despawned; lifecycle
ordering changes with component registration. The orphan-proxy guards pass. Those 55 operations
are included in the total, rather than presented as removed work.

Main's two spawn-time mass updates both aggregated descendants (DERIVED from its unconditional
`total_mass_properties` call). The lane's one update reads the complete explicit owner values and
performs no descendant aggregate. The normal prepare update remains: on the following tick the
census saw two mass-helper calls in both revisions, with only the remaining automatic owner
aggregating on the lane. Contact-collider insertion still reads the existing prepared mass cache;
sensor insertion supplies zero. Per-collider initialization is preserved.

Temporary counters and the census test were removed. Raw local artifacts remain under
`target/spawn-batch/`: `baseline.log`, `after.log`, their `*-counts.json` summaries and the
temporary probe source. No timing experiment, process attachment or debugger was run.

The collider-fact snapshots matched on both sampled ticks, including chunk ownership, occupied
and contact points, layers, friction, material and contact mass cache. Their SHA-256 was
`8d5b26d898e2e183e28c2080ea392be113d0e153b1fc491374fdc0401cca2b1f` before and after.
The tests independently check construction-data shapes, layers, complete live membership and
occupied body properties at spawn and split. Automatic, partially explicit and self-attached
explicit bodies retain their mass-update contracts.

The insertion order changes from alternating occupied/contact leaves to one batch of each.
Chunk generation, grouping inputs and physical parameters are unchanged. Landing trajectories
were not compared by this zero-gravity census; later contact ordering and final positions remain
for the owner's hosted run and visual judgement. No observed trajectory is pinned in a test.

Gates passed: `cargo fmt --all --check`; CI clippy with `--locked --workspace --all-targets
--features dev_ui,bitprobe -- -D warnings`; targeted Avian guards (26), construction breakage
(72 passed, one existing ignored), `headless_test` (40 passed, six existing ignored),
`doc_citations` (3), `ui_ascii` (4), `net_boundary` (1), and `bake::` (40). Six independent lens
reviews are clean after addressing their findings. No applicable backlog rows name the edited
files. Test linking emitted the same nonfatal `__eh_frame` warning on main and the lane.

For the owner's timing, the tracked recipe below collapses one house after admission of 30 tanks,
with one real client and 29 bots. Print the deployment's recipe first:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
python3 scripts/perf/collapse.py \
  --schedule .agents/scratch/spawn-batch-one-house.json \
  --tanks 30 --clients 1 --duration 60 --print-server-env
```

Start each two-vCPU deployment with those printed values and
`OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1`. Set `SPAWN_BATCH_TARGET` to
its `IP:PORT` and `SPAWN_BATCH_DEPLOYMENT` to its deployment ID, then run this same command for
the main and lane server builds. The runner writes separate timestamped artifact directories.

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
python3 scripts/perf/collapse.py \
  --schedule .agents/scratch/spawn-batch-one-house.json \
  --tanks 30 --clients 1 --duration 60 \
  --target "$SPAWN_BATCH_TARGET" --deployment "$SPAWN_BATCH_DEPLOYMENT"
```

For a local rehearsal, replace the target/deployment arguments with `--rehearsal`. The runner
builds through `scripts/serial-cargo` with the required features. Neither timing command was run
in this lane.
