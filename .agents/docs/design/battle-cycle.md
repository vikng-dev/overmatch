# Battle cycle

Subject: Overmatch's match flow and the game's damage model, game state only.
The owner chose the provisional mode in #220; #240 supplies its authority clock.

## Boundary decisions

- The first fill-up is idle until the first authenticated player connects. Once
  started, it runs to its deadline even if that player leaves. Later fill-ups
  start immediately when players remain connected; otherwise they wait too.
- Tanks exist from fill-up through combat. A connected player gets a tank as soon
  as the Battle can place one: on joining during a fill-up, and at the start of every
  fill-up for players already connected, into the world that boundary rebuilds. Each
  is a fresh life through the automatic map spawn rule, held until combat starts
  (see The hold); combat starts in the fill-up's tanks. Late joins during open combat
  use the same rule. Bots follow it too. The operator's harness instruments retain
  their declared placement.
- A connection after late join closes is refused at the handshake
  (`JoinRefusal::BattleClosed`) until the next fill-up reopens admission. A direct
  client's loading screen shows that cause and stops retrying; a matchmaking client
  returns to its lobby with it.
- Only combat scores. A combatant's standing remains after disconnect, so leaving
  cannot erase its result. Combat runs its full duration regardless of occupancy,
  losses or score. The hosting empty timeout cannot interrupt combat; an expired
  empty timeout can retire the server once combat finishes.
- At results, the server chooses a unique winner, a draw among tied best standings,
  or no contest for an empty board. It removes tanks and flying rounds, including
  detached turrets through the existing root-removal observer. The score freezes.
- The next fill-up increments the Battle number, empties the score and clears
  spawn-map picks; its spawns then open each combatant's zero standing. Connected
  links stay; their combatant identities remain unique for the server lifetime and
  their life counters continue across Battles.
- Every Battle starts with a fresh battlefield. The next fill-up reconstructs the
  map, so connected players see intact scenery while the next Battle fills.

All phase durations are required mode data in `assets/modes/battle.ron`. The
owner's chosen durations are CLAIMED from #220: fill-up 60 s, combat 900 s,
late join 600 s from combat start, results 15 s. Durations round up to authority
whole ticks and fit a wrapped signed deadline comparison. Clients receive
`BattleState` with the phase, exact deadline, Battle number and authoritative
result. They never advance a phase or select the result locally.

## Fresh-world construction

The next fill-up is the world boundary. Results retain the completed battlefield;
fill-up increments the replicated `BattleState.number` and reconstructs the mutable
world in the same authority command flush. Clients derive the same construction
from that replicated number before their fixed loop or record consumers run. No
countdown or reset message advances a client world.

`scatter::ScatterScene` retains parsed manifest/grid data and shared view handles.
Its `construct` method builds both startup and subsequent worlds through
`buildings::spawn` and `trees::spawn`, with complete state from data. Despawning
map roots also removes their standing colliders and views. Loose bodies are
separate lifetime owners: fallen building groups and freed trunks, including tree
segments reparented under a trunk, are removed too. Rebuilding reuses stable
`MapInstance` ids and ordinals; ECS entity identities are new.

| Mutable state | Construction and boundary | Late join |
|---|---|---|
| Building roots, elements/chunks, structural state, cracks and holes | Fresh `Breakage` and intact element colliders from the building data; old roots/views and loose groups depart | Current-generation `WallLedger`, copied into `WallRecord`; collision and drawing derive from it |
| Trees, segment HP pools, cuts, loose trunks and their collision copies | Fresh tree and full segment pools from tree data; old root and freed-body ownership both depart | Current-generation `DestructionLedger` plus interpolated trunk pose carriers |
| Building debris and stress-fixture debris | Bodies and carriers depart; new Battle starts without loose pieces | Only current Battle carriers are spawned by authority |
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
The wire surface deliberately moves to protocol revision 57.

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

Until combat starts a player controls only the camera. `net::battle::hold_commands`
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
the player's aim from combat's first tick. A composition with no Battle
(offline, fixtures) holds nothing.

## Admission and reporting

| Phase | Heartbeat | Handshake |
|---|---|---|
| Idle or running fill-up | `open` | accepts below configured capacity |
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
next combat places all connected players from an empty combatant field.

MEASURED: the expanded authored candidate set admits the owner's 30-player
stationary lobby from every possible first point; all candidates pass ground
and building-footprint checks. It has 36 candidates with minimum pair separation
240 m. Moving players can still block candidates during combat, but a connected
waiter cannot remain blocked across the next fresh combat layout.

## Match UI

`net::battle_hud` shows the replicated phase and score only. A clock line stays on screen:
the wait for a first player, the time until combat, the time left in combat, the time left in
results. The scoreboard lists rank, name, knockouts and losses in `rank` order; tied standings
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

Until this link's own tank has a presented pose, a complete local body and a bound view, an opaque loading screen
covers the view (`Flow::Joining`): LOADING… through the asset load and the wait for the tank,
CONNECTING… through connect attempts, and a refusal's cause once refused. A tank that later
leaves (results, a respawn) keeps the match on screen.

## Verification

The short-duration mode file `src/net/fixtures/battle-short.ron` drives the same
clock installation and protocol over real loopback UDP without a renderer. The
probe reads the replicated deadlines and score, checks the matching heartbeat
states, observes a rendered typed refusal, and keeps the first player connected
into the next fill-up. A separate challenge-crossing probe closes admission
between the request and response. Score and deadline-wrap tests pin the boundary
invariants. The server spawn tests exercise complete tanks and replacement lives: a player
present at a fill-up holds a tank, combat starts in it, results sweep it and the next fill-up
hands every connected player a fresh one. The hold test drives the production cycle and hold:
every command is idle with the brakes held through the fill-up, and live on combat's first tick.
The transmission's brake-hold test brings a rolling tank to a parked rest where a released
command coasts.

The world-cycle regression uses a mixed map fixture through the startup scatter
construction. During combat a wall chunk is freed and a tree falls; the authority,
connected client, combat late joiner and next-fill-up joiner must recover startup
counts and map identities in the next Battle. A separate arrival-order regression
holds old and future ledgers across the replicated transition and verifies that
neither is consumed against the wrong construction. The print-store regression
checks both empty CPU history and new GPU image identities; extraction retires
pending writes whose old image never reached the GPU. Arrival-order coverage also
reconstructs without a fixed update and reconnects to a new authority whose Battle
number coincides with the previous authority's.

The release smoke (`scripts/ci/archive-smoke.sh`) runs against the shipped mode with its 60 s
fill-up, and the scripted client's watchdog keeps its one rule: the client takes its input slot
on joining the fill-up. MEASURED 2026-11-01, this lane's dev server and headless client on
loopback: input slot 2.95 s after the client's first log line and 16 ms after `client:
connected`, while the server's Battle 1 fill-up ran; script complete 4.02 s later; exit status 0.

## World-reset review

Two blind reviews of the fresh-world lane, each finding checked against the code:

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | No P1–P3 findings | — | — |
| Claude | No test consumed a record from Battle 2 or later on a client: every later snapshot was empty, so a filter selecting only the first generation passed | verified | `net: settle the world-reset review findings (#253)`: the arrival-order test grows the second Battle's ledgers and sees them apply |
| Claude | The census counts no pose carriers, so nothing guarded the boundary's carrier despawn | verified | same commit: the UDP cycle asserts every authority carrier belongs to the new Battle |
| Claude | `vfx::reconstruct_world` retired ring ids with `World::despawn`, which warns for every id already gone | verified: `World::despawn` logs a warning for a missing entity in bevy_ecs 0.19, and a ring keeps ids past their own end-of-life despawn | same commit: `try_despawn` |
| Claude | The authority's first generation came from a code default that matched `BattleState::default().number` by coincidence; replica compositions init the same default | verified for the authority; not a defect on a production replica, where `world_reset::install_client` inserts `None` over every init | same commit: `server::install_battle` derives it from the first Battle |
| Claude | The authority holds an `Option` it can never empty, checked by five `expect`s | verified | open: an authority-side generation type would carry it |
| Claude | `world_reset::follow_battle` had an unreachable branch whose comment promised more than it kept | verified | same commit: a frame with no Battle forgets the construction, so the next Battle constructs its own |
| Claude | `world_reset::start_next` decides from its own list which environment records exist, while startup decides through each `install_server` | verified: the short-mode probe server gains both ledgers at its first fill-up | open: each `install_server` would register its own boundary constructor |
| Claude | Stale docs: `WallRecord` said its generation moves on a replica's next link; the wall ledger's module doc said one entity holds every change | verified | same commit |

## Fill-up review

Both blind reviews concern the game's match flow and damage model, game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | Loading clears on ownership before an interpolated pose and attached rig | verified: an owned `NetTank` without `Position`, `Rotation`, local `Tank` or bound view sets the old latch | `client: wait for the own tank presentation before leaving loading (#253)`; the gate now requires all of them, and remains latched through results and respawn |
| Codex | The repair test never drives repair-to-brake selection | verified: the old assertions never reached drive-target selection; the missing-repair-arm mutant fails the new assertion on the selector used by `apply_track_forces` | `track: guard repair and phase holds at the drive-target seam (#253)` |
| Claude | No test covers either hold arm at the gameplay-to-brake seam | verified: both missing-arm mutants fail independently; the production selector is shared with the repair test, which asserts repair braking, release after completion and phase braking without a repair | `track: guard repair and phase holds at the drive-target seam (#253)` |
