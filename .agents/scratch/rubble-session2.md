Subject: the video game's environment destruction model; game state only.

The hull sweep releases bodies before physics. The law pass can split a new body after physics in that same tick. Its parent carrier then has an empty [born, retired) interval; preserving that carrier gives the client duplicate chunk birth identities. Retirement drops that empty carrier, and the post-law publication gate refuses duplicate final identities on the server in every build. Historical carriers retain their claims until world replacement because late joiners receive the history too.

The client keeps the typed stream refusal and visible match-close path. A replica despawned while independent presentation commands wait is a legitimate absence; marker insertion/removal expires with the replica. The visible-sample component assertion remains a bug gate.

Census method: the same ignored fixture on both source revisions, installed with the shipped server carrier adapter and no network transport. The fixture uses the tracked ram recipe, shipped terrain, exact fixed steps with ShotClock published from that clock, and ends at contact +320. Lifecycle observers count collider Add/Remove (sensors included), rigid-body Add, and first component Add / final Despawn per entity. A world census at every observed fixed boundary checks complete entity coverage. No frame/tick duration is collected. Bevy 0.19.1's component-scoped lifecycle observers, and Avian 0.7.0's component contracts, were checked against pinned source.

The historical sweepfix report's collider-only fixture omitted carrier installation. These paired runs include it in both versions; added entity allocation can change later solver ordering, so historical totals are not an oracle. The matched sweepfix run nevertheless reproduces the retained collider-only totals and selected rows exactly. Both the owner's absolute worst ticks and equivalent contact offsets are reported.

DERIVED from source: the Sweep, Structure and Refit brackets end when their systems return; queued spawning, reparenting and despawning apply afterwards. Avian's collider mass, hierarchy and tree hooks execute during those flushes. Counts identify structural work that those brackets omit; they do not assign the owner's unbracketed milliseconds to a particular hook.

Decision: retain the current collider representation. All lifecycle results are observations, not obligations or tuned targets.

MEASURED: both variants first touch occupied matter at fixture tick 1263. The owner supplied pricing impact 1299 and worst tick 1301, so that worst offset (+2) corresponds to fixture tick 1265. No new timing run was taken.

All table cells use `colliders created / deleted; bodies created; entities spawned / despawned`. Colliders include sensors; bodies count RigidBody additions. These counters do not cover collider replacement or reparenting.

| Fixture tick | Contact offset | Sweepfix | Rubble |
|---|---:|---|---|
| 1263 | +0 | 131 / 2; 0; 131 / 2 | 131 / 2; 0; 131 / 2 |
| 1264 | +1 | 80 / 8; 2; 84 / 8 | 78 / 10; 0; 78 / 10 |
| 1265 | +2 | 123 / 41; 0; 123 / 43 | 1098 / 51; 7; 1112 / 53 |
| 1266 | +3 | 90 / 76; 0; 90 / 76 | 691 / 22; 20; 731 / 27 |
| 1267 | +4 | 0 / 0; 0; 0 / 0 | 0 / 0; 0; 0 / 0 |
| 1268 | +5 | 14 / 4; 0; 14 / 4 | 0 / 0; 8; 16 / 3 |
| 1287 | +24 | 91 / 59; 0; 91 / 59 | 0 / 0; 0; 0 / 0 |
| 1300 | +37 | 9 / 0; 1; 11 / 0 | 0 / 0; 0; 0 / 0 |
| 1301 | +38 | 85 / 32; 0; 85 / 32 | 0 / 0; 0; 0 / 0 |
| 1302 | +39 | 87 / 114; 2; 91 / 115 | 0 / 0; 0; 0 / 0 |
| 1303 | +40 | 0 / 7; 5; 10 / 8 | 0 / 0; 0; 0 / 0 |
| 1304 | +41 | 0 / 12; 0; 0 / 14 | 0 / 0; 0; 0 / 0 |
| 1308 | +45 | 0 / 2; 0; 0 / 3 | 0 / 0; 0; 0 / 0 |
| 1310 | +47 | 5 / 2; 1; 7 / 3 | 0 / 0; 0; 0 / 0 |

| Through contact +320 | Sweepfix | Rubble | Delta (rubble minus sweepfix) |
|---|---:|---:|---:|
| colliders created | 806 | 1998 | +1192 |
| colliders deleted | 550 | 85 | -465 |
| bodies created | 21 | 56 | +35 |
| entities spawned | 848 | 2110 | +1262 |
| entities despawned | 569 | 105 | -464 |

At the pricing spike's +2 offset, rubble adds 975 collider creations, 10 collider deletions, 7 rigid-body creations, 989 entity spawns and 10 despawns over sweepfix (DERIVED subtraction of the paired MEASURED counts). At +3 it adds 601 collider creations and 20 rigid-body creations. The added lifecycle burst is the finding; no representation was changed or duration inferred.

The census fixture and full nonzero per-tick rows are in `src/net/debris_stress/tests/ram_census.rs` and `evidence/rubble/rubble-session2-counts.json` (orchestrator scratchpad copy). Run with:

```sh
OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  tracked_ram_entity_census -- --ignored --nocapture --test-threads=1
```

Fixes: `net: omit same-tick intermediate rubble carriers (#235)` and
`net: expire presentation commands with departed replicas (#235)`. The birth-tick split
witness was red before the server fix; duplicate birth claims panic before publication.
The deferred presentation witness was red with the reported missing-entity command error;
both insertion and removal now expire on a departed replica, while construction refusal
still reaches the lobby.

MEASURED validation: formatting and CI clippy pass; 86 targeted/guard tests pass (the manual
census is ignored there); both clock-corrected manual census runs pass; the detached full
library suite passes 2,438 tests with 40 ignored and zero failures.
