# Overmatch

A realistic, official-server-hosted online PvP tank game (Bevy/Rust).
This file is the project glossary — terms only. Decisions live in `.agents/docs/adr/`.

## Product loop

**Battle**:
A finite, server-authoritative contest governed by a game mode and completed when its victory condition produces a winner.
_Avoid_: match, game (when referring to the contest itself)

**Game mode**:
The policy governing a Battle's teams, admission, spawn and respawn rules, eligible content, and victory condition.

**Garage**:
The persistent player context between Battles, where tanks and crews are configured and Progression is presented.

**Progression**:
Durable player advancement across Battles: unlocked tanks, tank improvements, crew development, currency, and tech-tree advancement.

**Damage confirmation**:
An authority-issued fact that the player's action damaged an enemy. Its presentation and level of disclosed detail are deliberately unsettled.
_Avoid_: hit marker (that names one disposable presentation)

## Battlefield

**Battlefield destruction**:
An authority-owned change to a placed world object that alters gameplay during a Battle, such as a fallen tree or breached wall.

**Surface evidence**:
Visual-only traces of action on an otherwise simulation-static surface, such as track marks, impact scars, scorch, dust, and shallow visual craters.
_Avoid_: terrain deformation (surface evidence does not change ground collision or traversal)

## Aiming

**Sight** (reticle):
The camera's view direction, marked by the fixed dot at screen center. Where the player is *looking*.
_Avoid_: crosshair, cursor

**Aim point**:
The ground point the gun is *commanded* to hit, resolved from the camera's screen-center ray. Intent — where we've told the gun to go, not where it actually points. Held in the hull's local frame, so a hold is a bearing off the tank (ADR-0001); carried on the wire tagged with the frame its authoring view is anchored to, so no latency can rotate it out from under that view (ADR-0038).
_Avoid_: target, aim target

**Bore axis**:
The line straight down the barrel (the muzzle's forward direction); shells depart along it.
_Avoid_: gun line, muzzle direction

**Bore point**:
Where the bore axis currently meets the ground; what the green bore indicator marks. The gun's *reality*, as opposed to the aim point's intent.
_Avoid_: bore aim point

**Target**:
A designated thing to engage (a locked-on or selected enemy). Reserved for future designation; not yet implemented. Never use it for the commanded ground point — that is the aim point.
_Avoid_: using "target" for the aim point

## Tank rig

**Rig contract**:
The set of nodes the model must provide for code to bind behaviour to. Only `Hull` and `Center_Of_Mass` are fixed-name singletons; the variable parts (servos, weapons, ballistic volumes, view anchors) are **declared in the per-variant RON, keyed by node name**, and the binder iterates that spec to resolve them (ADR-0012). Plus at least one collision proxy and one roadwheel per side. Absence — a declared node with no matching model node, or a missing fixed node — is a fatal authoring error caught at bind, not a runtime condition.

**Hull**:
The tank body — the chassis the turret sits on, and the frame all aim math is computed relative to.

**Turret**:
The rotating top; yaws to bear on the aim point.

**Gun**:
The gun mount — the elevation pivot and the (stationary) mantlet. Elevates in pitch.
_Avoid_: barrel (that is a separate, recoiling node)

**Gun barrel**:
The recoiling barrel — child of the Gun, parent of the Muzzle. Slides under recoil while the Gun mount stays put.

**Muzzle**:
The barrel's tip. Its forward is the bore axis; shells spawn here.

## Sim / view

**Sim body** (sim skeleton):
The tank's simulated entities — servo frames, wheel stations, colliders, armor volumes, carried `TankSim` state — built synchronously at spawn from extracted data (`TankGeometry`), never from the glb scene. This is what the server and predicted client run on; it is rollback-registered and complete the tick the tank spawns (ADR-0014).
_Avoid_: "the rig" for the sim body (the rig is the *contract*; the sim body is the spawned entities)

**View**:
The instantiated glb scene, attached onto the sim body as pure presentation whenever it loads. It only renders — no sim state reads or lives on it. A view node mirrors a sim part by name (`ViewOf` / `ViewNode`); render smoothing writes view nodes, and the sim transforms stay pure per-tick truth (ADR-0014).
_Avoid_: calling the view "the model" (that is the source `.glb`/`.blend`)

## Geometry LOD

(Model: geometry mipmapping — a mip chain for meshes. Decisions in the LOD ADR.)

**Source** (L0):
The artist's mesh as it sits in the .blend — level 0 of every chain, deviation zero *by definition, not by merit* (every mesh has finite geometric resolution). Never generated, never derived, never a manual step.
_Avoid_: base model, raw model (the source IS the shipped L0)

**Source hygiene** (re-encode):
An artist-side rewrite of the source at its true information content (the 5,550-tri link collapsed to ~1.5k), done in the .blend, by a human, once — the result *becomes* the source. Authoring, not LOD generation: the mesh analogue of not shipping an 8K texture on a 13 cm prop.
_Avoid_: LOD0 generation, decimation preset

**Deviation**:
Worst-case point-to-surface distance (mm) between a generated level and the source — the biggest lie the level tells about where the surface is. Measured **two-way** (one-way misses holes); never an average (averages hide the spike that pops). Travels with the level as `worst_dev_mm`.
_Avoid_: error percentage, triangle reduction (those are inputs/outputs, not the lie)

**Normal deviation**:
Worst-case angular error (degrees) of the interpolated shading normal, two-way sampled. Positional deviation is blind to it, and a wrongly-lit pixel is visible over its triangle's whole projected *area*, not its sub-pixel displacement — so it gets its own numeric gate (with UV drift and tangent validity) instead of a human eyeball.

**Pixel budget**:
How many pixels of positional lie the current view tolerates. 1 px = sub-pixel = positionally indistinguishable from the source in that view. Per-view (optic tighter than commander), and a player-facing setting whose honest top rung is "indistinguishable from the raw model".

**Sub-pixel distance**:
Where a deviation drops below budget: `D = dev_m × height_px / (2·tan(vfov/2) × budget_px) + r`. EXACT, not small-angle — the `vfov_rad` shortcut is 0.06 % out at the optic and 5.5 % at the commander fov, and `r` is the asset's origin radius (`VisibilityRange` measures to the ORIGIN, the guarantee is about the surface). Worked number: 27.842 mm through the 0.12 rad optic at 2160 px, 1 px budget, +0.4 m → 500.95 m (the shipped shoe's L3). A test re-derives every wired threshold from the manifest and prints the metres to write on mismatch.

**Exact-world radius** (D₁) / **first lie** (e₁):
The one declared constant of the system, seen from its two ends: within D₁ every surface renders exactly (source); beyond it, levels lie by at most the budget. e₁ is view-invariant mm; D₁ is its sub-pixel distance in the reference view (350 m optic ≈ 26 m commander, same e₁).
_Avoid_: LOD bias, detail distance (engine-flavored words for other mechanisms)

**Error ladder**:
e_N = e₁ × 2^(N−1): each level doubles the allowed lie, doubling its switch distance (clean octaves) and roughly halving its triangles (tris ∝ 1/deviation on curved surfaces — measured 2.4× dev ⇒ 2.0× fewer tris). Triangle counts are OUTPUTS of the decimator hitting targets, never inputs. Full-chain storage converges to ≈ +100% of source — kilobytes; never a design driver.

**The two walls**:
The octaves a ladder can claim are bounded by the *left wall* — the source's intrinsic detail scale, the smallest deviation shedding ~half the triangles; targets below it emit near-copies (pure waste) — and the *right wall* — the map DIAGONAL (not its radius: the far corner is what a camera in the near corner can still see), past which a level never renders and is never generated. Optimal D₁ sits at the left wall, measured once game-wide; bigger maps automatically earn deeper ladders.

**Magnification**:
(This ladder's sense. An optic's magnification — the `×` a view authors, `spec::Optics` — is a different quantity entirely.)
What happens closer than a level's band, including closer than the source's own resolution: nothing swaps in, the asset shows its finite detail — exactly as a texture goes soft inside mip 0. Not a defect. (Escape hatch if an eyeball ever objects: an additive near band rendering the pre-hygiene authored mesh — one chain row, parked.)

**What sub-pixel does NOT cover**:
The guarantee is positional only. Silhouette IS covered (silhouette error is surface deviation). Separately gated because deviation cannot see them: normal deviation, UV drift, defaulted tangents, degenerate faces, holes. The red-test class lived here and is now CLOSED — the shipped shoe's one defaulted-tangent vertex sat on a 50.16 mm edge drawing 16.2 budget pixels of wrong shading at L1's own 55.9 m switch, while passing every positional check AND the manifest's own `tangent_default_verts: 0` (which counts zero-UV-AREA faces, and that face was not one: a source-data gate is not a gate on what the runtime's solver returns). Fixed by BOTH halves at once — needle cleanup removed the sliver (855/581/315 -> 854/580/314) and the exporter now BAKES tangents, so the runtime generates none and the certified bytes are the rendered bytes. The test stays, green, as the standing regression gate.

**Build** (the one command):
`scripts/tank/build.py` — one command per tank, a `.blend` in and three shipped artifacts out (view glb, sim artifact, certificate); the asset door is its certification step.
_Avoid_: export, generate (those are stages inside it, not the command)

**Certificate** (`<id>.lod.json`):
The per-tank record of what was measured — `blend_digest`, `view_glb_sha` + `sim_glb_sha`, `mesh_count`, and per source primitive a bounding `radius_m` with ordered `rungs[{mesh, deviation_mm}]`. Carries no metre distances: the runtime derives those from certified deviation and the active view profile.
_Avoid_: manifest (the retired global `lod_manifest.json` was one; a certificate is per tank)

**Sim artifact** (`<id>.sim.glb`):
A byte-strip of the certified view glb — LOD0 geometry and material names, no textures, no UVs, no rungs — so server and client walk identical accessor bytes by construction.
_Avoid_: collision mesh, physics glb (it is the same bytes as the view artifact's LOD0, not a second authoring)

## Gunnery

**Servo**:
A 1-DOF *kinematic* rotational motor with a trapezoidal motion profile, slewing turret yaw / gun pitch toward a commanded angle. Not a physics joint — we drive it ourselves.

**Recoil**:
The barrel's rearward kick on firing and its damped spring back to battery — a 1-DOF translational motor, the bore-axis cousin of the Servo.

**Battery**:
The barrel's rest (fully forward) position, to which recoil returns. "Return to battery."

**Weapon gate**:
The complete authority-owned state that decides whether one weapon slot may fire: its absolute
next-ready simulation tick and, for an automatic weapon, its belt count. The owner predicts it, but
corrections restore at the authority sample's producing tick and replay forward (ADR-0029).
_Avoid_: reload timer (the gate also covers cyclic readiness and belt swaps), belt snapshot

**Stabilization**:
Keeping the gun's lay steady against hull motion. Three regimes, by what is held fixed:
- *Unstabilized* — the gun holds a hull-relative bearing and sweeps as the hull moves (WW2). Aim stored hull-local.
- *Directional stabilization* — the gun holds a fixed world *direction* (a ray: bearing + elevation), counter-rotating against hull motion but not tracking a point while driving (the modern two-plane stabilizer; fire-on-the-move). Aim stored as a world ray.
- *Point stabilization* — the gun holds a fixed world *point* (a position), re-laying as the hull rotates *and* translates so it tracks the spot through parallax (lock-on / FCS auto-tracker). Aim stored as a world point.
The mechanism is unstabilized, and the other two regimes are deliberate later mechanics. But the lay
follows the aiming view and the player owns the view (ADR-0038), so a player holding the crosshair in
third person — a world-locked orbit camera, re-picking live every frame — does keep the gun on a
world point, rate-limited by the mount. That is the crew answering live input, not a stabilizer:
stop steering and it is gone. A hold, free-look, silence, and the whole hull-anchored gunner optic
are all hull-rigid.
_Avoid_: "stab" (write it out); calling the third-person track a stabilizer

## Driving

**Running gear**:
The whole belt-contact mechanism of one side — roadwheels, track, sprocket, idler, and the belt stations through which support and traction reach the hull.

**Roadwheel**:
A load-bearing wheel that shapes the track route and its presentation. Roadwheels no longer own locomotion samples; the belt's contact stations carry support and traction.
_Avoid_: wheel (ambiguous with the sprocket / idler / return rollers)

**Sprocket / Idler**:
The drive sprocket, where transmission output sets belt motion, and the idler, which closes and tensions the route at the other end. Their pin-line circles shape the belt loop.
_Avoid_: drive wheel

**Track** (belt):
The continuous material loop around one side's running gear. Its advected speed and phase are simulation state; its pitch-spaced stations are the tank's ground-contact model, while the rendered links are only its view.
_Avoid_: cosmetic track, tread, caterpillar

**Contact station**:
A pitch-spaced sample on the belt's ground-facing route. Each station probes across the shoe width and contributes belt-normal support plus longitudinal and lateral traction.
_Avoid_: roadwheel contact, contact patch

**Belt element**:
A material track-link identity crossed with one lateral collocation column. Elements advect with belt phase, so grip state follows the piece of track carrying it instead of whichever geometric station happens to occupy that place.

**Per-element strain grip**:
The elastic–plastic ground-shear state carried by each belt element: world-space strain develops force, stays elastic below breakaway, saturates into sliding at the available grip budget, and is forgotten only after definitive contact loss. There are no grip anchors and no stick-speed switch.
_Avoid_: grip anchor, static/kinetic gate

**Belt speed / Belt phase**:
*Speed* is the signed material speed of one track against its route. *Phase* is its unbounded accumulated travel, used both to advect contact stations and to identify which belt elements occupy them.

**Sprocket pitch radius**:
The radius derived from authored track pitch and sprocket tooth count. Declared per-gear belt speeds derive the transmission reductions against this radius.
_Avoid_: wheel radius, effective radius

**Ride height**:
The hull's resting height where the belt-normal support field carries its weight. It emerges from belt penetration and the declared support law, not from roadwheel suspension rays.

**Differential thrust**:
The left–right difference in track force. Steering and yaw emerge from the two belts' traction lever arms, not from a separate yaw actuator.

**Skid steer**:
Turning through differential belt motion while the contact elements shear laterally against the ground.

**Neutral steer**:
Counter-rotating the belts at near-zero mean speed so their equal-and-opposite traction produces yaw with little translation.
_Avoid_: pivot turn, neutral turn

**Declared transmission**:
The vehicle-authored engine envelope, gear ladders, steering radii, capacities, brakes, shift behavior, and architecture. Speeds and radii are source data; reductions, curvatures, and other coupled quantities derive in one validated constructor.

**Transmission adapter**:
The drivetrain law selected behind the shared joint two-output seam:

- *Governor* — the legacy independent-belt path and multiplayer parity mode.
- *Hybrid* — continuously variable regenerative steering with inner-to-outer power recirculation.
- *Fixed radii* — geared regenerative steering constrained to the declared radius detents.

**Signed shaft**:
Mean belt motion expressed relative to the engaged forward or reverse ladder. Normal motion in the selected direction is positive; a belt back-driven against that direction is negative. The sign belongs to the shaft, not the engine crank.

**Engine crank** (`ω_e`):
The non-negative engine-side angular-speed state. Engine torque, drag, inertia, clutch coupling, stall guard, and declutched rev matching evolve it directly; geared belt speed no longer stands in for engine rpm.

**Reserve scheduler**:
The gear selector for the regenerative adapters. It filters signed mean-axis load demand, compares each legal gear's available force with the required reserve, and schedules protective downshifts, hill-hold recovery, or grade-limit truth without discarding the shift and landing guards.

**Steering detent**:
A fixed-radii steering state selected with hysteresis: straight, wide radius, or tight radius. While a turn detent is engaged, the transmission constrains belt-speed difference to the declared curvature and defers predictor-blind upshifts.

**Hill-hold**:
The reserve scheduler and brake latch holding or rescuing a tank on a grade when the engaged gear lacks launch reserve. Grip remains the belt elements' strain law; hill-hold is drivetrain state, not a world anchor.
_Avoid_: grip anchor

**Engine-brake / coast-down**:
Zero-throttle crank drag transmitted through the engaged clutch and gear, bleeding belt and hull speed toward the parking regime. Service braking comes from opposite-throttle intent and is a separate, capacity-limited stop force.

## Netcode

**Shot identity** (`ShotId`):
The stable Battle-local identity shared by one authored round, its public trajectory facts, and its
owner-private damage confirmation: firing `CombatantId`, weapon slot, and authority fire tick. The
current weapon mechanism emits at most once per slot per tick; a future mechanism that breaks that
invariant must widen the identity first.
_Avoid_: projectile entity (an entity is local and transient; shot identity is plain correlated data)

**Divergence continuity**:
The Layer-1 rule (ADR-0015): contact and force laws must be continuous functions of pose and velocity, so tiny client/server divergence nudges a blend weight instead of flipping a force regime and bifurcating the sims. Precedents: the sphere-cast suspension probe and the static↔kinetic friction blend, both from the retired `driving.rs` force law (the track model replaced it — see `src/track/forces.rs`); binding on all future force laws, the track model included.
_Avoid_: "determinism" for this (continuity bounds divergence growth; determinism eliminates divergence)

**Forward determinism / Replay determinism**:
*Forward*: same state + same inputs → same result, on any machine. *Replay*: restore a snapshot, resimulate, and land bit-identically on the original forward path. Prediction + rollback wants both — forward to make corrections *rare* (client and server agree while nothing surprising happens), replay to make a correction *converge* instead of seeding new error. Neither is a correctness requirement under server authority, which re-anchors regardless.
_Avoid_: filing determinism under lockstep. Lockstep *needs* forward determinism; so does predict-and-rollback. Determinism is a property of the sim, orthogonal to who holds authority (ADR-0015).

**Misprediction / Divergence**:
The two error classes a correction repairs. *Misprediction*: you guessed a remote player's next input wrong — information-theoretic, irreducible, and **determinism cannot touch it**. *Divergence*: same inputs, different results — determinism eliminates it entirely. Solo rollbacks are pure divergence, which is why ADR-0015 treats them as a defect metric with target ~zero.
_Avoid_: "determinism makes prediction more accurate" (it makes *replay* exact; the guess is bounded by information, not reproducibility)

**Prediction margin**:
How many ticks the client runs ahead of the confirmed state it receives. Input delay eats it: `InputDelayConfig::balanced()` at loopback RTT absorbs all latency into delay, margin hits zero, and every confirmed update arrives at-or-ahead of the current tick.

**Check starvation**:
The zero-margin failure (fixed by `net/watchdog.rs`): lightyear's receive-time rollback check is skipped for any sample stamped at-or-ahead of the current tick and never retried, so state rollback goes permanently, silently dead — measured 35–50 m divergence with fresh authority arriving and zero rollbacks. Pre-watchdog lat0 rollback counts measured this, not convergence.

**Tick index** (predicted `P` / server `S` / confirmed `C` / interpolation `I`):
The tick a given entity is a view of. A client's world is not a snapshot of one instant: its own tank lives at the *predicted* index `P`, an opponent's collider at the *interpolation* index `I`, and server-authoritative facts arrive on the *confirmed* frontier `C`. See `design/timelines-and-shear.md` for the offsets and their sources.
_Avoid_: comparing `C` and `I` as if commensurable — `C` is a global replication-completeness frontier, `I` a per-entity render index.

**Shear**:
The tick gap between two entities that interact. Interactions are only well-posed between entities on the same tick index; static world geometry has no index, which is why driving feels right and ramming does not. Ramming, un-learnable aim lead, and the incoherent opponent tracer are one phenomenon (ADR-0017).
_Avoid_: "lag" or "latency" for this (those are wall-clock; shear is measured in ticks between two entities)

**Complete cause**:
A cause whose whole future is a function of information already held, so a consequence can be placed on *any* tick index by exact arithmetic rather than guessed. A fire event is complete — *a projectile has no free will*. An input stream is not: the next input is unknowable. The first test for deriving rather than replicating (ADR-0016).

**Contractive / Expansive**:
Whether a system's dynamics shrink or grow a perturbation — the Lyapunov question. A servo chasing a target and a damped recoil spring are contractive, so they tolerate a stale cause and can be derived. **A contact solver is expansive**, which is why collision resolves on the authority. Distinct from *divergence continuity*: a contact solver is roughly continuous and still expansive.
_Avoid_: "self-correcting" (does not distinguish contractive from merely bounded or oscillatory)

**Netcode scaffolding** (Layer 1 / Layer 2):
The two-layer doctrine (ADR-0015). *Layer 1* — permanent sim-design work, ours: divergence continuity. *Layer 2* — deliberately removable workarounds, each mapped to a named upstream defect with a removal condition (watchdog, contact-restore fix, coarsened thresholds). The render-space error layer looks like Layer 2 but is permanent — other players' inputs are unpredictable forever, and it is how any correction is presented.
_Avoid_: calling Layer-1 work a workaround (it stands on its own merits)

## Collision

**Part layer**:
One of the parallel concerns a rig part carries: its visual mesh, its collision proxy, and its ballistic volumes (armor and modules alike — see Armor & penetration). Each is authored as child geometry/components of the part and queried independently, by type. The part is the unit; the layers compose on it.

**Collision proxy**:
A simplified convex shape standing in for a part's detailed mesh in the physics solver — authored on the model as a hidden collider mesh, never the render mesh. Coarse by design: only the outer convex envelope matters to collision.
_Avoid_: collision mesh (suggests the full visual mesh)

**Compound collider**:
Several convex proxies on one rigid body that together approximate a concave shape (e.g. the stepped hull front as 2–3 pieces). The only way to represent concavity for a dynamic body, which cannot use a single concave collider.

## Armor & penetration

(Model: `.agents/docs/design/armor-penetration-and-damage.md`.)

**Ballistic volume**:
A watertight solid mesh plus a material that taxes a penetrator over the line-of-sight distance through it — the single primitive both armor and modules are. Read by the penetration raycast, not the physics solver, so it need not be convex (but must be manifold).
_Avoid_: armor plate, module (those are roles layered on a ballistic volume, not the thing itself)

**Module**:
A ballistic volume that also carries a function and state (engine, ammunition, breech, optics, transmission). Loses capability when damaged; repairable (ammunition excepted). Crew are the other layered role.
_Avoid_: component (use it loosely in prose, but the rig term is module)

**Material factor**:
The per-volume multiplier turning line-of-sight distance into penetration cost — high for dense armor steel, low for an engine block. Density/hardness expressed as one number.

**Line-of-sight thickness**:
The geometric distance a penetrator travels through a ballistic volume, entry face to exit face. Slope is captured by this length, not by a separate cosine term.
_Avoid_: effective thickness (that is line-of-sight thickness × material factor — the cost)

**Penetration capability**:
The reference-millimetres of armor a shell can defeat at its *current* velocity — a derivative of velocity for a given shell, not a fixed stat.
_Avoid_: penetration value, pen (it changes shot-to-shot as velocity bleeds)

**Normalization**:
The penetrator's path bending toward the surface normal as it enters a volume, shortening its line-of-sight path.

**Ricochet**:
Deflection off a too-steep face without entering — spawns a new path segment and bleeds velocity, no penetration. Suppressed by overmatch.

**Overmatch**:
When a shell's caliber greatly exceeds a volume's thickness along its normal, suppressing ricochet and slope. The game's namesake, but one modifier among many — not the centre of the model.

**Spall** (exit cone):
The fixed-shape cone of fragments thrown from a volume's exit face on perforation — dense on-axis, thinning with angle and distance — and the primary crew-killer. Each fragment is one HP unit that stops at the first volume it reaches.
_Avoid_: spalling, fragmentation, frag (the noun is spall; the emitter is the exit cone)

**Station**:
The *place* a crewman works — a fixed, spatial ballistic volume carrying a **role** (the gunner's station grants the gunnery capabilities). Persists whether occupied by a living crewman, a corpse, or briefly no one. Role lives on the station, not the occupant.
_Avoid_: crew slot, seat, position

**Crewman**:
The *human* occupant — carries HP, death, and (later) skill. **Occupies** one station at a time; backfills a foreign station at degraded effectiveness, the commander being the universal backup. Crew ↔ station is always a 1:1 matching; a swap is a transposition (the dead occupant takes the survivor's vacated station).
_Avoid_: treating crew as a counter (crew is never a count — see kill model)

**Capability**:
A gameplay verb the tank can perform, gated *and graded* by its requirements (crew stations + module functions); its current degree is its effectiveness. The grammar (Group / Part / Pool / Backup, evaluated over part qualities) is shared, but the verbs are now **layered by scope** rather than one global list (ADR-0013): per-**servo** slew gates (`requires`), per-**weapon** `fire`/`load` gates, per-**view** gates (`requires`), and a small global `Capability` map for genuinely tank-wide verbs — currently only **Drive**. (Traverse / Fire / Load / GunnerSight / CommanderView were global capabilities before the rig refactor; they moved onto the servo / weapon / view that owns them.)
_Avoid_: action (the player-facing intent verb is a Control; the tank-model verb is a Capability)

**Effectiveness**:
How well a capability is currently served, ∈ [0, 1] (0 = unavailable, 1 = full) — a *rate* (reload speed, traverse speed, drive power). **Relational**: a crewman's contribution is `competence(crewman, station)`, native 1.0 / foreign degraded, not a fixed attribute. The seam the skill/training system plugs into.
_Avoid_: efficiency (reserve that for a single requirement member's coefficient)

**Cookoff**:
Detonation of an ammunition volume when its HP is depleted — instantly kills all crew. The one terminal, non-repairable event.
_Avoid_: ammo rack explosion, detonation (reserve detonation for HE)

**Union field**:
The material-factor field along a shot's path: at every point, the **max** `material_factor` over the volumes covering it. Cost is its integral, so shared space is charged exactly once, authoring order cannot matter, and adding a volume never lowers protection. Damage stays per-presence — every HP-bearing volume deposits over its own chords, at its own factor, with no ownership.
_Avoid_: CSG merge, boolean union of the meshes (the union is evaluated lazily on the ray; no merged mesh exists)

**Walk**:
How one sample ray is resolved: collect *all* crossings, pair them per volume into enter/exit runs, ε-weld near-touching runs, then integrate cost along the field and fire the boundary laws at each factor step. Replaces the serial one-plate-at-a-time march, in which overlapping and exactly-abutting plates were both crossed free.
_Avoid_: march (that is the shell's flight through the world; the walk is what happens inside one crossing)

**Shell** (as a sampled disc):
The projectile as a caliber-wide body rather than a line: k sample rays (the axis plus a ring at `caliber/2`) each walked, then aggregated. Every scalar the point model consumed becomes an area aggregate — the mean entry normal `n̄`, and the covered fraction **η**, which scales cost, ricochet bleed and deflection angle alike. A fragment is this same primitive at r→0, k=1.

**η** (engagement fraction):
The fraction of a shell's disc covered by material at a crossing. η = 1 is a fully-buried hit, η = 0 flies free, and everything between is continuous — which is what makes a graze a partial ricochet, an MG port a graded weakspot, and a weakspot caliber-gated without any authored weakspot data.

**ε-weld**:
Runs separated by less than the weld tolerance — DERIVED ≈2 mm, measured **perpendicular** to the faces — merge into one run. It merges event *topology*, not material: one entry face, one exit face, one overmatch test, while the gap itself is never charged and material steps inside the welded run still spall by the field law. Bounds the omitted void per run at ε, so a picket fence cannot chain into one plate.
_Avoid_: tolerance, snapping (this deletes phantom faces; it never creates steel)

**Corridor**:
A path through a tank that reaches crew or ammunition without crossing enough material to stop the admitting caliber. Always an authoring defect: the ray fuzzer reports one with its admitting caliber and per-caliber η ("this seam admits ≥8 mm; at 88 mm η = 0.93") and fails the gate by name.

**Fail-closed**:
The armor read's response to a question it cannot answer honestly — an unpairable topology, an unprobeable collider, a replica with no authoritative verdict. The round stops where it was: no perforation, no spall, no transit damage, no fabricated event. Free penetration is the one outcome worse than a stopped shell, because it is indistinguishable from armor that was never modeled.
