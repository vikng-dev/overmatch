# Battle cycle

Subject: Overmatch's match flow and the game's damage model, game state only.
The owner chose the provisional mode in #220; #240 supplies its authority clock.

## Boundary decisions

- The first fill-up is idle until the first authenticated player connects. Once
  started, it runs to its deadline even if that player leaves. Later fill-ups
  start immediately when players remain connected; otherwise they wait too.
- Tanks exist only during combat. At its start, every connected player is queued
  through the automatic map spawn rule, with a fresh life. Late joins use the
  same rule. The operator's harness instruments retain their declared placement.
- Only combat scores. A combatant's standing remains after disconnect, so leaving
  cannot erase its result. Combat runs its full duration regardless of occupancy,
  losses or score. The hosting empty timeout cannot interrupt combat; an expired
  empty timeout can retire the server once combat finishes.
- At results, the server chooses a unique winner, a draw among tied best standings,
  or no contest for an empty board. It removes tanks and flying rounds, including
  detached turrets through the existing root-removal observer. The score freezes.
- The next fill-up increments the Battle number, empties the score and clears
  spawn-map picks. Connected links stay; their combatant identities remain unique
  for the server lifetime and their life counters continue across Battles.
- Battlefield destruction persists for this server's lifetime. This cycle resets
  combatants and score; it does not rebuild terrain, buildings or tree ledgers.
  A fresh battlefield would require its own world-reset contract.

All phase durations are required mode data in `assets/modes/battle.ron`. The
owner's chosen durations are CLAIMED from #220: fill-up 60 s, combat 900 s,
late join 600 s from combat start, results 15 s. Durations round up to authority
whole ticks and fit a wrapped signed deadline comparison. Clients receive
`BattleState` with the phase, exact deadline, Battle number and authoritative
result. They never advance a phase or select the result locally.

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

## Verification

The short-duration mode file `src/net/fixtures/battle-short.ron` drives the same
clock installation and protocol over real loopback UDP without a renderer. The
probe reads the replicated deadlines and score, checks the matching heartbeat
states, observes a rendered typed refusal, and keeps the first player connected
into the next fill-up. A separate challenge-crossing probe closes admission
between the request and response. Score and deadline-wrap tests pin the boundary
invariants. The server spawn tests exercise complete tanks and replacement lives.
