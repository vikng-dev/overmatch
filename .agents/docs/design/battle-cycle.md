# Battle cycle

Subject: Overmatch's match flow and the game's damage model, game state only.
The owner chose the provisional mode in #220; #240 supplies its authority clock.

## Boundary decisions

- An empty server is `BattlePhase::WaitingForPlayer`, with no deadline. The first
  authenticated connection starts combat on that authority tick (#271). This is the
  same connected-link arrival that previously started the fill-up clock.
- A joining player gets one tank as soon as placement permits. Bots can already be
  present while the server waits; the first player releases them too. Late joins
  during open combat use the same placement rule. Instrument placement remains explicit.
- Connections after late join closes are refused at the handshake
  (`JoinRefusal::BattleClosed`) until the next Battle reopens admission. The joining
  screen shows that cause; matchmaking returns to its lobby with it.
- Only combat scores. Existing bots enter the score at combat start; combat spawns
  open standings immediately. Standings survive disconnect and respawn. Combat runs
  its full duration regardless of occupancy or score. The hosting empty timeout may
  retire an empty server after combat finishes, never during it.
- Results select the authority's winner, draw or no contest, remove tanks and flying
  rounds, and freeze the score. Connected links remain.
- The results deadline increments the Battle number, resets score and spawn picks,
  rebuilds the world and queues every connected player for a fresh life. With anyone
  connected, that tick is already combat. Otherwise the new Battle waits for a player.
  Combatant identities and life counters retain their server lifetime.

Durations remain required mode data in `assets/modes/battle.ron`: combat, late join
and results. The owner's choices are CLAIMED from #220; #271 removes the timed
fill-up, its data and its admission check. Tick conversion rounds up and refuses
spans outside the wrapped signed comparison horizon. Clients receive the authority's
phase, deadlines, Battle number and outcome; they never advance a phase locally.

## Fresh-world construction

The next Battle is the world boundary. Results retain the completed battlefield;
the results deadline increments the replicated `BattleState.number` and reconstructs the mutable
world in the same authority command flush. Clients derive the same construction
from that replicated number before their fixed loop or record consumers run. No
countdown or reset message advances a client world.

`scatter::ScatterScene` retains parsed manifest/grid data and shared view handles.
Its `construct` method builds both startup and subsequent worlds through
`construction::spawn` and `trees::spawn`, with complete state from data. Despawning
map roots also removes their standing colliders and views. Loose bodies are
separate lifetime owners: fallen construction groups and freed trunks, including tree
segments reparented under a trunk, are removed too. Rebuilding reuses stable
`MapInstance` ids and ordinals; ECS entity identities are new.

| Mutable state | Construction and boundary | Late join |
|---|---|---|
| Construction roots, elements/chunks, structural state, cracks and holes | Fresh `Breakage` and intact element colliders from the construction data; old roots/views and loose groups depart | Current-generation `Ledger`, copied into `Record`; collision and drawing derive from it |
| Trees, segment HP pools, cuts, loose trunks and their collision copies | Fresh tree and full segment pools from tree data; old root and freed-body ownership both depart | Current-generation `DestructionLedger` plus interpolated trunk pose carriers |
| Construction debris and stress-fixture debris | Bodies and carriers depart; new Battle starts without loose pieces | Only current Battle carriers are spawned by authority |
| Cosmetic track prints | Startup print-store constructor, new GPU images and empty shoe memory; old uploads address unbound images | View-local history begins at join |
| Ground scars and other billboards | Occupants depart and their rings use the startup constructors | View-local effects begin at join |
| Terrain samples/collider, immutable terrain field, ground material/masks, lighting and shared meshes | Immutable construction retained; terrain has no deformation state | Shared shipped data, checked by the content fingerprint |

Each ledger and environment pose carrier carries `WorldGeneration`, selected by
the Battle number. The authority replaces ledger entities rather than truncating
append-only diff histories. Wall publication derives its cursor from the current
ledger identity. Replica consumers select only the current generation, so a phase
packet arriving ahead of the new records produces the intact world immediately;
a record arriving first waits for its Battle. Before any Battle snapshot, and
between links, no generation is selected. A reconnect also compares the authority
entity identity, so equal Battle numbers on different servers cannot reuse a
construction. Retired carriers remain owned by replication until their despawn
arrives, but cannot draw or supply a trunk collision copy in the next world.

The boundary does not change the shared presentation delay: motion within a Battle
still comes from its authority stream. The phase transition itself selects the
new construction immediately, consistently with the match UI's replicated phase.
The current wire contract is `net::protocol::PROTOCOL_REV`.

Contact readings and resting loads are keyed by complete ECS entity identities and
are refreshed by the next authority physics step; the prior step has already
drained the deposit queue before the boundary. They cannot address reconstructed
roots. Collider insertion/removal hooks maintain the spatial-query trees as roots
are replaced; client construction and record derivation do not wait for a fixed
update. Record derivation cursors follow the replaced ledger identity and local
record generation. Shared view templates, player settings, transport clocks and
deduplication, combatant identities and operator instrument counters keep their
session lifetime.

## The hold

Before the combat snapshot, commands are held behind the loading screen. `net::battle::hold_commands`
replaces every tank's command with `ParsedTankCommand::hold` while the replicated phase
is not combat. It runs where commands are applied: after the input bridge and a bot's
command writer, before every gameplay consumer. So drive, turret, gun, fire and every
edge are held by one rule. A held command has no aim, so the servos keep their spawn
targets, and holds the brakes (`track::drive::DriveTarget::Brake`): no drive, full
service brakes while the tank moves, then the parking latch at rest. A repairing crew
uses the same brake hold. The tank stays a dynamic body resting on its suspension.

The authority advances the phase (`cycle_battles`) before the hold in the same tick,
so combat's first tick releases every command. A client runs the same rule from its
replicated phase, which holds its own turret and fire presentation. Its phase arrives
with replication, about one one-way trip after the authority's tick (DERIVED), so the
own turret's local lead is released that much later; the authority's turret follows
the player's aim from combat's first tick. Where the hold is mounted (`net::plugin`) and no
Battle has arrived yet (a client's first frames before its Battle snapshot, a fixture that mounts
the plugin without one), every command is held. A composition without the plugin (offline) holds
nothing.

## Admission and reporting

| Phase | Heartbeat | Handshake |
|---|---|---|
| Waiting for a player | `open` | accepts below configured capacity |
| Combat, late join open | `open` | accepts below configured capacity |
| Combat, late join closed | `closed` | refuses `JoinRefusal::BattleClosed` |
| Results | `closed` | refuses `JoinRefusal::BattleClosed` |
| Process retiring | `closing` | no new Battle starts |

The Worker learns the state at its next heartbeat; an earlier ticket can arrive
later. Netcode therefore checks the same authority admission gate both when it
issues a challenge and when it confirms the response. The local netcode patch
retains the authenticated refusal through the terminal client state, so the
joining screen or reconnect overlay shows its cause and stops retries. A
matchmaking client returns to its lobby with that cause. A new connection after
closure is refused even if it previously occupied this server.

The Worker must deploy its `closed` support before a server using this cycle ships.
The lane performs no deployment.

## Crowded placement

The minimum distance is never relaxed. A crowded automatic join or requested
respawn waits and retries; a phase boundary clears pending placement, and the
next Battle places all connected players from an empty combatant field.

The owner receives typed `RespawnState`, scoped by `CombatDisclosure`; no UI text travels on the
wire. `RespawnState::waiting` means the request remains pending and retries automatically. The death
screen says "WAITING FOR SPAWN SPACE", explains automatic respawn when clear, and offers M to
re-pick. Its status line carries the same meaning through a map or menu, with the action appropriate
to that overlay. R sends no redundant request and cannot re-arm the local retry timeout. That timeout
covers only an unacknowledged request, not an authority-reported placement wait.

The authority evaluates only the newest pick per link in each frame; earlier clicks in that drain
are superseded before admission. It rejects nonfinite coordinates, clamps finite ones to the
spawn region, then searches for a candidate clear of standing map footprints. If none exists, refuse
it as `SpawnPickRefusal::NoClearGround` rather than waiting. Body occupancy is different: tanks can move or their owners
can replace them, so a geometrically admissible pick may wait for space. This distinction promises no
deadline for another player's action; re-picking is the waiting player's way out. Placement checks
again against current geometry and occupants before spawning. Any re-pick preserves a pending
request. A refused pick costs nothing: R still requests the previous accepted point, or the automatic
map spawn rule when there was no accepted point. Admission waiting and pick refusal are independent
owner-private facts. The death prompt retains R and its countdown; the status line tells a living
player why the pick was refused and that the previous spawn choice was kept. The requested dot
turns red on refusal, retains that feedback across respawns, and clears at the next Battle.
The most recent accepted point persists until replaced or the Battle boundary clears choices; a refused point never replaces it.

MEASURED: the expanded authored candidate set admits the owner's 30-player
stationary lobby from every possible first point; all candidates pass ground
and construction-footprint checks. It has 36 candidates with minimum pair separation
240 m. Moving players can still block candidates during combat, but a connected
waiter cannot remain blocked across the next Battle's fresh layout.

## Match UI

`net::battle_hud` shows the replicated phase and score only. A clock line stays on screen:
the time left in combat and results. An empty-server snapshot shows no match UI. The scoreboard lists rank, name, knockouts and losses in `rank` order; tied standings
share a place. It shows while Tab is held in combat and without a key through results, where its
title names the authority's winner (also marked on its row), the draw or no contest. It hides
whenever an input-capturing overlay is latched and otherwise draws above the death banner.
The card is bounded by screen height, including its title and hint under UI scaling. Its table
viewport shrinks within that bound; Page Up/Down moves every column together,
so larger rosters remain reachable. Hiding the board resets its scroll position.

Countdowns run from the replicated deadline on the shared presentation timeline
(`InterpolationTimeline`), the clock used to sample the server stream. Input time deliberately
leads the server and must not advance the match display. The replicated phase alone changes
the view; a countdown reaching zero cannot start or end combat. Both views wait while the
presentation clock is synchronizing. No player name rides
the wire, so a row reads `PLAYER <combatant id>`. The own row is unmarked, so the board reads the
same through results, when the client holds no tank and so no replicated fact naming its own
combatant.

An opaque loading screen covers the view (`Flow::Joining`) until this link's own tank has a
presented pose, a complete local body, a bound view and a combat or results snapshot. It shows
LOADING… through asset and tank loading, CONNECTING… through connect attempts, or the refusal's
cause. Loading leads directly into combat, without a countdown or intermediate waiting screen.
A tank that later leaves (results, a respawn) keeps the match on screen.

## Verification

The short-duration mode file `src/net/fixtures/battle-short.ron` drives the same
clock installation and protocol over real loopback UDP without a renderer. The
probe reads the replicated deadlines and score, checks the matching heartbeat
states, observes a rendered typed refusal, and keeps the first player connected
into the next Battle. A separate challenge-crossing probe closes admission
between the request and response. Score and deadline-wrap tests pin the boundary
invariants. The server spawn tests exercise complete tanks and replacement lives: a player
starts combat on its arrival tick, results sweep its tank and the next Battle gives every
connected player a fresh life immediately. The hold test drives the production clock and hold:
every command is held on an empty server and live on the first arrival's tick.
The transmission's brake-hold test brings a rolling tank to a parked rest where a released
command coasts.

The world-cycle regression uses a mixed map fixture through the startup scatter
construction. During combat a wall chunk is freed and a tree falls; the authority,
connected client, combat late joiner and next-Battle joiner must recover startup
counts and map identities in the next Battle. A separate arrival-order regression
holds old and future ledgers across the replicated transition and verifies that
neither is consumed against the wrong construction. The print-store regression
checks both empty CPU history and new GPU image identities; extraction retires
pending writes whose old image never reached the GPU. Arrival-order coverage also
reconstructs without a fixed update and reconnects to a new authority whose Battle
number coincides with the previous authority's.

The release smoke (`scripts/ci/archive-smoke.sh`) uses the shipped mode and checks that its
script completes under the launch and completion bounds below.

## Scripted workloads after immediate start

The harness waits only for its own input slot, a combat snapshot and timeline synchronization.
Its launch watchdog is finite and never extended by a replicated phase; completion has its own
script-length bound. The archive smoke and connect batch derive outer deadlines from loading
allowances and script length alone. The fill-up-only shell helper and its tests are deleted.

`scripts/cost/run.sh` creates `SPIKE_WORKLOAD_READY` only after all scripted clients hold input
slots. Both their script clocks and the server/client cost recorders wait for this file; warmup
cannot include partial-cohort ticks. `scripts/shot/run-mg-armor.sh` and
`scripts/shot/run-pencam-latency.sh` share that admission helper: their script lifetimes and firing
offsets start from the tool barrier. The pen-camera cost runner uses the same start-file contract
for its two scripts. No tool can delay the Battle. The online cost runner starts warmup from the
measured frame-stream span when all load clients have slots; the debris stress runner starts
warmup after every client reaches synchronized combat. These latter scripts outlast admission
and their requested measurement windows. The combat-start probe bounds only loading and script
length. Hosting reports waiting as open, then follows the unchanged combat/results join gate;
the stress fixture still runs its operator workload while an empty server waits for a player.

The scratch runner in `.agents/scratch/battle-verify/` is unchanged. Its `run.py` waits for
`server: Battle 2 FillUp`, uses that to stop its capture and refuses a run without it. Its
`audit.py` parses `FillUp` and reports `next_fill_up_observed`. Those assumptions require updating
before the orchestrator reuses that scratch instrument.

## Immediate-start lane verification (#271)

MEASURED: 189 targeted tests passed, with six existing tests ignored. The run includes the
Battle clock and command-release tests, spawn/score lifecycle, harness and recorder start-file
gates, loading snapshot order, HUD, hosting's real-UDP cycle and typed refusal, environment
reconstruction, and the brake-hold seam. Whole-tree `doc_citations`, `ui_ascii`, `net_boundary`
and `bake::` guards passed in that run. The wire type tripwire passes after re-pinning the
changed Battle phase; `PROTOCOL_REV` is deliberately incremented. No frame or tick cost was
sampled, and no screen or capture was taken. Formatting, CI clippy with warnings denied, and
Bash/Zsh syntax checks for every changed shell runner passed.

Ratchet review: no backlog row was closed. `net-21` and `net-22` still describe real defects in
`protocol::publish_servo_angles` / `apply_servo_angles`: ancestor state is absent from the stream,
and non-owner servo targets are applied on arrival. This lane changes only the protocol revision
and type hash in that file; the servo items and their tracked presentation decisions remain open.
This is a file-level ratchet exception, not a claim that either row is fixed.

## Historical timed-fill-up evidence

The measurements and review dispositions below describe the retired timed flow.
The current start rule and tools are defined above and in the #271 lane report.

MEASURED 2026-11-01 for `lane/sweep-net-7`: the end-of-lane smoke ran once with this bay's dev
server and client, headless and unattended under the machine lock on loopback. The client
connected and claimed its input slot during fill-up. Its harness clock reported the slot at
1.628 s, combat release at 61.093 s (script tick zero), and completion at 65.088 s after 256 idle
script ticks; exit status was zero. Formatting, CI clippy, targeted suites and the final whole-tree
`doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards passed.

MEASURED 2026-11-02 for `lane/sweep-net-8`, the same smoke the same way: the slot at 1.503 s,
combat release at 60.963 s (script tick zero, client tick 3903), completion at 64.950 s after 256
idle script ticks, exit status zero. The shortest cost run (`scripts/cost/run.sh loft1`, a
640-tick script, headless under the machine lock) took its slot at 1.714 s, was released at
61.222 s and completed at 71.216 s; its client stream holds 256 tick rows (t 384..639) and the
server's 260 (t 384..643, the four ticks until cleanup): no fill-up tick in either percentile
table. The ordering mutant of `protocol::install_shot_clock` (the `before(GameplaySet)` removed)
fails `the_clock_is_published_before_every_gameplay_reader` with a `ShotClock` access conflict;
the restored ordering passes it. Formatting, CI clippy, the targeted suites and the whole-tree
guards passed.

## World-reset review

Two blind reviews of the fresh-world lane, each finding checked against the code:

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | No P1–P3 findings | — | — |
| Claude | No test consumed a record from Battle 2 or later on a client: every later snapshot was empty, so a filter selecting only the first generation passed | verified | `net: settle the world-reset review findings (#253)`: the arrival-order test grows the second Battle's ledgers and sees them apply |
| Claude | The census counts no pose carriers, so nothing guarded the boundary's carrier despawn | verified | same commit: the UDP cycle asserts every authority carrier belongs to the new Battle |
| Claude | `vfx::reconstruct_world` retired ring ids with `World::despawn`, which warns for every id already gone | verified: `World::despawn` logs a warning for a missing entity in bevy_ecs 0.19, and a ring keeps ids past their own end-of-life despawn | same commit: `try_despawn` |
| Claude | The authority's first generation came from a code default that matched `BattleState::default().number` by coincidence; replica compositions init the same default | verified for the authority; not a defect on a production replica, where `world_reset::install_client` inserts `None` over every init | same commit: `server::install_battle` derives it from the first Battle |
| Claude | The authority holds an `Option` it can never empty, checked by five `expect`s | verified | `net: carry authority world generation in a nonempty type (#253)`; `AuthorityWorld` carries the generation directly, while replica selection retains its legitimate absence |
| Claude | `world_reset::follow_battle` had an unreachable branch whose comment promised more than it kept | verified | same commit: a frame with no Battle forgets the construction, so the next Battle constructs its own |
| Claude | `world_reset::start_next` decides from its own list which environment records exist, while startup decides through each `install_server` | verified: the short-mode probe server gains both ledgers at its first fill-up | `net: register environment constructors for startup and rebuild (#253)`; each installed publisher registers its constructor in one list used by both paths, and all generation-tagged wire roots share boundary cleanup |
| Claude | Stale docs: `Record` said its generation moves on a replica's next link; the wall ledger's module doc said one entity holds every change | verified | same commit |

## Fill-up review

Both blind reviews concern the game's match flow and damage model, game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | Loading clears on ownership before an interpolated pose and attached rig | verified: an owned `NetTank` without `Position`, `Rotation`, local `Tank` or bound view sets the old latch | `client: wait for the own tank presentation before leaving loading (#253)`; the gate now requires all of them, and remains latched through results and respawn |
| Codex | The repair test never drives repair-to-brake selection | verified: the old assertions never reached drive-target selection; the missing-repair-arm mutant fails the new assertion on the selector used by `apply_track_forces` | `track: guard repair and phase holds at the drive-target seam (#253)` |
| Claude | No test covers either hold arm at the gameplay-to-brake seam | verified: both missing-arm mutants fail independently; the production selector is shared with the repair test, which asserts repair braking, release after completion and phase braking without a repair | `track: guard repair and phase holds at the drive-target seam (#253)` |
| Codex | A combined census inequality lets either later-generation ledger consumer hide the other's failure | verified: first-generation-only wall and tree mutants fail independently at their own assertions; the later-generation wall record, derived construction elements and tree state are checked separately | `net: verify each later-generation environment consumer independently (#253)` |
| Claude | A fill-up-only connection opens a ghost standing that can win or tie the later result | verified: spawning a waiting combatant populates the old board before combat; a solo fill-up departure can therefore win without participating | `net: open standings only for combat participants (#253)`; combat release opens existing tanks, combat spawns open once, and combat standings survive disconnect and respawn |
| Claude | Harness scripts run out inside fill-up and silently exercise no combat | verified, already fixed on main | `net: start harness scripts at combat release (#253)`; `held_ticks_advance_nothing_and_release_writes_tick_zero` and the end-of-lane smoke verify the production release clock |
| Claude | Crowded-placement prose says combat queues the fresh layout | verified: `Boundary::FillUp` queues it; combat retains those tanks | `docs: record the fill-up review and measured release smoke (#253)` |
| Lane | The narrative names a pre-merge protocol revision instead of the current wire contract | verified against `net::protocol::PROTOCOL_REV` | `docs: record the fill-up review and measured release smoke (#253)`; refer to the contract's symbol |

## Harness combat-start review

Two blind reviews of the combat-start harness lane; both concern the game's scripted client,
its watchdog and the measurement scripts, game state only. Main had already landed sweep-net-7,
sweep-drive-7 (REV 63, which republishes the timeline for fire and deadline clocks and leaves the
client's sync state untouched) and the smoke's mode-file fix when these were verified.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex, Claude | Script ticks advance before timeline sync, so a late joiner's tick-0 click is spent and never filed | verified: lightyear's `buffer_action_state` reads `SyncedLocalTimeline`, which skips the system while `LocalTimelineSync::is_synced()` is false; REV 63 does not touch it; the held-ticks test now covers a combat snapshot on an unsynchronized timeline | `net: release the harness script on the synchronized timeline (#253)`; `buffer_input` releases only when the phase is combat and the timeline is synced |
| Codex, Claude | The watchdog's launch bound reads the remaining fill-up against an unsynchronized local tick, once | verified: before sync the tick counts from this process's launch, so the remainder reads the server's uptime | same commit: the watchdog accounts the fill-up only from a synced timeline, and a late join (combat first) adds none |
| Codex, Claude | Measurement windows and bounds count from launch or the first frame row, not from release | verified: `SPIKE_COST_WARMUP` skipped 384 recorder ticks from startup; `run-online-cost.sh`, `run-debris-stress.sh` and `run-pencam-cost.sh` opened at WARMUP_S of app time | `scripts: open every measurement window at combat release (#253)`; the cost recorder counts played ticks only, the frame-stream runners take the harness's `released` line as their origin, the debris runner waits for every client's release |
| Claude | P1: the smoke read `fill_up_s` from the server tarball, which never ships the mode file | verified: fixed on main | `ci: read the smoke's fill-up from the checkout's mode file (#253)` |
| Claude | `batch.sh`'s 90 s kill fires before a healthy run through the fill-up ends | verified: 3–20 s startup + 60 s fill-up + 9.4 s | `scripts: open every measurement window at combat release (#253)`; the bound is DERIVED from the mode file and the watchdog's two slacks |
| Claude | Stale prose in four runners (input-slot windows, "+128 ticks outlives the later start", "the opponent has exited by now") | verified | same commit; the pen cam runner now spans from the release, so the opponent really has exited |
| Claude | Two bools beside the clock enum can represent impossible launch states | verified | `net: release the harness script on the synchronized timeline (#253)`; `ScriptClock` is `AwaitingSlot`, `AwaitingFillUp`, `AwaitingRelease`, `Released` |
| Claude | The smoke's awk ceiling compares text, so `fill_up_s: 60.0` rounds to 61 | verified (MEASURED: the shared helper reads 60 for `60.0` and 60 for `59.25`) | `scripts: open every measurement window at combat release (#253)`; `scripts/lib/battle-mode.sh` is the one reader |
| Claude | `SMOKE_TICKS`, `SMOKE_DEADLINE`, `SMOKE_LOGS` have no tracked user | verified: release.yml sets only `SMOKE_EXPECT_API` and `SMOKE_SERVER_ASSET_ROOT` | same commit: deleted; the script length, bound and log directory are fixed |
| Claude | This record still described the one-rule watchdog | verified: fixed on main | `docs: record the fill-up review and measured release smoke (#253)` |

## Net-7 review

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | No P1–P3 findings | — | — |
| Claude | The loading latch's pose, body and `Tank` filters guard nothing: `TankViewReady` implies them | verified: `tank::view::bind_tank_view` binds only a root carrying `SimParts`, inserted with the body's `Tank` after the rig attached to the replicated pose | `client: latch the loading screen on the bound view alone (#253)` |
| Claude | `Flow::Joining`, `FlowFacts::Net` and a fixture still say "replicated" where the latch waits for presentation | verified | same commit |
| Claude | The three publishers' `init_resource::<AuthorityWorld>` keep a code default no production composition uses | verified: `server::run` always installs the Battle after them | `net: derive the authority world generation from the Battle alone (#253)`; the `Default` is gone, a publisher without a Battle fails at its constructor |
| Claude | Overlong lines rustfmt gave up on in `score.rs` and `world_reset/tests.rs` | verified: seven lines over 100 columns | `net: reflow the lines rustfmt gave up on (#253)` |

## Drive-7 review, the net path

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | `shot_transport::queue_fire`'s fire-tick arm compares the published clock with itself; the enqueued tick's agreement with the timeline rests on an untested ordering | verified: `shooting::fire` stamps the id from the same `ShotClock` in the same tick | `net: pin the clock publication before every gameplay reader (#238)`; `protocol::install_shot_clock` is the one installation, and its test refuses to build a schedule where the publication is unordered against a gameplay reader |


## Net-8 review

Subject: the game's battle cycle, scripted client, measurement windows and damage-model
presentation, game state only. Both net-8 doubles were present and verified; the Codex double
was re-read before this table. Its three findings overlap the Claude double. The ball-10 row is
this lane's assigned net path.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude, Codex (net-8) | Paired pen cam windows and the knockout cut reuse the on-run's absolute clock | verified: earlier and later off-run releases select different script intervals; the new analyzer regression failed before the fix for both directions | `scripts: align paired pen cam windows by combat release (#253)`; shift every paired mark and its cut by off-release minus on-release, read from the frame clock |
| Claude, Codex (net-8) | The release's wall clock differs from the frame rows' elapsed clock | verified: Bevy's real clock excludes startup before its first update from elapsed time; the delayed-first-update fixture failed with the old stamp | `net: stamp combat release on the frame clock (#253)`; frame runners read `frame_s`, the same elapsed clock as rows and marks; `app_s` remains the named watchdog wall clock |
| Claude (net-8) | `hold_commands` promises a pre-Battle hold but returns without holding commands | verified: a tank snapshot preceding its Battle keeps its raw and parsed intent while instruments read Held; the old return fails `a_tank_before_its_battle_snapshot_commands_nothing` | `net: hold commands while awaiting the Battle snapshot (#253)`; the absent snapshot follows the same neutral-command hold as non-combat |
| Claude (net-8) | `cost::write_tick` keeps an unreachable absent-`MatchCommands` fallback and the harness documents it | verified: both recorder compositions mount the net plugin and its publication | `cost: require the match command state in tick recordings (#253)`; a required resource replaces the optional arm and the obsolete lever wording is gone |
| Claude (net-8) | No system test guards the watchdog's sync gate | verified test gap: deleting that gate fails `watchdog_waits_for_sync_before_accounting_the_fill_up`, which runs the watchdog before and after synchronization against different remaining spans | `net: test watchdog fill-up accounting through synchronization (#253)` |
| Claude (net-8) | No regression guards held-tick exclusion or its phase publication | verified test gap: removing the recorder skip fails the persisted-row test; publishing Live outside combat fails the phase-publication test | `net: pin held tick recording and phase publication (#253)`; holds spend no warmup or rows, including results after recording begins |
| Claude (net-8) | Authority generation comes from a different default construction than the spawned Battle | verified structural defect: the defaults agree today, but changing the first Battle independently separates them; a non-default fixture detects a discarded startup value | `net: construct the startup Battle and world generation together (#253)`; one Battle value supplies both the pre-Startup generation and the spawned board |
| Claude, Codex (net-8) | `battle_fill_up_s` silently coerces compact or unreadable fields to zero | verified: the compact valid field was read as zero; the regression exercises compact, spaced and malformed fields in both Bash and Zsh | `scripts: refuse unreadable battle fill-up spans (#253)`; parse a whole numeric value independently of whitespace and refuse anything unreadable |
| Claude (net-8) | Release checkout comments claim only the smoke runner is needed | verified: the runner also sources the shared mode reader and reads the checkout's non-LFS mode file | `ci: name every checkout input to the archive smoke (#253)` |
| Claude (net-8) | The measured client's no-input-slot refusal is redundant after release and advises an obsolete warmup fix | verified: the window requires release, and the script claims its slot before it can release; independent load clients still require their own gates | `scripts: remove the obsolete pre-release input-slot refusal (#253)` |
| Claude (ball-10) | A recovered shot invents its physical birth as the muzzle | verified: the convex-envelope fixture places the muzzle beyond the source face; the muzzle-birth mutant fails its first-open-point assertion | `net: recover physical shot birth from the source envelope (#247)`; use the current rig's source proxies and authored bore through `shooting::outside_source`, retaining the muzzle as visual origin; the original shot pose is unavailable |

MEASURED 2026-11-02 for `lane/sweep-net-9`: six independent mutants were applied together to
disjoint test paths. Each failed its own named regression: the pre-snapshot return, non-combat
publication, recorder skip, watchdog sync gate, separate startup default and muzzle birth. The
source was restored exactly; the restored targeted `net::`, `cost::` and `headless_test` run
passed 342 tests with five existing ignored tests. The shell reader's two tests and the pen cam
analyzer's 24 tests passed; the clock and paired-window regressions had failed on the old paths.

MEASURED: the release smoke (`scripts/ci/archive-smoke.sh`) ran once with freshly built bay dev
binaries, headless and unattended under the machine lock on loopback. The client's slot was at
`app_s=1.635`, release at `app_s=61.113 frame_s=60.224` (script tick zero, client tick 3906),
and completion at `app_s=65.097` after 256 idle ticks; exit status was zero. No wire type changed:
`PROTOCOL_REV` deliberately remains 65. The excluded simulation modules were untouched.

Formatting and CI clippy passed before each commit. MEASURED final whole-tree guard run:
44 passed, none ignored (`doc_citations`, `ui_ascii`, `net_boundary`, `bake::`). No findings or
owner decisions remain from the assigned doubles.
