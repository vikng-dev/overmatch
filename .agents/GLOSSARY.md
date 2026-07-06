# Overmatch

A realistic 3D multiplayer tank game (Bevy/Rust), single-player vertical slice in progress.
This file is the project glossary — terms only. Decisions live in `.agents/docs/adr/`.

## Aiming

**Sight** (reticle):
The camera's view direction, marked by the fixed dot at screen center. Where the player is *looking*.
_Avoid_: crosshair, cursor

**Aim point**:
The ground point the gun is *commanded* to hit, resolved from the camera's screen-center ray and stored in the hull's local frame. Intent — where we've told the gun to go, not where it actually points.
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

**Bind window** (retired):
The old hazard interval between a replicated tank root arriving and its sim body finishing an async scene-driven bind — the source of a run of netcode bugs. Closed by ADR-0014: the sim body is now complete at spawn, so a late scene is only a cosmetic view pop-in. The term should now describe *only* the view attach, or be deleted.

## Gunnery

**Servo**:
A 1-DOF *kinematic* rotational motor with a trapezoidal motion profile, slewing turret yaw / gun pitch toward a commanded angle. Not a physics joint — we drive it ourselves.

**Recoil**:
The barrel's rearward kick on firing and its damped spring back to battery — a 1-DOF translational motor, the bore-axis cousin of the Servo.

**Battery**:
The barrel's rest (fully forward) position, to which recoil returns. "Return to battery."

**Stabilization**:
Keeping the gun's lay steady against hull motion. Three regimes, by what is held fixed:
- *Unstabilized* — the gun holds a hull-relative bearing and sweeps as the hull moves (WW2). Aim stored hull-local.
- *Directional stabilization* — the gun holds a fixed world *direction* (a ray: bearing + elevation), counter-rotating against hull motion but not tracking a point while driving (the modern two-plane stabilizer; fire-on-the-move). Aim stored as a world ray.
- *Point stabilization* — the gun holds a fixed world *point* (a position), re-laying as the hull rotates *and* translates so it tracks the spot through parallax (lock-on / FCS auto-tracker). Aim stored as a world point.
Today's default is unstabilized; the other two are deliberate later mechanics.
_Avoid_: "stab" (write it out)

## Driving

**Running gear**:
The whole ground-contact mechanism of one side — roadwheels, track, sprocket, idler.

**Roadwheel**:
A load-bearing wheel of the running gear; the wheels whose share of the tank's weight presses the track onto the ground.
_Avoid_: wheel (ambiguous with the sprocket / idler / return rollers, which carry no ground load)

**Sprocket / Idler**:
The drive sprocket (where engine torque enters the track) and the idler (track tensioner) at the ends of each side. They shape the track loop but bear no ground load.
_Avoid_: drive wheel

**Track**:
The continuous belt around the running gear. In the sim it is **cosmetic** — it carries no physics; locomotion is modelled at the roadwheels.
_Avoid_: tread, caterpillar

**Contact station**:
A longitudinal point where a roadwheel transfers load to the ground; the unit at which both suspension and track-against-ground friction are sampled.
_Avoid_: contact patch

**Effective radius**:
The hub-centre-to-ground distance — wheel radius plus track thickness — shared by the suspension and the visual track so they agree on where the ground is.
_Avoid_: wheel radius (that is only part of it)

**Ride height**:
The hull's resting height, set by where the loaded suspension settles each roadwheel above the ground.

**Suspension travel**:
A roadwheel's vertical range between full compression (bump) and full extension (droop).

**Differential thrust**:
Independent longitudinal force per track; steering arises from the left–right difference, not a separate turn input.

**Skid steer**:
Turning by differential thrust, resisted by the tracks shearing sideways against the ground.

**Neutral steer**:
Pivoting in place with the tracks counter-rotating — equal and opposite thrust giving a pure yaw couple and zero net travel.
_Avoid_: pivot turn, neutral turn

**Friction circle**:
The shared grip budget at a contact station — longitudinal and lateral force together capped at μ × normal load.
_Avoid_: friction ellipse

**Grip anchor**:
The world point a roadwheel's contact sticks to at rest; a brush spring pulls the contact back toward it (capped at the friction circle) to hold the tank statically. Planted when the contact slows past the stick speed, dropped when it breaks loose.
_Avoid_: contact patch (that is the contact station)

**Stick speed**:
The contact speed below which a roadwheel grips — plants a grip anchor and holds with static friction — and above which it slips into kinetic friction. The static↔kinetic gate.

**Hill-hold**:
The tank holding station on a slope under its own grip anchors with no throttle — emergent static friction up to μ × load. Past that the slope wins and it slides.
_Avoid_: handbrake (that is a separate, future input)

**Engine-brake / coast-down**:
The light longitudinal resistance applied when the throttle is released while the tank is still rolling — bleeds speed toward a stop before the grip anchors take over. The "heavy-glide" feel: how much momentum a released tank keeps.

## Netcode

**Divergence continuity**:
The Layer-1 rule (ADR-0015): contact and force laws must be continuous functions of pose and velocity, so tiny client/server divergence nudges a blend weight instead of flipping a force regime and bifurcating the sims. Precedents: the sphere-cast suspension probe and the static↔kinetic friction blend (`driving.rs`); binding on all future force laws, the track model included.
_Avoid_: "determinism" for this (continuity bounds divergence growth; determinism eliminates divergence)

**Forward determinism / Replay determinism**:
*Forward*: same state + same inputs → same result — what lockstep needs, and what engines actually ship. *Replay*: restore a snapshot, resimulate, and land bit-identically on the original forward path — what prediction + rollback needs, and what no engine sells today (avian #734 is the open thread). Our dominant divergence term is a replay failure, not a forward one.

**Prediction margin**:
How many ticks the client runs ahead of the confirmed state it receives. Input delay eats it: `InputDelayConfig::balanced()` at loopback RTT absorbs all latency into delay, margin hits zero, and every confirmed update arrives at-or-ahead of the current tick.

**Check starvation**:
The zero-margin failure (fixed by `net/watchdog.rs`): lightyear's receive-time rollback check is skipped for any sample stamped at-or-ahead of the current tick and never retried, so state rollback goes permanently, silently dead — measured 35–50 m divergence with fresh authority arriving and zero rollbacks. Pre-watchdog lat0 rollback counts measured this, not convergence.

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
