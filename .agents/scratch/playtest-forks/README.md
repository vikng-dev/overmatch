# Playtest forks — decisions only the game can settle

A register of design forks where the *correct* answer can't be reasoned out at the
desk — it depends on how the slice actually **feels in play**. Each fork records the
default we ship now, the alternatives we deliberately kept alive, why it's a playtest
call, and the cost to flip.

This is **not** the issue tracker (`.agents/scratch/<feature>/`, see
`.agents/docs/issue-tracker.md`) and **not** the ADR log (`.agents/docs/adr/`, for
*settled* decisions). It's the home for decisions that are *open by design* — chosen
provisionally, revisited at the controller.

## How to read an entry

- **Default** — what's wired in the code today.
- **Alternatives** — the paths we preserved, with enough detail to actually build them.
- **Why it's a playtest call** — what feel-question decides it; what to watch for in play.
- **Revert cost** — how mechanical the flip is, so a default never silently calcifies
  into an assumption. A high revert cost is a warning to playtest *sooner*.
- **Lives in** — code / design-doc anchors so the fork and its implementation stay linked.

When a fork is settled by play, promote it: write the ADR (or fold it into the design
doc), record the verdict here, and strike it from the live list.

---

## F1 — Crew juggling model (lone-survivor gun operation)

**Status:** open · default chosen + mechanic designed 2026-06-29 · settles in slice 2 playtest

**The question.** When only one combat-crewman survives (e.g. Driver + Gunner alive,
loader and commander dead), how does one human operate a gun that needs both an
aimer/firer *and* a loader?

**Default — hardcore (physical swap, one role at a time).** The survivor physically
moves between positions and can serve only one at a time: aim/fire **or** load, never
both at once. The cost of doing both is *time* (the swap), not a degraded stat. Real
tactical tension — the player gives up one capability to gain another.

**Mechanic (designed 2026-06-29, full spec in design §7b).** Bare **tap-source** (living
crewman) → **tap-target** (station). **Timed transition**: the crewman keeps manning the
source station until the timer completes, then the assignment flips atomically — trade-
with-latency, *no dead window*. Cancellable, not pausable. **Anyone → any** station;
`competence` (1.0 native / 0.6 foreign) is the only gate. The swap is a **1:1 transposition**
(crew ↔ station stays a bijection; the corpse takes the survivor's vacated seat).

**Two sub-knobs that are themselves playtest calls:**
- **Transition cost model** — *source-live latency* (chosen; the survivor keeps the old
  capability during transit) vs *both-ends-dark* (a real sacrifice window). Source-live is
  the gentler dial; flip to both-ends-dark if play feels too cheap. This is the lever that
  decides how harsh hardcore actually feels.
- **Timer duration** — ~3–5s start, tuned live.

Chosen because it's the most physically honest (a human can't occupy two seats), the
cleanest data model (`Occupies` stays strictly 1:1, efficiency stays 1.0 — no
coefficient field needed yet), and it layers: arcade efficiency tiers can be added *on
top* later if the time-cost feels too punishing. Going the other direction (ripping out
dual-occupancy) is harder.

**Alternatives kept alive:**
- **Arcade/1 — dual-occupancy at reduced efficiency.** The survivor occupies both
  positions simultaneously; reload and aim both run slower. Needs an `efficiency < 1.0`
  coefficient on `Occupies` and a rule for how one crewman maps to two positions at once.
  Ships with *no new player input* — its appeal and its danger (invisible stat-fudge,
  against the visualisation-first principle: the player should *see* the crewman move).
- **Arcade/2 — remote load from the gunner's seat at reduced efficiency.** Loading is
  performed without a physical swap. Needs the `provides` override deferred in slice 1
  (a position grants a role it isn't natively wired for).

**Why it's a playtest call.** Whether the swap *time-cost* reads as engaging tension or
as punishing dead-time can't be known until the swap is in hand and timed. Watch:
does the lone-survivor endgame feel like a desperate juggle (good) or like the tank is
just broken (bad)? If the latter, the arcade tiers are the pressure-release.

**Revert cost.** Low → medium. Hardcore is the smallest model; arcade is *additive*
(introduce the efficiency coefficient and/or the `provides` override on top). No teardown
of hardcore is required to trial an arcade tier.

**Lives in.** Design §7b (backfill + capability model v2 + the swap mechanic). Code:
the `Occupies` relationship and the capability-effectiveness evaluator in `src/damage.rs`
(slice 2 — renamed from slice-1's `action_available`/`TankActions`).

---

## F2 — Kill threshold (when is a tank knocked out)

**Status:** resolved 2026-11-01 by owner decision (#227) · B ships, A deleted

**The rule.** A tank is knocked out when fewer than two of its crew members are in action
(`damage::knockout_from_counts`, latched by `mark_dead_tanks`). A cook-off is a cause, not a
second rule: it empties every crew HP pool of its tank, so the crew rule knocks the tank out and
names the cook-off. Modules decide capabilities only: a tank with every module out of action
stays in the Battle while two crew members are in action. `TankKnockedOut` stays a label (scoring
and the Battle read it); no capability is gated on it.

**The losing arm.** A, knocked out only at zero crew in action so a lone survivor could still
staff one position (the F1 backfill state), is deleted from the code and from the crew sweep. A
tank seating fewer than two crew stations is knocked out at spawn under this rule; every shipped
tank seats more.

**Lives in.** `knockout_from_counts` and `mark_dead_tanks` in `src/damage.rs`; the crew sweep
(`src/ballistics/crew_sweep.rs`) measures shots to knockout under it.

---

## F3 — Combat damage disclosure

**Status:** open · authority semantics settled, presentation deliberately temporary

**The question.** How much additional information should a shooter receive after the authority confirms enemy damage? The spectrum runs from no cue beyond visible world damage to an exact internal X-ray account.

**Default — explicit authoritative confirmation with temporary presentation.** The player's firing action is predicted immediately, but damage is never predicted as fact. The current hit-marker-like cue is a disposable view of authoritative damage, not the product target and not the semantic interface.

**Alternatives kept alive:**
- **World evidence only.** No additional indication beyond visible damage, fire, smoke, lost capability, and enemy behavior.
- **Restrained confirmation.** A subtle sound, crew callout, sight response, or other low-information acknowledgement that damage occurred.
- **Detailed disclosure.** Increasingly precise penetration, crew, tank-module, or X-ray information, up to a complete internal account.

The server owns disclosure. A live client must receive only the detail the current rule permits; hiding privileged truth in the UI is not concealment from a modified client. Armor inspection and scientific diagnostics may use a separate privileged adapter.

**Direction chosen 2026-10-10 (not yet wired).** One frame per verdict: a ricochet shows the deflection from outside, a stopped round the plate slice, a penetration the see-through path and clouds with each box tinted by the dose it took and no state shown live; the full record with its consequences after the match. "Detailed disclosure" stays the alternative as live state tint in the same frame; the authority holds the consequences either way, so the flip is a delivery-policy change on the same record. Decision: `.agents/scratch/damage-cam-2026-10-08/product/in-battle/owner-direction-2026-10-10.md`, "Corner call, by verdict". Queued beside it, to register when the burst integrator lands: whether boxes shadow a bursting round's cloud (default yes; alternative plates only).

**Why it's a playtest call.** The correct point depends on whether feedback improves comprehension and learning or erodes observation, uncertainty, and tank knowledge. It cannot be settled from implementation convenience.

**Revert cost.** Expected to be low for presentation once the confirmation seam is complete: the required interface is a discrete, shot-attributed authoritative damage fact, deduplicated by `ShotId`, rather than a cue inferred from replicated health snapshots. Changing how much detail the server discloses may widen or narrow that semantic fact, but no presentation belongs in ballistics or damage truth.

**Lives in.** `src/net/hit_feel.rs`, the authoritative damage/outcome path in
`src/net/protocol.rs` and `src/net/shot_transport.rs`, and the future armor-inspection presentation.

---

## F4 — Remote automatic-fire continuity

**Status:** open · bounded default implemented 2026-07-13 · settle with impaired-network playtests

**The question.** How much loss or thinning in another tank's automatic-fire presentation remains
readable before cadence, direction, or cause-and-effect feels broken?

**Default — every authored automatic fact is eligible for bounded repair.** Fire, ricochet, and
terminal visuals receive up to three unordered-unreliable admission opportunities and expire after
16 authority ticks. Both values are **DERIVED STARTING DEFAULTS**. Admission pressure may permit
fewer opportunities; instrumentation distinguishes expiry before the first from expiry after a
partial repair. The receiver deduplicates by `ShotId`; missing visuals never change authority or
owner-private damage confirmation. Without contention, the present spacing is the emission tick and
the next two authority ticks—a **DERIVED 31.25 ms** first-to-last span at 64 Hz. This is not yet a
claim of resilience to correlated loss.

**Alternatives kept alive:** fewer copies under contention; longer or shorter presentation expiry;
distance/visibility-aware tracer density that never hides a gameplay-visible tank; or a compact
firing-generation/cadence state for recovery after correlated loss. Reliable per-round automatic
traffic is deliberately not an alternative because stale retransmit debt scales with the burst.

**Why it's a playtest call.** Independent-loss arithmetic cannot tell us whether real correlated
loss looks like an acceptable hole in a burst or a misleading silence. Test at normal and adverse
RTT/loss while several tanks fire, watching remote muzzle cadence, post-bounce continuation, late
visuals, and whether confirmation still feels causally attached to the shot. **MEASURED on local
loopback 2026-07-14:** all three server-traced copy opportunities fell inside a six-receive-update
loss window; the covered automatic shot produced no presentation, while the following shot recovered
exactly once. That establishes the current correlated-loss hole; it does not decide whether the hole
is perceptually acceptable.

**Revert cost.** Low for copy count, expiry, and batch admission; medium for a firing-generation
state because that widens the wire contract and join-in-progress model.

**Lives in.** `src/net/shot_transport.rs`, `src/net/client.rs`, `src/net/shot_loss.rs`, and
ADR-0021.

---

## F5 — Spall cone half-angle and axis

**Status:** half-angle settled 2026-10-23 (owner): measured, per plate material · axis open,
default chosen 2026-10-12 (owner) · settle the axis in the damage feel pass

**The question.** How wide is the spall cone thrown from a body exit, and where does it point
behind an oblique plate? Together they decide which boxes behind a perforation take a dose, and how
far off the shot line a crewman is safe.

**Settled — the half-angle is the exit material's, measured.** A cone's half-angle is its exit
layer's substance's `debris_half_angle_deg` (`assets/materials/materials.ron`), the rim of the ball
law (F6). RHA's 26.2° is the constant-angle fit of the MEASURED Qi et al. 2026 normal-impact debris
extents (shots 5 and 6; `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/shape-law.md`
§2.1–2.2); those are 97 % contours, so the ball's rim may sit up to about 2° wider. Every other
hard substance authors RHA's value as Unknown until its own extent is measured. The 25° that stood
before it was chosen at random, and the fixed-angle alternatives (20°, 29°, 35°) retire with it.

**Default — axis = the bisector.** The axis is `normalize(rear-face outward normal + exit
direction)` at a genuine exit and the exit direction at an internal step. The bisector is an
ENGINEERING ASSUMPTION with MEASURED support (Verolme 1997; Recht & Howell 1967). Evidence:
`.agents/scratch/damage-cam-2026-10-08/research/cloud-angle-deep.md` and
`research/codex-cloud-angle/verdict.md`.

**Alternatives kept alive:**
- **The exit-direction axis** (`ExitDirection`) — the cone as it stood before 2026-10-12: straight
  along the round's path behind an oblique plate.

**Why it's a playtest call.** The measurements do not fix the axis at obliquity in the game's
regime. Watch whether a sloped plate's spall goes where a player expects, and whether crew a little
off the shot line are knocked out too often or too rarely.

**Revert cost.** Low — one data edit: `axis` (`RearNormalBisector` / `ExitDirection`) in
`assets/ballistics/spall_cone.ron`. Box counts may then want the feel pass again.

**Lives in.** Design §5 (`.agents/docs/design/armor-penetration-and-damage.md`). Code:
`src/ballistics/spall_cone.rs` (shape, axis rule, directions) and `throw_spall_cone` in
`src/ballistics.rs`.

## F6 — Spall field profile and passage

**Status:** settled · profile 2026-10-23 (owner): the ball law · passage retired 2026-10-26 (Yan):
full shadowing is the only law

**The question.** How is a spall cone's strength spread across its angle, and what happens when a
direction meets material? Together they decide how much a box behind a perforation takes, and
whether internals shield what lies behind them. Crew are witnesses: dosed by every direction that
enters them short of where it stops, never stopping one (ADR-0040).

**Settled — the ball law.** Sideways debris speeds spread evenly through a ball, from zero to the
greatest, give a cone brightest on its axis and fading to nothing at its rim
(`.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/shape-law.md` §1.2): direction
`k` carries `S·f_k`, about 0.84 S on the axis and 0.05 S at the outermost of 64 strata of RHA's
26.2° cone (DERIVED), and the cone still emits S/2 in all. Owner decisions 2026-10-23, night
(`research/ap-aphe-2026-10-23/decision-2026-10-23.md`); the Flat/Linear dial retired with it, and
another profile is another field law (`FieldLaw`), a code and wire change. The half-angle and axis
are F5.

**Settled — full shadowing.** A direction stops at the first hard body it enters: an armour face
(no dose) or a hard HP-bearing box (that box takes its whole strength). The default from 2026-10-17
(owner), on the reasoning that fragments pushing through material are nuance the player can never
control, so the game stays consistent and learnable without them. Yan retired the `passage` dial on
2026-10-26: full shadowing is the only law, with no key in `assets/ballistics/spall_cone.ron`, and
another passage is another law, a code change.

**Retired.**
- **`passage: Partial`** — a direction spent `chord × resistance` through every hard body and dosed
  each box it entered at its entry strength, then continued. Graded shadowing (a plate stops it, an
  engine mostly), resting on material data two values of which are Unknown.
- **Absorbed dose** (never built) — like Partial, but a box takes only the strength it spends
  inside it, so dose is conserved and materials still matter. Another passage law, a code change.

**Revert cost.** One commit: reverting `damage model: a field direction stops at the first hard
body it enters; the partial passage is retired` restores the `Partial` passage, the `passage` key
and their tests. Records and box-count tuning would want the feel pass again.

**Lives in.** Design §5 (`.agents/docs/design/armor-penetration-and-damage.md`). Code:
`src/ballistics/spall_cone.rs` and `field_direction` in `src/ballistics/resolve.rs`. Captures of
the even profile the ball replaced: `.agents/scratch/damage-cam-2026-10-08/demo/capture/looktest/field-final.*`.

## F7 — Burst shape: the ring or the ball

**Status:** settled 2026-10-27 (owner, epic #103 §5): the ring · the burst's ball retired from
the data and the code 2026-10-28 (#115)

**The question.** In the game's damage model, a burst round's fuze arms in a thick enough plate,
runs its delay from that body's exit and bursts, throwing a field from the burst point about its
flight direction. How does that field spread? It decides which crew stations and modules a
penetration with a burst knocks out, and whether the shot's line goes on past the burst point.

**Settled — the ring.** Every burst round bursts as the ring. The wall around the burst core
becomes a band about the flight axis, centred `atan(v_s / v_c)` off it (`v_c` the carried speed,
`v_s` the wall's greatest sideways speed, 541.5 m/s on the 8.8 cm Pzgr. 39, DERIVED), half-width
10° (TUNED, provisional); the nose piece (8.370 kg, DERIVED) flies on along the line as an inert
round of its own. A round's burst block names no shape, and the ring's own law (its width, its
potency) stays provisional data, outside this fork.

**Retired.**
- **The ball** — the round less its core driven as one lobe at `λ = v_c / v_s` (`v_s` 205.8 m/s,
  DERIVED): a forward cone above `λ = 1`, the whole sphere below, brightest on the axis; nothing
  flew on. Its driven-mass energy was about the ring's (214.8 against 211.1 kJ, DERIVED), so the
  two differed in where the burst's potency went, not how much there was. Its round
  (`88mm_APHE_ball`), its `ball_directions` count, its scenario record (s14) and the record's
  burst end (`RoundEnd::Burst`, `WireEnd::Burst`) left with it. The ball law itself stays: every
  plate exit's spall cone is thrown with it (F6).

**Why it was a playtest call.** The comparison the owner asked for: whether a band that reaches the
stations beside the line, with the nose piece continuing down it, reads better than a lobe that
drives forward and leaves nothing behind it; and whether either knocks out whole crews too often
from one penetration.

**Revert cost.** One commit: reverting `damage model: every burst is a ring; the burst's ball
retires and burst facts are an optional substance field` restores the ball's round, its
`option`, its direction count, its record and the burst end on the wire (a protocol revision).

**Lives in.** Design §5a, "The burst" (`.agents/docs/design/armor-penetration-and-damage.md`).
Code: `src/ballistics/burst.rs` (the fuze and the derived parts), `throw_burst` and `burst_here` in
`src/ballistics.rs`, `BurstFields` in `src/ballistics/spall_cone.rs`.

## F8 — A fallen tree's rest: scripted pose or simulated body

**Status:** open · default built 2026-10-26 (slice 1), its ground rest landed 2026-10-27 (#140) ·
settles when trees must rest on tanks, other trees or buildings · the alternative is the stated
destination (owner, 2026-10-27), the default is provisional

**The question.** In the game's destruction model, what is authoritative about a fallen tree's
rest: a pose every peer computes from the state word, or a settle the server simulates and
replicates? It decides what a lying trunk can rest on, what the wire carries per fallen tree, and
whether a fall can land on a tank.

**Default — scripted.** The ledger word carries the cut segment and the fall's azimuth bin;
every peer derives the rest pose as a pure function of the word, the body and the terrain
(libm-only trig, one fixed order, the same bits everywhere): hinge about the cut to the first
terrain contact, then the butt slides off the stump and the piece lies along the ground (#140).
Nothing on the wire beyond the word; late join is the word; no settle window. A trunk rests on the
terrain only and bridges a dip under it.

**Alternative kept alive — the rigid body (the destination).** The cut piece becomes a dynamic
body on the server with its segment colliders, falls under gravity, meets the stump, other trees,
walls and tanks, settles and freezes to static; its transform is replicated like a hull's and
interpolated on every client (ADR-0037: one timeline, so no peer reproduces the fall and none
needs to). The ledger word stays for the HP pool and the lying segments; the pose comes from the
transform. A tree can land on a tank, which resolves on the authority like every contact.

**Why it's a playtest call.** Whether the scripted trunk reads false in play — bridging a dip,
lying through a tank, never leaning — before the interaction layer is otherwise wanted; and
whether a fall seen at interpolation delay reads fine. Watch where lying trunks look wrong.

**Revert cost.** Medium — code, no data: the rest rule (`resting_tilt`, the lying branch of
`derive_state`) is replaced by a body split at the cut, replication of its transform, a freeze on
sleep, and the view bound to the transform instead of the scripted turn; the ledger word and the
tree assets are untouched.

**Lives in.** Code: `src/trees.rs` (`resting_tilt`, `derive_state`, `fall_rotation`),
`src/trees/view.rs` (`present_fall`), `src/net/destruction.rs`. Tracker: #121 (the resolution),
#140 (the scripted rework), epic #106.

## F9 — A house's walls: a hull obstacle or a climbable block

**Status:** open · default built 2026-10-30 (#182, slice 2 of building destruction) · settles when
tanks drive at houses in play, and before the breach law (B2) lands

Subject: Overmatch's environment destruction model, game state only.

**The question.** How does a tank's running gear meet a house's intact wall: as a hull collider
only, or as a block the track field senses and the belts can climb? It decides whether a tank
noses into a wall and stops, or rides up onto it like a ramp, and whether the inside of a hollow
house is ground a tank can stand on.

**Default — hull colliders only.** A house is its elements (piers, sills, lintels, gables, wall
plates, roof planes), each a static convex collider on `[Terrain, Armor]` with a `BallisticVolume`;
none enters the `TerrainMap` block list, so the analytic track field never senses a wall and the
floor inside is the terrain. The hull's own collision is the interaction, as a tree trunk's is.
Spawn avoidance keeps refusing a spawn inside a house through each house's bounding box
(`world::SpawnObstacles`), outside the track field.

**Alternative kept alive — the walls as blocks.** Each element also enters `TerrainMap` as its
own posed box (a pier, a sill and a lintel are boxes already; a gable and a roof plane would want
their bounding box, or stay hull-only), so the belts sense a wall and climb its top edge where the
hull allows, the way they climb the authored course's slabs. The graybox did this with the whole
house as one block, which made a house a plateau a tank could mount.

**Why it's a playtest call.** Whether a tank meeting a brick wall reads right when it stops hard
(hull) or when its tracks start to ride up the wall before the hull meets it (block). Watch for
tanks wedged against a wall with no way back, and for tracks visibly sinking into a sill or
climbing a 2.5 m wall.

**Revert cost.** Low: the scatter's structure arm pushes each element's box into the block list
(code only; the building RON already carries every element's prism), and spawn avoidance then
reads them there.

**Lives in.** Code: `src/scatter.rs` (the structure arm), `src/buildings.rs` (`spawn`),
`src/world.rs` (`SpawnObstacles`), `src/net/server.rs` (`resolve_free_spawn_xz`). Tracker: #182,
epic #106.

## F10 — A broken wall's pieces: when they freeze, what wakes them, how many move

**Status:** open · default built 2026-10-30 (#188, walls break in game) · settles when tanks fight
through broken houses in play

Subject: Overmatch's environment destruction model, game state only.

**The question.** A loose group of a house's chunks falls on the server as one body, brick in
ballistics, its pose replicated. How long does such a piece stay a body the solver moves, what
turns it back into one once it froze, and how many may move at once? It decides whether rubble
reads as a heap a tank shoves or as fixed blocks, and what a breach costs the server per tick.

**Default — the stress harness's rule, made safe** (`src/debris.rs`, shared by the game's pieces
and the debris stress lever):

- A piece whose speed and spin stay under 0.15 m/s and 0.15 rad/s for 0.5 s, touching something
  and no dynamic body but other pieces, freezes to a static body (ASSUMED: avian 0.7's sleep
  defaults), remembering what it rests on.
- Past 256 live pieces (ASSUMED), the oldest pieces that rest now freeze early. The cap is kept by
  freezing only resting pieces, never by admission: a moving or unsupported piece never freezes,
  so the cap bounds the live pieces once the moving ones settle, not while they fall.
- A frozen piece wakes when what it rests on goes (a chunk turned to dust or fallen, an element
  swapped to its chunks) or moves, and before the physics step when a moving body other than a
  piece is about to reach it, so a tank's first contact pushes rubble rather than meeting a fixed
  block.
- Rubble stays all match: nothing despawns a piece.

There is no runtime switch: the numbers are constants of the rule.

**Alternatives kept alive.**
- **Admission control:** past the cap, a newly freed group becomes a body only once a live slot
  frees, holding where it stood until then, so the cap holds at every instant. Costs a group
  hanging unsupported for that wait, the static support in mid-air the default refuses.
- **Coarse rubble:** a frozen piece is replaced by a coarser static shape (its group's hull) to
  cut static proxies, keeping its mass and occupied space.
- **Fading fine rubble:** small pieces despawn after a while; large ones stay. Costs gameplay
  cover and line-of-sight changes.
- **Avian sleeping instead of freezing:** rejected for now (module doc of `src/debris.rs`): it
  sleeps whole islands, so neither the rule nor the cap can freeze one piece.

**Why it's a playtest call.** Whether frozen rubble reads as solid ground or as props tanks clip
through or snag on; whether a tank shoving a heap looks right; whether a big breach's settle time
and server cost are acceptable at 30 tanks. Watch for pieces that hang, jitter on a tank, or
stall a tank as if welded.

**Revert cost.** Low: the four constants and the cap's resting test are in `src/debris.rs`; an
admission cap is a queue in front of `buildings::breakage::spawn_piece`. No wire change for any
of them.

**Lives in.** Code: `src/debris.rs` (the rule), `src/buildings/breakage.rs` (`spawn_piece`),
`src/net/debris_stress.rs` (the carriers). Tracker: #188, epic #106; the supported-settlement
design in #189.

---

## F11 — The first Battle's combatants and capacity

**Status:** open · owner default chosen in #220 · mode and admission wired

Subject: Overmatch's match flow and the game's damage model, game state only. F11–F16
record #220's provisional mode decisions; numeric defaults attributed to that issue
are **CLAIMED owner decisions**, not physical measurements. Alternatives are prose,
not runtime switches.

**Default — free-for-all, no automatic bots, capacity 30 (CLAIMED, #220).** There are
no teams or balancing assignments. Development bots remain an explicit operator
instrument; early-access hosting does not enable them. The Worker supplies capacity,
and the hosted server requires it explicitly.

**Alternatives kept alive.** Balanced teams with assignment at admission; other team
splits; bots filling empty slots; a different configured capacity. A queue that forms
filled or balanced lobbies can replace sparse-server admission later.

**Why it's a playtest call.** Watch whether sparse free-for-all Battles are engaging,
whether new players find opponents, and whether the chosen concurrency feels crowded.
A guaranteed player population cannot be assumed for early access.

**Revert cost.** Low for capacity data; medium for teams or automatic slot-filling,
which need explicit assignment and score policy rather than a hidden flag.

**Lives in.** `backend/wrangler.jsonc`, `src/net/hosting.rs`, `src/net/server.rs`;
tracker #220 and #239.

## F12 — The Battle cycle and late admission

**Status:** open · owner durations chosen in #220 · clock and replicated phase wired

**Default — a recurring Battle, with players staying connected.** CLAIMED from #220:
fill-up lasts 60 s from the first player, combat lasts its full 15 min even alone,
late join closes 10 min into combat, results last 15 s, then another fill-up begins.
An empty initial fill-up waits; an occupied next fill-up starts its timer immediately.
The authority publishes phase, deadlines and result. Results stay closed to joins.
An in-flight challenge is refused if admission closes before its response arrives;
a new connection is refused after closure even if it previously played here.

The implementation's boundary policy is provisional too: fresh tanks at combat
start, no tanks or flying rounds in results or fill-up, score frozen through results
and reset at the next fill-up. Battlefield destruction persists for the server's
lifetime; the cycle does not rebuild the battlefield.

**Alternatives kept alive.** Shorter or longer phases and admission windows; filled
or balanced matchmaking lobbies instead of timed fill-up; a fresh battlefield each
Battle; a different reconnect policy after closure. Other modes can choose a victory
condition that ends combat early rather than retaining this mode's full timer.

**Why it's a playtest call.** Watch the solo wait, how much useful combat a late
arrival gets, whether results linger, and whether persistent destruction gives the
next Battle useful history or an exhausted battlefield.

**Revert cost.** Low for duration data. Medium for admission policy; high for replacing
fill-up with queued matchmaking or resetting world ledgers, colliders and their replicas.

**Lives in.** `assets/modes/battle.ron`, `src/net/battle.rs`, `src/net/server.rs`,
`src/net/admission.rs`, `src/net/hosting.rs`; boundary contract in
`.agents/docs/design/battle-cycle.md`; tracker #240.

## F13 — Unlimited respawn with a wait

**Status:** open · owner default chosen in #220 · scored opening wired for all combatants

**Default — unlimited respawns after 10 s (CLAIMED, #220).** The game's damage model:
a knocked-out life has one authority-tick opening. Players request another life;
development bots use the same mode wait. Respawning cannot change the retained score.

**Alternatives kept alive.** A finite life budget, one life per Battle, or a different
wait. Those remain prose alternatives; no unused life-budget branch remains in code.

**Why it's a playtest call.** Watch whether re-entry sustains a full-length Battle or
makes a knockout inconsequential, and whether the wait is useful breathing room or
idle time.

**Revert cost.** Low for the duration; medium for a finite budget, which needs a new
explicit admission state and corresponding server refusal and player readout.

**Lives in.** `assets/modes/battle.ron`, `src/net/score.rs`, `src/net/server.rs`,
`src/net/death_screen.rs`; tracker #241.

## F14 — Winning by knockouts, with losses breaking ties

**Status:** open · owner default chosen in #220 · ranking and authoritative result wired

**Default — most knockouts, then fewer losses, then a draw.** The game's damage model:
only combat scores. There are no survival points; the owner rejected rewarding hiding.
A disconnected combatant's standing remains eligible for the result.

**Alternatives kept alive.** Objective or capture scores, a score threshold, last
combatant or team standing, or different tie-breakers. Survival scoring remains a
rejected alternative to revisit only if play changes the owner's reason for rejecting it.

**Why it's a playtest call.** Watch whether the ranking rewards active play and whether
losses are a satisfying tie-breaker. Record draws and cases where the table's winner
feels different from the Battle players experienced.

**Revert cost.** Low for ranking and tie-breaking; medium for objective state or a
victory trigger that changes the cycle's full-duration promise.

**Lives in.** `src/net/score.rs` (`rank`, `Scoreboard::outcome`), `src/net/battle.rs`;
tracker #241 and #240. The tank-knockout rule itself remains #227's concern.

## F15 — Automatic fixed-point spawns, without protection

**Status:** open · owner default chosen in #220 · automatic selection wired

**Default — authored points selected by nearest-live-tank distance.** CLAIMED from
#220: choose the point closest to a target distance of about 400 m, never inside
200 m; alone, choose a random authored point. There is no spawn protection. Distances
are mode data and points are map data. Existing spawn-map picks and harness placement
remain named instruments. Automatic placement retries congestion without relaxing the
minimum; the next combat starts from a fresh combatant layout.

**Alternatives kept alive.** Team bases or zones, different distance tuning, different
candidate layouts, player-selected placement, or temporary spawn protection. Each
would need its own explicit rule; a crowded map does not silently enable one.

**Why it's a playtest call.** Watch immediate re-entry pressure, time spent seeking an
opponent, and waiting when players block candidates. A mathematically valid distance
can still feel too close or too remote.

**Revert cost.** Low for distances or map points; medium for zones, bases or protection,
which change the server's placement or combat-entry contract.

**Lives in.** `assets/maps/kalinovo/level.json`, `assets/modes/battle.ron`,
`src/net/spawn_placement.rs`, `src/net/server.rs`; tracker #242.

## F16 — A small held scoreboard and the same table at results

**Status:** open · owner presentation chosen in #220 · server data wired, UI queued in #243

**Default — Tab holds rank, name, knockouts, losses and time left; results reuse that
table and mark the winner.** The game's damage model: no other figures are requested.
The authority holds the standings, phase, deadline and winner; the UI reads them.

**Alternatives kept alive.** A persistent HUD table, a different results layout,
additional authoritative figures, or a different binding and information density.

**Why it's a playtest call.** Watch whether players can read their standing and the
remaining time without losing sight of the battlefield, and whether the result explains
who won without inviting irrelevant statistics.

**Revert cost.** Low for layout or binding once #243 exists; medium if new figures
need a wider authority-owned wire contract.

**Lives in.** `src/net/score.rs`, `src/net/battle.rs`; presentation tracker #243.
