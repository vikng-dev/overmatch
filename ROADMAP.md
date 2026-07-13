# Deliberately lean roadmap — ideas always shift, this is a general direction

> **Status: historical.** This captured the pre-multiplayer sequence and is preserved as evidence, not current status. Multiplayer is now the product runtime. All quantities below are **DERIVED historical planning assumptions**, not measurements. See [`.agents/PRODUCT.md`](.agents/PRODUCT.md) for the current product target and [`.agents/docs/adr/`](.agents/docs/adr/) for accepted architecture.

Goal: online tank PvP. 10v10 is the aspiration; the design target is 1v1–3v3
(fun must be validated at small player counts — population is the existential
risk for an indie MP game, not server capacity).

## Done

- [x] Single tank operation — suspension/propulsion, cameras, firing, superelevation, shell physics
- [x] Ballistics — armor penetration against ballistic volumes, fragmentation/spall
- [x] Tank vs tank model — armor, components (crew & modules), HP, kill chain
- [x] Control ownership — `Controlled` marker + two-tank Tab swap
- [x] Duel-feel pass — engagement ranges/TTK validated via the two-tank swap
- Track locomotion sandbox parked at `checkpoint/track-model-3-parked` — resume
  only after netcode exists (the server tick budget decides what the track model
  can afford to be; likely client-side dressing over replicated hull state)

## Milestone A: Command layer + fixed-tick sim  ✓ DONE

Hard prerequisite of authoritative MP. Input → serialized `Command` → sim on a
fixed clock → interpolation for render.

- [x] All gameplay input (throttle/steer, fire ×2, aim intention, dialed range,
      crew swap) flows through one serializable per-tank `TankCommand`; the sim
      reads no devices (`command.rs`; edges latched to exactly one tick)
- [x] Sim fully on `FixedUpdate` (servos, damage chain, fire/reload re-clocked;
      physics/driving/shells already were); servo render pose interpolated by
      fixed-clock overstep
- Parked for M5: `drive_aim_servos`/`fire` read render-interpolated
  `GlobalTransform`s (≤1 tick stale) — derive from sim truth when determinism
  matters

## Milestone B: Minimal authoritative multiplayer  ← NEXT

Smallest networked game: two clients, drive/aim/shoot, static graybox terrain,
no destruction. Evaluate lightyear vs bevy_replicon vs hand-rolled when we get
here, not before.

- [ ] Authoritative server, fixed tick, commands up / state down
- [ ] Two players in the same world, full combat loop (shoot, get shot, kill)
- [ ] **First friend playtest the week this works** — this is the real alpha line

## Milestone C: Environment, network-aware

Built once, replicated from day one — destruction is among the hardest things
to replicate; never build it single-player first.

- [ ] Destructible walls (brick wall) replicated authoritatively
- [ ] Tank–wall collision
- [ ] Graybox PvP level: terrain + cover worth fighting over

## Milestone D: Grow the match

Only as far as the design earns it — each step re-validated by playtest.

- [ ] 2v2 / 3v3 (spawn logic, teams, round flow)
- [ ] Second tank variant (maybe — mirror-match is fine for a duel game)
- [ ] Scale toward larger counts if playtests demand it

## Milestone E: Polish

- [ ] Audio, VFX, graphics/textures
- [ ] Pretty screenshots (feeds the Steam page)

## Parallel track: getting it out there (starts NOW, not after polish)

Wishlists compound over time; an MP game needs a community before launch.

- [ ] Devlogs — start early; the physics work (suspension, penetration, spall,
      link-belt tracks) is months of ready-made material
- [ ] Steam page up as soon as a graybox screenshots decently (~Milestone C),
      not after polish
- [ ] Discord for playtest scheduling once Milestone B lands
- [ ] Steam Next Fest with a demo when the loop is fun
- No paid ads — for indie MP, wishlist velocity comes from content, not spend
