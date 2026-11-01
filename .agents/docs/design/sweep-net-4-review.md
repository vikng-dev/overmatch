# Net sweep 4: review and disposition

Subject: the game's netcode and its UI, game state only (the damage model's rounds and crew as
game state).

Base: `42135567`; lane `sweep-net-4`, bay-2. MEASURED start 2026-11-01 09:18:18 +0700. No
subagents, captures, pricing, deployment, Edgegap/Cloudflare calls, pushes or external writes.

## Sweep-net-3 review

No P1 or P2 was found. Its P3s and the weighed questions:

| ID | Severity | Disposition |
|---|---|---|
| R3-parse-doc | P3 | Fixed (`f341b793`). `parse_commands` claimed a networked tank's raw command only holds the bridge's previous parsed value. True on the server (the bridge writes `parsed.command()`; only `consume_edges` and `clear_command_on_release` touch it after). On the network client the own tank's raw command holds the frame's device writers when the parse runs: the same writers an offline run parses under the same `expect`. The fix in `42135567` is correct on both ends; its stated invariant now names both. |
| R3-trace-av | P3 | Fixed (`87b38d87`). With `AngularVelocity` off the wire, the client trace still wrote each replica's `av`/`hav` from its local static body (Avian's zero). Both are null on the client role. No other client consumer reads it: `diagnostics` reads the finite placeholder through `Option`, and the other readers (bitprobe, headless, trace's server/SP roles, sandboxes) run local bodies. `PROTOCOL_REV` 54 and `the_live_wire_has_no_unused_angular_velocity` stand. |
| R3-route-forget | P3 | Fixed (`3adb9ee0`). Route removal of a projectile without a `Shot` was the one unnamed route absence: an unkeyed emission `queue_fire` never routed. Ownerless bots, a spent private receipt, a disconnected recipient and an empty observer set are named game states with metrics counters, not fifth outcomes. |
| Handshake | No finding | `protocol_id` folds the embedded source bytes the registries parse, once per process (`OnceLock`): stricter than the parsed values (any byte edit changes the tag). A mismatch reaches the player as the joining hint naming server availability or build/asset skew, as sweep-net-3 recorded. |
| Receipt refusals | No finding | Non-finite fire origins and impossible smallest-three rotations are typed refusals. DERIVED: a stored-square sum in (0.75, 1) decodes to a valid, non-canonical unit quaternion, not a malformed pose. |
| Delay contract | No finding | The code computes the empirical percentile law, `max(Q_p{l} - min{d}, 0)` plus one send interval (`ArrivalStats::spread`), as sweep-net-3 corrected; sweep-net-2's "link's own worst lateness" is not what it computes. |

## Assigned inventory (N4)

| Finding | Disposition |
|---|---|
| `TankSimSource` / `server.rs` `source.get()` (5 sites, + bitprobe, sandbox, scenario, fuzz, debris) | Fixed (`a788a590`). Required `Res` for the blueprint and track colliders, `get()` returns `TankContent`; a missing bake is a param-validation panic. |
| `respawn_player_tanks` `Option<Res<TerrainMap/SpawnObstacles>>` | Fixed (`a788a590`). Both required; the test-course world inserts its empty obstacle list. `HeightGrid` stays optional, named (the slab world). |
| `walls.rs` publish `single()`, `unwrap_or_default`, replica `single()`, shrink | Fixed (`e48b0970`), as `destruction.rs` already was (`b20301bc`). |
| `destruction.rs` / `walls.rs` "record shrank" | Classified (`e48b0970`): the authority only appends and a snapshot carries its whole list, so a shorter list on one entity is a malformed peer record: logged, then replaced. Not a panic: it is outside input. |
| `destruction.rs` entry naming no tree | Already handled: a logged refusal of a malformed peer entry (the lookup's `get_mut` was already an `expect`). |
| `debris_stress.rs` `carry_wall_piece` | Fixed (`6f898f29`); partly disagreed: an `expect` alone would panic on the carrier, which holds the group's `WallPiece` itself. The carrier is named and skipped; a pose-less piece panics. |
| `fire_presentation.rs` recovery and three `single()` | Fixed (`ab7b432d`). Missing muzzle named (rig attaches after the arm); bore `expect`; a load index past the list is a logged peer refusal before the cosmetic default; ping/input-slot/armed-root reads name none and panic on many. |
| 13 UI `Single` sites (`hit_feel`, `spawn_map`, `lobby_ui`, `state`) | Fixed (`591e9bab`, `06888667`). Nodes spawned once are queries with `expect`; the primary window goes through `state::primary_window` (none named, two a bug). `591e9bab` alone fails the kill cam's composed test; `06888667` gives the bare mount its panel. Land them together. |
| `command.rs` `snapshot_device_command` | Fixed (`f341b793`), through `tank::controlled_one`. |
| `lobby_ui.rs` `groups().unwrap_or_default()`, `link else return` | Fixed (`591e9bab`). |
| `worker.rs` thread spawn, beacon resolve | Fixed (`bd3350af`). Each request answers its own refusal; the resolve cause is logged. |
| `lib.rs` asset root absolute | Fixed (`9e73ec50`): refuse start, exit 2. |
| `lib.rs` `cycle_transmission_feel`, code-branching net-A | Already fixed in the base (sweep-net-1); the stale key doc in `spawn_map.rs` is corrected (`591e9bab`). |
| `quit.rs` handler-install arms | Disagreed: an OS/winit fact with visible logging; the tripwire the inventory proposes needs winit's delegate class, which only `EventLoop::new` registers, on the macOS main thread no test thread is. |
| `quit.rs` `check_quit_latency` | Disagreed: the warning is the stated choice, and `the_latency_guard_fires_only_on_modes_that_can_swallow_a_quit` pins the two modes the roots insert. |

Base-truths read of every file in the share: no finding (no per-object rule, named asset or
per-asset constant in logic; the fixture round keys in `fire_presentation` tests are labelled
fixture data asserting a class invariant).

## The net area's sweep is complete

Still open, with owners:

- Owner (batched feel and pricing review): the empirical receipt-spread delay (sweep-net-1, -3).
- Destruction area: `buildings::breakage::derive_from_record`'s same-generation shrink arm is
  unreachable now that `receive_ledger` moves the generation on every replacement.
- Driving area: `tank::scenario::spawn_tank_when_loaded` reads `PendingTankAssets` as optional,
  though `load_tank_assets` inserts it at `Startup`.
- Net (P3, unverified): `fire_presentation::loaded_of` reads round 0 while no `WeaponLoads` has
  arrived; whether it can arrive after `WeaponGate` (same spawn, same disclosure) was not checked.

## Verification

MEASURED, unattended and headless through the queue: CI clippy (locked, workspace, all targets,
`dev_ui,bitprobe`) and `cargo fmt --all --check` clean on the final tree. Targeted batches: 197
passed (changed modules, `bake::`, `doc_citations`, `ui_ascii`, `net_boundary`); 295 passed (`net::`,
`headless_test`); 1684 run with 1 failure (the kill cam composition, fixed in `06888667`), then 193
passed (`damage_cam`, `spawn_map`, `shot_transport`).
