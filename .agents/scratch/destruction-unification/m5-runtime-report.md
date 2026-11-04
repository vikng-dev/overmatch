# M5: complete construction state at spawn

Subject: the video game's environment destruction model; game state only.

Implemented in **buildings: build construction state at spawn (#264)**.

`buildings::spawn` builds complete `Breakage` synchronously from admitted RON data. Its mandatory
`Box<Live>` holds the scaled structure, pools, fates, cracks, element swaps, holes and structural
scratch before the root enters the world. There is no first-touch constructor or absent live state.

`Breakage::construction` is the source map ordinal. Placed matter, departing groups, later splits,
replica derivation and ledger admission use that identity. `MapInstance` remains placement metadata;
the lifecycle fixture and ledger receive fixture work without it. `Live::group` retains source
indices, wear and holes, sharing `Arc<Structure>` and `Arc<BuildingKind>`. Ledger replacement resets
mutable state while retaining the immutable structure.

The authority evaluates independent owners through Bevy's parallel query batches. Shared impulses,
records and departures commit in the previous source-entity order. An earlier owner's immediate
plastic return can change a later moving owner's spin; `owner_batches` preserves that dependency
before the later strength check. Each deposit completes before the next field is cast. The
`strike -> structural -> weigh -> fall` laws and their inputs are otherwise unchanged. Dispatch
cost is unmeasured; this lane makes no speedup claim and takes no frame or tick prices.

## Admission and wire decision

The existing `BuildingLibrary` already admits any construction ID under its file convention and
exposes every loaded RON's bytes to `map::content_digest`. The inline column fixture passes the same
admission path as shipped constructions and needs no view asset. No parallel library or house-name
branch was introduced.

Keep the existing file and wire names. No schema, registry or construction content changed, and
`WallEntry`, `WallEvent`, `Hole` and `WallPiece` retain their serialized definitions and ordinal
values. `Breakage` is local simulation state. Consequently neither `PROTOCOL_REV` nor a fingerprint
changes. The digest and wire-pin tests passed. Tree activation stays with M11; the tools package,
house asset and production view code were untouched.

## Proof

MEASURED: the committed runtime passed CI-feature workspace/all-target clippy, formatting and
**252 tests**, with **7 existing ignored tests**. The test selection covered `buildings::`,
`net::walls::`, `net::world_reset::`, `headless_test`, the map construction digest, wire pins,
`doc_citations`, `ui_ascii`, `net_boundary` and `bake::`. This includes real UDP world replacement.

The production-plugin column fixture is a stack of three unit cubes (Fixture geometry), with
mass derived from the registry density. Before any update it checks complete state. Delivered
`ShellMarched` lines wear the upper chunk, remove the base, and split the released group by removing
its lower member. It checks source addresses, identity, retained wear and holes, shared geometry,
parent retirement, falling under the production physics plugin, replica replay and ledger reset.
The separate batch witness guards the source-return/spin dependency and grouping of independent
owners. Bevy query/parallel-buffer APIs were checked against registry source for Bevy 0.19.1;
Avian hull APIs against the vendored 0.7 source.

MEASURED mutation checks: the positive control passed, and each fault below independently failed
the same lifecycle test.

| Temporary fault | Failure observed |
|---|---|
| Leave the mutable pool empty at spawn | Initial pool equality: empty versus the construction's full pools |
| Require a GLB read during construction | The column's nonexistent view file is refused |
| Refill pools when creating a group | `departure keeps wear` |
| Renumber a group's carried addresses from zero | `source addresses survive departure` |

Optional live state is also excluded by the component's type. All mutation instrumentation was
removed; the runtime source was restored byte-for-byte from the green runtime commit.
The restored source passed CI-feature clippy and a final targeted module/source-guard recheck:
**218 passed, 3 existing ignored (MEASURED)**.

## Wall continuity observation

Fixture script: select the first breakable element of the shipped house; visit its law chunks
with `step_by(3).take(6)`; send a resolved line carrying 1.25 times that chunk's pool-equivalent
work through each selected chunk; run the production law sequence after each line. These are
Fixture inputs, not a strength target. The temporary probe was deleted after the comparison.

| MEASURED outcome | Before | After |
|---|---:|---:|
| Departed chunks | 6 | 6 |
| Groups formed | 6 | 6 |
| Group sizes | `[1, 1, 1, 1, 1, 1]` | `[1, 1, 1, 1, 1, 1]` |

No test pins those observations. No current lens-backlog row names the changed files. The diff
was checked against all review lenses; no captures or pricing were taken, and no owner decision
blocks M5. Graph reuse remains M9 and tree activation remains M11.
