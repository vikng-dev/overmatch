//! Ballistics: the shared shell mechanic. Spawn a kinematic shell, integrate gravity, raycast the
//! terrain along each step, and emit an `Impact`. This is the library seam both the player's gun
//! (`shooting`) and the armor sandbox (`bin/armor_sandbox`) drive: they raise a `FireShell` event;
//! ballistics owns the trajectory and the impact query. Hand-integrated on purpose — we own the
//! trajectory (muzzle velocity, gravity, later drag/penetration as data + rules); Avian only answers
//! the impact query: what the segment hit, where, and the surface normal.
//!
//! The armor penetration march, ballistic volumes, and spall (design doc
//! `.agents/docs/design/armor-penetration-and-damage.md`) grow off the `Impact` seam here.

use std::time::Instant;

use avian3d::prelude::{Forces, LayerMask, SpatialQuery, SpatialQueryFilter, WriteRigidBodyForces};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;

use crate::damage::{VolumeOf, hit_ancestor};
use crate::state::GameplaySet;
use crate::{ClientReplica, Layer, PredictedPresent, Replaying, ShotId};

/// Gravity applied to shells each fixed tick (m/s²).
const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

/// World-floor height (m): a shell that descends past this has cleared the map edge into the void
/// below the terrain and is culled. Gravity guarantees every shell reaches it within seconds unless
/// it hits terrain first — and an in-play arc always does, impacting the ground well above this — so
/// this only removes the escapees that would otherwise integrate forever (the never-despawn leak),
/// with zero effect on any legitimate shot (including straight-up or lobbed shells, which come back
/// down onto terrain). Far below the lowest terrain (the ~0 m slab). A shell can't reach it via
/// f32 overflow instead: max reach is ~10^5 m (gravity bounds the apex), ~33 orders below `f32::MAX`.
const KILL_FLOOR: f32 = -100.0;

/// Lumped drag-form constant for the quadratic air-drag model `dv/dt = −k·v²`. The per-shell
/// coefficient is `k = DRAG_FORM · caliber²/mass` (1/m): `caliber²/mass` is the shell's (inverse)
/// sectional density, so a heavy-for-bore round (the 88) holds velocity while a light-for-bore one
/// (the 7.9 mm coax) bleeds it. Calibrated so the 88 (0.088 m, 10.2 kg) keeps its hand-tuned
/// k ≈ 2e-4 — which, from sectional density alone, makes the coax bleed ~7× faster with no per-weapon
/// field. A per-shell form factor (shape: pointed AP vs APCR vs ball) joins the shell data later.
/// Sandbox-tunable.
const DRAG_FORM: f32 = 0.263;

/// A shell's quadratic-drag coefficient `k` (1/m), from its (inverse) sectional density. Shared by
/// the live shell and the fire-control range table so the aim solution and the actual flight bleed
/// speed identically — penetration `capability` (∝ vⁿ) then falls with range for both.
pub fn drag_k(caliber: f32, mass: f32) -> f32 {
    DRAG_FORM * caliber * caliber / mass
}

/// One free-flight integration step: apply gravity, then quadratic drag, returning the new velocity.
/// Drag is integrated analytically (`v ← v/(1 + k·v·dt)`, unconditionally stable, unlike explicit
/// Euler at high `v·dt`). This is the shared flight kernel — the live shell march
/// ([`integrate_projectiles`]) and the fire-control range table both step it, so a shell lands where
/// the superelevation solution said it would. In-plate cost dwarfs drag, so this is free-flight only.
pub fn freeflight_step(velocity: Vec3, drag_k: f32, dt: f32) -> Vec3 {
    let v = velocity + GRAVITY * dt;
    let speed = v.length();
    if speed == 0.0 {
        return v;
    }
    (v / speed) * (speed / (1.0 + drag_k * speed * dt))
}

/// One free-flight ADVANCE of a shell over `dt`: step the velocity through the shared drag/gravity
/// kernel ([`freeflight_step`]), then step position by that new velocity (`p ← p + v·dt`). Returns
/// `(new position, new velocity)`.
///
/// This is THE single definition of "how a shell advances one tick in open air." The live march
/// ([`integrate_projectiles`]) opens every tick with it (its ray-march then refines the position only
/// if the segment hits something), and the FireEvent catch-up ([`fast_forward_shell`]) folds it once
/// per skipped tick — so a caught-up shell and a natively-integrated one advance by ONE
/// implementation, not two that happen to agree today (ADR-0016). Collision-free by construction: the
/// caller owns the raycast (the live march casts each step; the catch-up is cosmetic and deliberately
/// does not — see [`fast_forward_shell`]).
pub(crate) fn advance_shell(position: Vec3, velocity: Vec3, drag_k: f32, dt: f32) -> (Vec3, Vec3) {
    let velocity = freeflight_step(velocity, drag_k, dt);
    (position + velocity * dt, velocity)
}

/// Fast-forward a just-fired shell `ticks` free-flight steps from its muzzle — the net FireEvent
/// catch-up (`net::client::receive_fire_events`). Returns the caught-up `(position, velocity)` and the
/// arc it traced (origin first, one point per stepped tick) so the [`ShellPath`] trail starts at the
/// muzzle rather than 64 m behind the shell.
///
/// One per-tick advance — the shared [`advance_shell`] the live march steps — so the catch-up cannot
/// drift from natively integrating the same `ticks`. Ballistic (no per-step raycast): this returns the
/// free-flight arc, and whether the round ALREADY hit something during the skipped flight is the
/// caller's concern ([`on_fire_shell`] clears that with a single segment raycast — see there). The
/// skip is systematic under the predicted-present timeline — MEASURED ≈4 ticks / ~49 m at RTT ≈ 91 ms,
/// growing with RTT (see `net::protocol::FireEvent::fire_tick` and `design/timelines-and-shear.md` §2)
/// — which is exactly why the returned arc points matter: they
/// populate the trail so the tracer reads as a round already in flight, not one teleporting in.
pub(crate) fn fast_forward_shell(
    origin: Vec3,
    velocity: Vec3,
    drag_k: f32,
    dt: f32,
    ticks: u32,
) -> (Vec3, Vec3, Vec<Vec3>) {
    let mut pos = origin;
    let mut vel = velocity;
    let mut points = Vec::with_capacity(ticks as usize + 1);
    points.push(pos);
    for _ in 0..ticks {
        (pos, vel) = advance_shell(pos, vel, drag_k, dt);
        points.push(pos);
    }
    (pos, vel, points)
}

/// Penetration capability: `pen = K · mass^Mₑ · speed^N` (reference-mm — the DeMarre shape, design
/// doc §3). **Mass is the primary driver** (sectional density / kinetic energy), speed the secondary;
/// caliber is deliberately *not* here — it drives overmatch and spall hole-size, not raw penetration.
/// Calibrated so the 88 (≈10.2 kg PzGr at ~773 m/s) ≈ 250 mm — *identical to the old speed-only curve
/// at that mass*, so the existing 88 behaviour is unchanged; the mass term only separates other
/// rounds (a ~13 g rifle/MG round lands ~10 mm → can't defeat real armour, only chips exposed parts).
/// Per-shell constants become shell data later.
const PEN_K: f32 = 0.005_8;
const PEN_N: f32 = 1.43;
/// Exponent on projectile mass (kg). ~0.5 ≈ sectional-density-like — the lever that separates a heavy
/// tank shell (deep) from light small arms (shallow). Sandbox-tunable.
const MASS_EXP: f32 = 0.5;

/// Reference-mm a projectile of `mass` kg can defeat at `speed` m/s.
fn capability(mass: f32, speed: f32) -> f32 {
    PEN_K * mass.powf(MASS_EXP) * speed.powf(PEN_N)
}

/// Inverse of [`capability`] for a fixed projectile: the speed carrying `capability` reference-mm at
/// this `mass`. Spending cost then inverting is the Lambert–Jonas residual-velocity shape —
/// barely-penetrate exits slow, big overmatch barely slows (design doc §3).
fn speed_for(mass: f32, capability: f32) -> f32 {
    (capability / (PEN_K * mass.powf(MASS_EXP))).powf(1.0 / PEN_N)
}

/// Fragment directions for a spall cone, each paired with its normalized polar position `t` ∈ [0,1]
/// (0 = on-axis): `n` rays inside a cone of half-angle `half_angle` about `axis`, spread by the
/// golden angle and packed denser toward the axis (design §5). `t` lets the caller make on-axis
/// fragments stronger — the continuous form of War Thunder's "more power ↔ narrower cone" groups.
/// Deterministic: the same shot throws the same cone (A/B in the sandbox).
fn spall_directions(axis: Dir3, half_angle: f32, n: usize) -> Vec<(Dir3, f32)> {
    let z = Vec3::from(axis);
    let up = if z.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    let x = z.cross(up).normalize();
    let y = z.cross(x);
    const GOLDEN: f32 = 2.399_963_2;
    (0..n)
        .filter_map(|k| {
            let t = (k as f32 + 0.5) / n as f32;
            let polar = half_angle * t;
            let az = k as f32 * GOLDEN;
            let local = z * polar.cos() + (x * az.cos() + y * az.sin()) * polar.sin();
            Dir3::new(local).ok().map(|d| (d, t))
        })
        .collect()
}

/// Max RHA-mm an on-axis fragment can defeat at full shot energy (WT puts secondary fragments at
/// 3–30 mm RHA). Scaled down by off-axis angle and residual energy at birth.
const FRAG_PEN_MAX: f32 = 30.0;
/// Fragment air drag (1/m): a fragment's penetration bleeds with distance — low mass + tumbling, so
/// steep. Lethal point-blank behind the plate, nearly spent a few metres on (the BAD short range).
const FRAG_DRAG: f32 = 0.6;
/// HP a fragment deposits per RHA-mm of its current penetration at the moment of impact.
const FRAG_DMG_PER_MM: f32 = 0.12;

/// March one spall fragment as a mini-penetrator: it flies to the first ballistic volume, deposits
/// damage scaled by its current penetration (an energy packet), and either punches through a thin
/// volume (losing the cost it spent) or stops in a thick one — so the engine block still shadows the
/// crew, but a thin bulkhead no longer fully protects them and a strong fragment can exit the tank
/// to reach another (design §5). `pen` bleeds with distance (drag). Returns the visual trace.
fn cast_spall_fragment(
    origin: Vec3,
    dir: Dir3,
    mut pen: f32,
    mut range: f32,
    spatial: &SpatialQuery,
    volumes: &Query<&BallisticVolume>,
    parents: &Query<&ChildOf>,
    health: &mut Query<&mut ComponentHealth>,
    filter: &SpatialQueryFilter,
    // Authority-only HP deposition: `false` on the net client (a replica), which still traces the
    // fragment (for FX / `deposited`) but leaves the actual HP write to the server.
    deposit: bool,
) -> SpallFragment {
    const EPS: f32 = 1.0e-3;
    const PROBE: f32 = 50.0;
    let mut pos = origin;
    let mut deposited = false;
    while range > EPS {
        let Some(hit) = spatial.cast_ray(pos, dir, range, true, filter) else {
            pos += Vec3::from(dir) * range; // flew the rest, hit nothing
            break;
        };
        let at = pos + Vec3::from(dir) * hit.distance;
        pen = (pen / (1.0 + FRAG_DRAG * hit.distance)).max(0.0); // drag over the gap
        // Resolve the struck volume's node + material factor (`hit_ancestor`, the shared walk).
        let node = hit_ancestor(hit.entity, volumes, parents).map(|(e, v)| (e, v.material_factor));
        let Some((node_entity, factor)) = node else {
            pos = at;
            break;
        };
        // Deposit damage scaled by current penetration (energy), if it's a damageable component.
        // `deposited` still records the hit (the visual trace) on a replica; only the HP write is
        // authority-gated.
        if let Ok(mut hp) = health.get_mut(node_entity) {
            if deposit {
                hp.current = (hp.current - pen * FRAG_DMG_PER_MM).max(0.0);
            }
            deposited = true;
        }
        // Cost to cross this volume = its thickness along the fragment path × material factor.
        let span = spatial
            .cast_ray_predicate(
                at + Vec3::from(dir) * EPS,
                dir,
                PROBE,
                false,
                filter,
                &|e| e == hit.entity,
            )
            .map(|exit| EPS + exit.distance)
            .unwrap_or(0.0);
        let cost = span * factor;
        if pen > cost {
            // Punch through: spend the crossing cost and continue from the far face.
            pen -= cost;
            pos = at + Vec3::from(dir) * (span + EPS);
            range -= hit.distance + span + EPS;
        } else {
            // Stops inside this volume (depth scaled by the fraction it could pay).
            pos = at + Vec3::from(dir) * span * (pen / cost.max(EPS));
            break;
        }
    }
    SpallFragment {
        end: pos,
        deposited,
    }
}

/// Mirror a travel direction about a surface normal — the specular deflection of a ricochet.
fn reflect(dir: Dir3, normal: Dir3) -> Dir3 {
    let d = Vec3::from(dir);
    let n = Vec3::from(normal);
    Dir3::new(d - 2.0 * d.dot(n) * n).unwrap_or(dir)
}

/// Rotate `dir` toward `target` by `angle` radians (clamped to the angle between them). Used to bend
/// the penetrator toward the inward normal on entry — normalization.
fn bend_toward(dir: Dir3, target: Dir3, angle: f32) -> Dir3 {
    let d = Vec3::from(dir);
    let t = Vec3::from(target);
    let between = d.angle_between(t);
    if between < 1.0e-5 || angle <= 0.0 {
        return dir;
    }
    let Ok(axis) = Dir3::new(d.cross(t)) else {
        return dir;
    };
    Dir3::new(Quat::from_axis_angle(Vec3::from(axis), angle.min(between)) * d).unwrap_or(dir)
}

/// SHOOTER SELF-EXCLUSION — the ray-cast predicate every shell cast runs: a round never resolves
/// against the tank that FIRED it. `true` keeps the candidate collider, `false` makes it transparent.
/// `shooter` is the firing tank's root ([`ShotSource::tank`]); `None` (the sandbox's free-fly camera —
/// no tank) excludes nothing. Terrain has no [`VolumeOf`] ancestry and is therefore never "own", so it
/// stops every shell as before.
///
/// # Why a shell must ignore its own tank
///
/// A muzzle inside its own geometry is NORMAL, not a modelling error: a recoiling barrel *retracts*
/// its muzzle, and a gun mounted through a mantlet retracts it BEHIND that mantlet. The tiger's coax
/// is exactly this — its muzzle clears `Gun_Mantlet_Ballistic` by ~7 cm at rest, and its recoil spring
/// (kick 3.0 m/s) pulls it ~10 cm back, so every round after a burst's first one is fired from INSIDE
/// the tank's own mantlet. With no exclusion the round's first cast hit that mantlet millimetres out,
/// and:
///   * on the AUTHORITY it embedded there (the 7.9 mm round's ~8 ref-mm capability cannot cross
///     1000-factor steel) — the coax was a dud that shot itself in its own mask;
///   * on a NET CLIENT (`!deposit`) the contact fail-closed — the shooter's own shell held hidden at
///     the muzzle for the grace window, and an observer's shell was killed even earlier, by
///     `on_fire_shell`'s already-landed catch-up test, so no tracer was EVER spawned. The bow MG, which
///     has no ballistic volume anywhere in front of it, replicated fine — the asymmetry Yan saw.
///
/// The bore geometry is a modelling detail; the *rule* is not. [`crate::aim::aim_distance`] has always
/// excluded the firing tank from the aim ray this way; the shell march simply never did.
///
/// Exclusion runs for the shell's WHOLE flight, not just a muzzle-clearance distance: it needs no
/// magic radius, no per-shell distance state (which a rollback would have to restore), and it is the
/// same answer on every machine. The cost is that a round which ricochets off the world back into its
/// own tank passes through it — an outcome no gun can currently produce, and one the previous
/// behaviour got wrong in a far louder way.
///
/// IDENTITY, NOT AUTHORITY: this is why the observer's re-raised `FireShell` names its `shooter`
/// (`net::client::receive_fire_events`). Damage deposition stays gated on [`crate::ClientReplica`]
/// alone (`deposit`) — naming the shooter grants a replica shell no damage path whatsoever.
fn not_own_volume(
    entity: Entity,
    shooter: Option<Entity>,
    owners: &Query<&VolumeOf>,
    parents: &Query<&ChildOf>,
) -> bool {
    let Some(shooter) = shooter else {
        return true;
    };
    // Ownership sits on the hit's ancestry (`hit_ancestor`, the shared hierarchy-resolution rule) —
    // the same walk `aim_distance` makes for the aim ray.
    hit_ancestor(entity, owners, parents).is_none_or(|(_, owner)| owner.tank() != shooter)
}

/// Whether a spent shell freezes in place — keeping its stuck mesh, tracer, and penetration marks
/// for inspection — instead of despawning. The game despawns (default); the sandbox opts in.
#[derive(Resource, Default)]
pub struct RetainSpentShells(pub bool);

/// How the shell march is integrated. The game uses `Real`; the sandbox can toggle to `Demo`.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarchMode {
    /// On the fixed server-rate step (`Time<Fixed>`) — the true simulation cadence, so slow-motion
    /// shows the actual discrete hops.
    #[default]
    Real,
    /// Per-frame on virtual time (`Time<Virtual>`) — smooth continuous motion for demoing
    /// (frame-rate dependent; the velocity, hence penetration, is unchanged).
    Demo,
}

fn march_real(mode: Res<MarchMode>) -> bool {
    *mode == MarchMode::Real
}

fn march_demo(mode: Res<MarchMode>) -> bool {
    *mode == MarchMode::Demo
}

/// The tank + weapon a shell was fired from — the CAUSE the net server broadcasts so every OTHER
/// client can DERIVE that shot's consequences (the cosmetic tracer AND the shooter's barrel recoil)
/// from its own local spec, with no impulse or spring state ever riding the wire. Pairing the
/// attributed tank with the weapon slot in one value makes the two impossible to disagree — an
/// attributed shot always knows which weapon fired it, so the recoil kick lands on the right barrel.
/// The slot is the weapon's `TankSim::weapons` index (its `WeaponIndex`). Read by `net::server` to
/// attribute the shot on the wire.
///
/// Also a `Component`: `on_fire_shell` attaches it to the spawned shell whenever the shot was
/// attributed, so the authority (`net::server`) can complete the shell's [`ShotId`] with the fire
/// tick (the one part the sim layer cannot know — the tick lives in lightyear's timeline). See
/// [`Shot`] for the two-step stamping.
#[derive(Clone, Copy, Component)]
pub struct ShotSource {
    /// The tank root the shell was fired from.
    pub tank: Entity,
    /// The firing weapon's slot in `TankSim::weapons` — its spawn-time `WeaponIndex`.
    pub weapon: usize,
}

/// A cosmetic shell's network identity ([`ShotId`]) — the correlation spine both ends stamp so a
/// server-sanctioned [`ShellRicochet`]/`RicochetKeyframe` re-seeds exactly the shell it belongs to.
///
/// Stamped wherever a shot has a network identity, by two paths (the sim layer cannot read the fire
/// tick — it lives in the netcode timeline):
///   * **Observer shells** (a replica watching another tank): `net::client` fills [`FireShell::shot`]
///     straight from the wire and `on_fire_shell` attaches it here.
///   * **Attributed locally-fired shells** — the server's authoritative shell AND the shooter's own
///     predicted client shell: `on_fire_shell` leaves `FireShell::shot` `None` (no tick in the sim),
///     and the shared `net::protocol::stamp_shot_ids` completes it after spawn from the shell's
///     [`ShotSource`] + the timeline tick — yielding the SAME id on every machine (see that system's
///     doc for why the tick and the mapped shooter entity agree across ends).
///
/// Every `Shot`-carrying shell on a net client is keyframe-eligible: at armor contact the march
/// re-seeds it from its sanctioned bounce or holds briefly for one ([`integrate_projectiles`]) — the
/// shooter's OWN round included, which is the fall-of-shot read the gunnery loop needs. Only shells
/// with NO `Shot` (SP/sandbox — no wire; or an unattributed shot) fail-close immediately at contact.
#[derive(Component, Clone, Copy)]
pub(crate) struct Shot(pub ShotId);

/// A net-client shell FROZEN (and hidden — invisible-stop) at armor contact, waiting the grace window
/// for its server-sanctioned bounce keyframe. `ticks` counts the fixed ticks it has been held.
///
/// # Why `ticks` is exactly the re-seed catch-up (the hold-count arithmetic)
///
/// Every cosmetic shell on a client — the shooter's own (fired at the predicted present natively) and
/// an observer's (aged to the predicted present by the `fire_tick` catch-up) — lives on the SAME
/// P timeline: at local tick X it sits at pos(X) of the authoritative trajectory (shared integrator,
/// same origin and fire tick). The trajectory meets the plate at age `B − fire_tick` (B = the server
/// bounce tick), so the shell reaches contact at local tick ≈ B, and the present then advances one
/// tick per held tick: at keyframe consumption, present = B + `ticks`. Fast-forwarding the sanctioned
/// post-bounce state by `ticks` therefore lands the shell at pos(present) — correctly phased, with no
/// clock read the net-clean sim layer couldn't make. The identical arithmetic covers both cases; no
/// own-vs-observer branch is needed. (Residual: contact tick differs from B by the target's motion
/// over the interpolation delay — sub-metre at tank speeds, within the integration tolerance the
/// carry-through test pins.)
#[derive(Component)]
struct Held {
    ticks: u32,
    /// The surface normal at the contact where the shell froze (this client's own raycast against the
    /// interpolated pose) — used to orient the eventual bounce spark on re-seed, or the neutral spark
    /// if the wait times out. Saved because the top-of-loop `Held` handler has no raycast of its own.
    normal: Vec3,
}

/// The HOLD grace window — ~250 ms at 64 Hz. Because every client shell lives at the predicted
/// present P, AHEAD of server-now S, it reaches the plate (local tick ≈ bounce tick B) wall-clock
/// BEFORE the server resolves the bounce; the keyframe then needs one-way latency to arrive. So the
/// expected hold is ≈ (P − S) + OWL ≈ 4–8 ticks at droplet RTT, and the window must cover that plus
/// send jitter — 16 ticks covers RTT ≲ 200 ms, yet stays short enough that a shell whose verdict never
/// comes finalizes within a quarter second. Correctness never depends on the keyframe arriving inside
/// it — past it the shell degrades to the honest quiet dissolve.
///
/// **This window, not the retain window, is what bounds the redundancy the shell can actually USE.**
/// The server re-sends its window every tick (`net::server::broadcast_fire_window`), so a bounce
/// resolved at server tick B is re-broadcast on B..B+`FIRE_RETAIN_TICKS`; but the shell dissolves once
/// it has held this long, so only the copies sent in the first `RICOCHET_HOLD_TICKS − ((P − S) + OWL)`
/// ticks can still be consumed. At droplet RTT that leaves ~8–12 usable copies (a lost bounce needs
/// every one of them to drop); the slack shrinks as RTT grows, and at RTT ≈ 200 ms it closes — the
/// first arriving copy is the only one that can land. If high-latency play ever becomes a target, this
/// is the constant to derive from measured RTT (the arithmetic is `(P − S) + OWL + jitter`), NOT the
/// retain window.
const RICOCHET_HOLD_TICKS: u32 = 16;

/// F3 tick-triggered consumption margin. A client shell reaches the plate at local tick ≈ the server
/// bounce/impact tick; if it MISSES (its interpolated-pose flight grazes past a plate the server's
/// round resolved on) it never contacts and never holds. Once the predicted present has passed the
/// sanctioned outcome's server tick by THIS many ticks without a local contact, the march consumes it
/// anyway (re-seed at the server bounce, or finalize at the server impact). Sized to clear normal
/// contact slop — a legitimate contact happens right at present ≈ outcome tick and leaves the
/// free-flight path immediately, so a few ticks of grace avoids force-consuming a shell that is about
/// to contact — while staying short enough that a genuine miss snaps to truth after only metres of
/// fly-past, not the full hold window. ~94 ms at 64 Hz.
const OVERDUE_MARGIN_TICKS: u32 = 6;

/// One server-sanctioned ricochet, delivered to a net client's ballistics march so its cosmetic
/// shell — an observer's replica or the shooter's own predicted round — re-seeds from truth instead
/// of improvising a bounce against interpolated geometry. Net-neutral (no lightyear types) so
/// `ballistics` can consume it without naming the netcode; `net::client` fills the store from the
/// replicated `RicochetKeyframe`, the march drains it.
#[derive(Clone, Copy)]
pub(crate) struct SanctionedBounce {
    /// The exact server bounce point — where the re-seeded shell restarts.
    pub origin: Vec3,
    /// The post-bounce travel direction (unit; the receiver guards it before use).
    pub direction: Vec3,
    /// The post-bounce speed (m/s).
    pub speed: f32,
    /// The server tick this bounce resolved on (net-neutral `u32`; `net::client` unwraps the wire
    /// `Tick`). Enables F3's tick-triggered consumption: a client shell whose interpolated-pose flight
    /// MISSED this plate never contacts, so it is re-seeded here once the predicted present passes
    /// `bounce_tick` by a margin (`OVERDUE_MARGIN_TICKS`) — instead of flying on through where the
    /// authoritative round bounced. On the normal hold path the re-age equals `present − bounce_tick`.
    pub bounce_tick: u32,
    /// This bounce's 0-based ordinal within the shot — consumed strictly in order (`0`, then `1`, …),
    /// so multiple ricochets on one shell re-seed in the sequence the server resolved them.
    pub sequence: u32,
}

/// The server-sanctioned TERMINAL of a shot on armor — the honest read for the end the authority
/// resolved: an EMBED (round buried in the plate; shell over) or a PERFORATION (round breached the
/// plate; see the perforation note on [`ShellTerminal`] for why that too ends the cosmetic shell).
/// `penetrated` is `true` for both today — a ricochet is a [`SanctionedBounce`] instead, and terrain
/// never confirms — but rides explicitly so the client renders EXACTLY the flag the server's own
/// `Impact` carried (it gates the flame lick in `vfx::impact`), never re-deriving it. Net-neutral,
/// like [`SanctionedBounce`]: `net::client` fills it from the wire `ImpactConfirm`, the march
/// consumes it.
#[derive(Clone, Copy)]
pub(crate) struct SanctionedTerminal {
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal, straight from the server's raycast.
    pub normal: Vec3,
    /// The server's penetration verdict — gates the flame lick, exactly as the authority's read did.
    pub penetrated: bool,
    /// The server tick this terminal resolved on (net-neutral `u32`; `net::client` unwraps the wire
    /// `Tick`). Enables F3's tick-triggered consumption: a client shell whose interpolated-pose flight
    /// MISSED the struck plate never contacts, so it is finalized at the server's read once the
    /// predicted present passes `impact_tick` by a margin (`OVERDUE_MARGIN_TICKS`) — instead of holding
    /// for a contact the authority already resolved elsewhere.
    pub impact_tick: u32,
    /// How many ricochets the authority resolved BEFORE this terminal. A shell only consumes the
    /// terminal once it has re-seeded through that many bounces (its `ricochets` count matches), so a
    /// multi-bounce shot's terminal can never fire early at the wrong plate while a bounce keyframe is
    /// still in flight.
    pub after_bounces: u32,
}

/// Per-shot sanctioned state: ordered bounces + the (at most one) terminal, plus an age for expiry.
struct SanctionedShot {
    bounces: Vec<SanctionedBounce>,
    terminal: Option<SanctionedTerminal>,
    /// Seconds since last touched — evicted once it outlives any shell that could still consume it.
    age: f32,
}

/// The net client's bounded buffer of server-sanctioned shot outcomes — bounces AND terminals, for
/// observer shells AND the shooter's own — keyed by [`ShotId`] (the shot both ends agree on). Defined
/// here because the ballistics march CONSUMES it (the re-seed / the terminal read), but populated and
/// aged by `net::client` (which owns the wire). Follows the codebase's ring discipline: entries expire
/// with the shell that could consume them and the shot count is capped, so a long match never grows it
/// unbounded. Present only on a net client; SP/sandbox/server never insert it (the march reads it as
/// an `Option`), and the authority march never consults it anyway (it resolves shots for real).
#[derive(Resource, Default)]
pub(crate) struct SanctionedShots {
    shots: std::collections::HashMap<ShotId, SanctionedShot>,
}

impl SanctionedShots {
    /// Longest a shot's sanctioned state lingers unconsumed before eviction — comfortably past a
    /// shell's flight (a ~1.25 km round at 800 m/s ≈ 1.5 s) so a valid keyframe/terminal is never
    /// evicted before its shell reaches it, but bounded so a lost/never-consumed one does not leak.
    const MAX_AGE_SECS: f32 = 3.0;
    /// Hard cap on tracked shots — a backstop against pathological churn; a 1v1 duel never approaches
    /// it. Only reached by an implausible flood of distinct in-flight shots (well past two MGs), and
    /// then the eviction below picks the OLDEST entry — which under such churn could be a shot whose
    /// shell is still mid-hold, prematurely fail-closing (quiet-dissolving) it. Accepted: the cap is a
    /// leak backstop, not a hot-path policy (`MAX_AGE_SECS` does the real eviction), and 64 sits orders
    /// of magnitude above any real shot-in-flight count, so the premature-evict case is unreachable in
    /// practice.
    const MAX_SHOTS: usize = 64;

    /// This shot's entry, fresh-touched, with the over-cap eviction applied.
    fn entry(&mut self, shot: ShotId) -> &mut SanctionedShot {
        if self.shots.len() >= Self::MAX_SHOTS && !self.shots.contains_key(&shot) {
            // Evict the single oldest shot (age is monotonic between touches) — a backstop only (see
            // `MAX_SHOTS`: under a flood this could evict a mid-hold shot, accepted as unreachable).
            if let Some(oldest) = self
                .shots
                .iter()
                .max_by(|a, b| a.1.age.total_cmp(&b.1.age))
                .map(|(k, _)| *k)
            {
                self.shots.remove(&oldest);
            }
        }
        let entry = self.shots.entry(shot).or_insert_with(|| SanctionedShot {
            bounces: Vec::new(),
            terminal: None,
            age: 0.0,
        });
        entry.age = 0.0;
        entry
    }

    /// Record a server-sanctioned bounce, idempotently by `(shot, sequence)` — a redundantly
    /// retransmitted keyframe is a no-op, never a duplicate bounce.
    pub(crate) fn insert(&mut self, shot: ShotId, bounce: SanctionedBounce) {
        let entry = self.entry(shot);
        if !entry.bounces.iter().any(|b| b.sequence == bounce.sequence) {
            entry.bounces.push(bounce);
        }
    }

    /// Record a shot's terminal, idempotently by [`ShotId`] — a shot has AT MOST ONE terminal (the
    /// server strips `Shot` after emitting it), so the first insert wins and a redundantly
    /// retransmitted confirm is a no-op.
    pub(crate) fn insert_terminal(&mut self, shot: ShotId, terminal: SanctionedTerminal) {
        let entry = self.entry(shot);
        if entry.terminal.is_none() {
            entry.terminal = Some(terminal);
        }
    }

    /// Is anything buffered under this exact [`ShotId`]? The buffer is a map KEYED by the shot's id
    /// (whose `shooter` is an entity-mapped reference), so this is the question a mis-keyed shell
    /// silently answers `false` to for its whole short life — see
    /// `net::client::a_miskeyed_shooter_forges_a_second_shot_identity`.
    #[cfg(test)]
    pub(crate) fn has_shot(&self, shot: ShotId) -> bool {
        self.shots.contains_key(&shot)
    }

    /// The shot's next-to-consume bounce (ordinal `consumed`), if it has arrived. `consumed` is how
    /// many bounces this shell has already re-seeded through (its `PenetrationMarks::ricochets` count).
    fn next(&self, shot: ShotId, consumed: usize) -> Option<SanctionedBounce> {
        self.shots
            .get(&shot)
            .and_then(|e| e.bounces.iter().find(|b| b.sequence as usize == consumed))
            .copied()
    }

    /// The shot's terminal, if it has arrived AND the shell has already re-seeded through every bounce
    /// that preceded it (`consumed == after_bounces`) — the ordering guard that keeps a terminal from
    /// resolving a shell that still owes a bounce (its keyframe merely late/in flight).
    fn terminal(&self, shot: ShotId, consumed: usize) -> Option<SanctionedTerminal> {
        self.shots
            .get(&shot)
            .and_then(|e| e.terminal)
            .filter(|t| t.after_bounces as usize == consumed)
    }

    /// Age every tracked shot and evict those past [`Self::MAX_AGE_SECS`]. Driven by `net::client`.
    pub(crate) fn age(&mut self, dt: f32) {
        for entry in self.shots.values_mut() {
            entry.age += dt;
        }
        self.shots.retain(|_, e| e.age <= Self::MAX_AGE_SECS);
    }
}

/// The authority resolved a ricochet — the sim-layer seam `net::server` turns into a server-sanctioned
/// `RicochetKeyframe` broadcast (ADR-0016: replicate the cause). Carries the post-bounce state a
/// client needs to re-seed from truth, keyed by the shell's [`ShotId`] so it correlates to the right
/// cosmetic shell on every client — the observers' replicas AND the shooter's own predicted round —
/// plus the bounce ordinal so multiple ricochets stay ordered. Only raised for a shell that carries a
/// [`Shot`] (net-attributed) on the authority; SP/sandbox shells have no `Shot`, so they raise none
/// and there is no client to listen anyway. Local, never replicated — `net::server` maps it onto the
/// wire.
#[derive(Event)]
pub(crate) struct ShellRicochet {
    pub shot: ShotId,
    pub origin: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub sequence: u32,
}

/// The authority resolved a shot's TERMINAL on armor — an embed or a perforation — the sim-layer seam
/// `net::server` turns into a server-sanctioned `ImpactConfirm` broadcast, completing the shot state
/// machine: every shot now ends in exactly one of {terrain stop (local — pose-independent, both ends
/// agree), confirmed armor terminal (this), fail-closed truncation (the lost-confirm fallback)}.
/// Mirrors the authority's own `Impact` read at the struck plate (position/normal/`penetrated`), so a
/// client renders the SAME honest armor read — flame lick included — that SP shows.
///
/// **Perforation is a terminal for the COSMETIC shell, by choice.** On the authority a perforation
/// reads the struck plate at the entry face (`penetrated: true`) and the shell then continues INTO THE
/// TANK INTERIOR — invisible from outside; only a rare far-side overpenetration re-emerges. A client's
/// cosmetic shell cannot march that interior (interpolated volumes — invariant 2), and what an external
/// viewer of the authority actually sees at the struck plate is exactly this read. So the cosmetic
/// shell ends at the confirmed entry-face read; the interior transit and the rare far-side exit are
/// NOT shown on clients (a future continuation field on `ImpactConfirm` can upgrade that without a new
/// message). AT MOST ONE terminal per shot: the emitting march guards the same tick locally and strips
/// [`Shot`] for later ticks, so post-perforation interior events (crossings, embeds, even bounces)
/// emit nothing — the client shell already ended.
///
/// Only raised for a `Shot`-carrying shell on the authority (same rule as [`ShellRicochet`]);
/// SP/sandbox shells raise none. Local, never replicated — `net::server` maps it onto the wire.
#[derive(Event)]
pub(crate) struct ShellTerminal {
    pub shot: ShotId,
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal.
    pub normal: Vec3,
    /// The server's penetration verdict (gates the flame lick on the client, as in the local read).
    pub penetrated: bool,
    /// Ricochets resolved before this terminal (the client's ordering guard — see
    /// [`SanctionedTerminal::after_bounces`]).
    pub after_bounces: u32,
}

/// Fire a shell — the trigger-agnostic seam. The player's gun and the sandbox camera both raise
/// this; ballistics spawns and integrates the shell. Geometry only — origin, bore direction, muzzle
/// speed — so it carries no assumption about *what* fired it.
#[derive(Event)]
pub struct FireShell {
    pub origin: Vec3,
    pub direction: Dir3,
    pub speed: f32,
    /// Shell calibre (m) — drives overmatch (a round whose calibre dwarfs a plate can't be
    /// deflected by it) and spall hole-size, *not* raw penetration.
    pub caliber: f32,
    /// Projectile mass (kg) — the primary driver of penetration capability (design §3).
    pub mass: f32,
    /// The tank + weapon that fired this shell ([`ShotSource`]), or `None` for trigger sources with
    /// no tank (the sandbox's free-fly camera). It answers TWO questions, and keeping them apart is
    /// what this field's history got wrong:
    ///   * **Self-exclusion (ballistics, BOTH ends).** The round is transparent to the tank that fired
    ///     it — `on_fire_shell` carries the source onto the shell and every cast the march makes
    ///     excludes that tank's volumes ([`not_own_volume`]). Without it a muzzle that recoils behind
    ///     its own armour (the coax, behind its mantlet) shoots its own tank point-blank. This is pure
    ///     IDENTITY: it grants the shell no damage path — deposition is gated on
    ///     [`crate::ClientReplica`] alone — so a cosmetic replica shell names its shooter too
    ///     (`net::client::receive_fire_events`) and self-excludes exactly as the authority does.
    ///   * **Attribution (net server ONLY).** The server's `FireShell` observer reads it to broadcast
    ///     the cosmetic tracer AND the firing weapon to the OTHER clients (`net::server`, the
    ///     "FireEvent" seam): a shot whose source is known is attributed to the right replicated tank
    ///     and weapon slot; `None` shots (sandbox) simply never broadcast. `broadcast_fire` is
    ///     registered on the server and nowhere else, so a client naming its shooter re-broadcasts
    ///     nothing.
    pub shooter: Option<ShotSource>,
    /// Whether THIS round is a tracer (decided at fire time from the weapon's [`crate::spec::
    /// FireMode`]: a `Single`'s round always traces; an `Automatic`'s belt cadence — `tracer_every`
    /// against the belt counter — picks). Governs only the ATTACHED VISUAL, not
    /// the flight or the raycast: an MG tracer round gets the emissive streak, a non-tracer MG round
    /// gets NO visual entity (it still flies + raycasts invisibly), and the main gun keeps its shell
    /// scene regardless (`on_fire_shell`). Rides FireShell (and its net twin [`crate::net::protocol::
    /// FireEvent`]) so shooter, server, and every remote client agree on each round's tracer-ness.
    pub tracer: bool,
    /// How many free-flight ticks to fast-forward this shell at spawn ([`fast_forward_shell`]) — the
    /// net FireEvent catch-up. `0` for every locally-fired shell (the player's gun, the sandbox
    /// camera, and the shooter's own predicted shell): those spawn at the muzzle and fly from there,
    /// so the field is a no-op off the net path. Only `net::client::receive_fire_events` sets it > 0,
    /// to place a remote shot where it already is in the server's confirmed timeline.
    pub catch_up_ticks: u32,
    /// The shot's network identity ([`ShotId`]) when it is ALREADY known at raise time: `Some` only on
    /// the path that re-raises a remote tank's shot (`net::client::receive_fire_events`, which builds
    /// it from the wire, tick included) — `on_fire_shell` attaches [`Shot`] from it. `None` from every
    /// LOCAL trigger (`shooting::fire`, the sandbox): the sim cannot read the fire tick, so a
    /// locally-fired attributed shell — the server's authoritative shell AND the shooter's own
    /// predicted client shell — is stamped AFTER spawn by the shared `net::protocol::stamp_shot_ids`
    /// from its [`ShotSource`] + the timeline tick. Carrying [`Shot`] (either way) is what makes a
    /// net-client shell hold at armor contact for its bounce keyframe instead of fail-closing.
    pub shot: Option<ShotId>,
}

/// A shell in flight. Kinematic — integrated by hand, no physics engine.
#[derive(Component)]
pub(crate) struct Projectile {
    velocity: Vec3,
    caliber: f32,
    mass: f32,
    /// Quadratic-drag coefficient (1/m), from the shell's sectional density at spawn (see [`drag_k`]).
    drag_k: f32,
}

#[cfg(test)]
impl Projectile {
    /// Test-only 88-shaped shell for CROSS-MODULE tests (`net::protocol`'s `stamp_shot_ids` test needs
    /// a `Projectile`-carrying entity; the fields stay module-private for everyone else).
    pub(crate) fn test_88(velocity: Vec3) -> Self {
        Self {
            velocity,
            caliber: 0.088,
            mass: 10.2,
            drag_k: drag_k(0.088, 10.2),
        }
    }
}

/// The shell's flight path, accumulated one point per step — the data the sandbox's tracer gizmo
/// draws. Public so inspection tooling can read it; the game simply doesn't draw it. The growing
/// `Vec` is freed when the shell despawns on impact.
#[derive(Component, Default)]
pub struct ShellPath {
    pub points: Vec<Vec3>,
}

/// A ballistic volume: a solid the penetrator marches *through*, taxing it over the geometric
/// line-of-sight distance (the unified primitive — armor plates and modules alike, design doc §2).
/// On the `Armor` layer. `material_factor` (density/hardness → reference-mm per metre) is authored;
/// the march doesn't spend it yet — that is the next increment.
#[derive(Component)]
pub struct BallisticVolume {
    pub material_factor: f32,
}

/// Role tags layered on a ballistic volume for the sandbox's visibility passes: armor plates vs
/// internal components (modules / crew / ammo). Attached at bind alongside `BallisticVolume`; the
/// game ignores them.
#[derive(Component)]
pub struct ArmorVolume;

#[derive(Component)]
pub struct ComponentVolume;

/// A component's HP pool (crew/module/ammo). A spall fragment deposits 1; the main penetrator
/// transiting deposits many (scaled by the cost it paid crossing — design §6). `current` clamps at
/// 0; the *consequences* of reaching 0 (cookoff, crew death, knock-out) are later increments (§§7–8).
#[derive(Component)]
pub struct ComponentHealth {
    pub current: f32,
    pub max: f32,
}

/// One crossing of a ballistic volume by the penetrator: where it entered and exited the solid.
/// `(exit - entry).length()` is the geometric line-of-sight thickness — slope captured by geometry,
/// no cosine term (design doc §2).
pub struct PenetrationEvent {
    pub entry: Vec3,
    pub exit: Vec3,
    /// Whether this crossing was an overmatch (calibre ≫ plate thickness): ricochet suppressed,
    /// slope largely cancelled.
    pub overmatched: bool,
}

/// The volume crossings a shell has made this flight — what the sandbox draws to inspect the march.
/// Public, like `ShellPath`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct PenetrationMarks {
    pub events: Vec<PenetrationEvent>,
    /// Points where the shell ricocheted off a too-oblique face (deflected, did not enter).
    pub ricochets: Vec<Vec3>,
}

/// A single spall fragment's trace: where it stopped, and whether it deposited HP (hit a component)
/// or merely shadowed / flew on (hit armor or air). Carries 1 HP; no penetration of its own (§5).
pub struct SpallFragment {
    pub end: Vec3,
    pub deposited: bool,
}

/// One spall event — the cone thrown from a perforation exit. Origin + axis + half-angle describe
/// the fixed-shape cone; `fragments` are the resolved rays the sandbox draws.
pub struct SpallBurst {
    pub origin: Vec3,
    pub axis: Dir3,
    pub half_angle: f32,
    pub fragments: Vec<SpallFragment>,
}

/// The spall a shell has thrown this flight — one burst per perforation exit. Public like
/// `PenetrationMarks`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct SpallMarks {
    pub bursts: Vec<SpallBurst>,
}

/// Live per-shell readout for the sandbox's info layer — current speed (m/s) and remaining
/// penetration capability (reference-mm). Public; refreshed each step.
#[derive(Component, Default)]
pub struct ShellReadout {
    pub speed: f32,
    pub capability: f32,
}

/// Calibre boundary between the two shell VISUALS (`on_fire_shell`). At or above this the round keeps
/// the `shell.glb` scene (the 88, 0.088 m — its own glow/trail dressing is a later slice); below it a
/// round is MG-calibre (7.9 mm) and renders as a tracer streak (or nothing, if it's not a tracer).
/// 20 mm (the autocannon line) cleanly separates the Tiger's armament and reads as a real boundary.
/// The projectile entity carries no weapon identity, so the visual keys off `caliber` — the physical
/// signal already on `FireShell`; a future per-weapon visual style would replace this heuristic.
/// `pub(crate)` so the view layer's 88 dressing (`vfx::muzzle`) gates on the SAME boundary as the
/// shell-scene branch below, rather than a second constant that could drift.
pub(crate) const TRACER_MAX_CALIBER: f32 = 0.02;

/// A remote shot older than this many fixed catch-up ticks (~250 ms at 64 Hz) is stale: its flash
/// moment is long over on the shooter's screen, so the cosmetic reads it would still fire late — the
/// muzzle dressing AND the catch-up impact phantom (`on_fire_shell`) — are suppressed rather than
/// erupted late from bare ground (a full-scale splash + a multi-second ground scar with no shell
/// attached reads as a phantom). `pub(crate)` so the sim-side catch-up gate here and the view-side
/// muzzle gate (`vfx::muzzle`) share ONE constant and can never drift apart. Damage is unaffected —
/// the shell resolved on the authority; this gates only the cosmetic catch-up read.
pub(crate) const STALE_FIRE_TICKS: u32 = 16;

/// View marker on a tracer round's emissive streak child (`on_fire_shell`). The streak is a VIEW
/// attachment on the cosmetic projectile entity (ADR-0014) — it carries no sim state; it just rides
/// the projectile's `Transform`, which `integrate_projectiles` keeps pointed down the velocity.
///
/// `nominal_len` is the full streak length (≈ one render frame of travel). The view layer
/// ([`crate::vfx`]'s tracer clamp) shortens the drawn streak to the distance the round has actually
/// flown since the muzzle or the last ricochet, so the tail never pokes back through the turret or a
/// bounce point; past `nominal_len` of travel the clamp is a no-op and the full streak shows.
#[derive(Component)]
pub struct TracerStreak {
    pub nominal_len: f32,
}

impl TracerStreak {
    /// THE definition of the streak child's local transform for a round that has flown `flown` metres
    /// since its anchor (the muzzle, or its most recent ricochet).
    ///
    /// The unit capsule is authored along its local +Y; this rotates that axis onto the parent's −Z
    /// (the travel axis — `integrate_projectiles` keeps the projectile `look_to(velocity)`), scales Y
    /// to the drawn length, and pushes the capsule half a length back along +Z, so the head rides the
    /// round and the tail trails it. The drawn length is clamped to `flown`, so the tail can never poke
    /// back through the muzzle/turret or behind a bounce point; past `nominal_len` the clamp is a no-op
    /// and the full streak shows.
    ///
    /// **One definition, two callers, deliberately.** The spawn ([`on_fire_shell`]) seeds the child with
    /// this, and the view layer's per-frame maintainer (`vfx::tracer::clamp_tracer_streaks`) re-derives
    /// it every frame as the round flies and re-anchors it at each ricochet. The spawn MUST clamp for
    /// itself: a shell born in `Update` (a net observer's — `net::client::receive_fire_events` re-raises
    /// `FireShell` at render rate) materializes at that schedule's command flush, i.e. AFTER the
    /// `Update` maintainer has already run, so its first rendered frame draws whatever the spawn wrote.
    /// A shell born in `FixedUpdate` (a locally-fired one) is clamped before it is ever drawn. Deriving
    /// the seed from anything but this function is what let the two paths silently disagree.
    pub(crate) fn drawn_transform(&self, flown: f32) -> Transform {
        let len = self.nominal_len.min(flown).max(0.0);
        Transform {
            translation: Vec3::Z * (len * 0.5),
            rotation: Quat::from_rotation_arc(Vec3::Y, Vec3::NEG_Z),
            scale: Vec3::new(1.0, len, 1.0),
        }
    }
}

/// Preloaded tracer-streak view assets (mesh + emissive material), built once so a tracer round clones
/// handles rather than rebuilding them per shot — the streak twin of [`ProjectileAssets`].
#[derive(Resource)]
struct TracerAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// Preloaded shell scene, cloned per shot rather than loaded each time.
#[derive(Resource)]
struct ProjectileAssets {
    scene: Handle<WorldAsset>,
}

/// What the round struck — the surface discriminator the view read branches on (armor is
/// categorically NOT dirt: spark-on-steel + spall vs a dirt splash). Resolved sim-side where the
/// hit's volume ancestry is known (`hit_ancestor` ⇒ `Armor`, else `Terrain`). Kept lean: wood/snow/
/// etc. are deferred until terrain carries material tags. Local-only, like the rest of [`Impact`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ImpactSurface {
    Terrain,
    Armor,
}

/// A shell hit something — the seam the armor penetration march/spall and impact VFX hang off. The
/// struck entity is available from the raycast; add it here when a feature needs it. Global event
/// (the shell despawns), handled by the sim-side `on_impact` observer; the dev-only debug marker
/// (`debug::spawn_impact_marker`) and the sandbox subscribe to the same event. Local, never
/// replicated — growing it is not a wire change.
#[derive(Event)]
pub(crate) struct Impact {
    pub(crate) position: Vec3,
    /// Outward surface normal at the hit, straight from the raycast (for an embedded round, the
    /// ENTRY face's normal — the surface a viewer sees the round strike). View consumers kick
    /// sparks/dust along it (`vfx::impact`); not guaranteed unit-length in degenerate hits, so
    /// consumers normalize with a fallback.
    pub(crate) normal: Vec3,
    /// The striking round's caliber (m) — the physical scale the view read branches on: the MG's
    /// tiny 0.0079 m spark-ping vs the 88's 0.088 m dirt fountain (`vfx::impact` splits on
    /// [`TRACER_MAX_CALIBER`]). Already in scope at every trigger (rides `FireShell`), so carrying
    /// it here costs nothing; `Impact` stays local-never-replicated, so this is NOT a wire change.
    pub(crate) caliber: f32,
    /// What was struck — armor (spark-on-steel + spall + optional flame) vs terrain (dirt splash).
    /// The view's second branch axis after caliber. Resolved from the hit's volume ancestry.
    pub(crate) surface: ImpactSurface,
    /// Whether the round bit INTO steel (a defeated embed OR a clean perforation) as opposed to
    /// bouncing off (ricochet) or striking terrain. Gates the armor read's brief flame lick — the
    /// hot metal signature of the round burying itself in the plate. `false` for terrain, ricochet,
    /// the MG short-circuit, and the cosmetic catch-up phantom.
    pub(crate) penetrated: bool,
    /// For a ricochet, the outgoing deflected travel direction — the view biases the armor spark fan
    /// along it (a bounce throws its sparks the way it deflected). `None` for every non-ricochet hit
    /// (the fan then splays symmetrically off the surface normal).
    pub(crate) deflection: Option<Vec3>,
}

/// One crossing's share of a shell's momentum, handed to the struck volume's owning body:
/// `impulse = m·(v_in − v_out)`, applied at the crossing's entry `point`. The `on_hit_impulse`
/// observer applies it — so a hit *rocks* the tank in proportion to the momentum it actually
/// absorbed (a shell that stops shoves it most; a clean overpenetration barely nudges it).
#[derive(Event)]
struct HitImpulse {
    body: Entity,
    impulse: Vec3,
    point: Vec3,
}

/// Tags a debug impact marker so the observers that spawn them (`debug::spawn_impact_marker` in the
/// game client, the sandbox's own) can ring-buffer/clear them. The marker mesh/material and the
/// spawn itself are view concerns and live with those observers, not in this sim module (ADR-0014).
#[derive(Component)]
pub struct ImpactMarker;

/// EXPERIMENTAL measurement A/B lever (`SPIKE_MG_SHORTCIRCUIT`, default OFF): the B-arm of the
/// machine-gun-march cost attribution. When armed, [`integrate_projectiles`] still free-flies each
/// sub-20 mm round and still fires its per-step world `cast_ray` — but the FIRST surface the ray hits
/// (terrain OR armor) STOPS the round outright, skipping the whole penetration-resolution march:
/// the thickness probe, the span probe, the ricochet/normalize logic, the spall sub-casts, and all HP
/// deposition / hit-impulse. So round LIFETIME and population are identical between the two arms (both
/// despawn on first contact), and the A−B tick-cost delta isolates the cost of the resolution
/// machinery beyond the base ray. Sub-20 mm only (the 7.9 mm MGs; the 88 mm keeps the full march
/// unconditionally), so the main gun's authoritative damage is never touched. This CHANGES sim
/// behaviour (an MG deposits no damage in the B-arm) — it is a measurement knob, never a shipping
/// path, hence default-off and gated behind an obvious env var.
#[derive(Resource, Clone, Copy, Default)]
pub struct MgShortCircuit(pub bool);

/// Caliber ceiling (m) the [`MgShortCircuit`] B-arm applies to — the 7.9 mm MGs (0.0079 m) fall well
/// under it, the 88 mm (0.088 m) well over, so the lever can never touch the main gun's march.
const MG_SHORTCIRCUIT_CALIBER_MAX: f32 = 0.020;

pub fn plugin(app: &mut App) {
    app.init_resource::<RetainSpentShells>()
        .init_resource::<MarchMode>()
        .insert_resource(MgShortCircuit(
            std::env::var("SPIKE_MG_SHORTCIRCUIT").is_ok(),
        ))
        .add_observer(on_fire_shell)
        .add_observer(on_impact)
        .add_observer(on_hit_impulse)
        .add_systems(Startup, setup_assets)
        // The same march, integrated on whichever clock the mode selects: `Real` on the fixed
        // server step (`Res<Time>` is `Time<Fixed>` here), `Demo` per-frame on virtual time
        // (`Res<Time>` is `Time<Virtual>` here). One reads as the true sim, the other as smooth.
        .add_systems(
            FixedUpdate,
            integrate_projectiles.in_set(GameplaySet).run_if(march_real),
        )
        .add_systems(
            Update,
            integrate_projectiles.in_set(GameplaySet).run_if(march_demo),
        );
}

fn setup_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Preload once; firing clones the handle rather than hitting the asset server per shot.
    commands.insert_resource(ProjectileAssets {
        scene: asset_server.load(GltfAssetLabel::Scene(0).from_asset("shell/shell.glb")),
    });
    // The tracer streak: a thin UNIT capsule authored along its local +Y. The per-shot child
    // transform (`on_fire_shell`) rotates that axis onto the shell's local −Z (its travel axis — the
    // projectile `Transform` is kept `look_to(velocity)` by `integrate_projectiles`) and scales the
    // length to ≈ one frame of travel, so it reads as a hot round with a trailing tail, not a box.
    let mesh = meshes.add(Capsule3d::new(0.018, 1.0));
    // The EMISSIVE IS THE WHOLE VISUAL: black base + zero reflectance kill every lit contribution,
    // so the streak renders exactly its emissive — which rides far above 1.0 in linear space, where
    // the HDR camera's `Bloom` (camera.rs) halos it and the tonemapper rolls the over-bright core to
    // white-hot for free. Do NOT set `unlit: true` here: StandardMaterial's unlit path outputs
    // `base_color` alone and IGNORES `emissive`, which rendered the old streak as a flat sRGB
    // "square sausage" that bloom never caught. Warm orange; magnitude tunes against bloom intensity.
    let material = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        reflectance: 0.0,
        emissive: LinearRgba::rgb(30.0, 12.0, 3.0),
        ..default()
    });
    commands.insert_resource(TracerAssets { mesh, material });
}

/// Spawn a shell from a `FireShell`: at the origin, oriented down the bore, with velocity along the
/// bore at the muzzle speed. For a net catch-up shell (`fire.catch_up_ticks > 0`) first fast-forward it
/// to OUR predicted present, where it is co-indexed with our own hull (see
/// `net::protocol::FireEvent::fire_tick`); `catch_up_ticks` is `0` for every locally-fired shell, so
/// that path is skipped and the shell spawns at the muzzle exactly as before (local shells unaffected).
///
/// **Hits during catch-up.** Under the predicted-present timeline the skip is systematic — MEASURED
/// ≈4 ticks / ~49 m at RTT ≈ 91 ms, growing with RTT (`design/timelines-and-shear.md` §2) — so a
/// close-range shot can catch up PAST its target. If the round flew into terrain
/// or a hull during the skipped flight it already impacted on the authority — there is nothing left in
/// the air, so we skip the phantom tracer rather than spawn it downrange of the surface it hit. That
/// test is ONE straight-segment raycast (`Terrain | Armor`): the catch-up arc's gravity drop over a few
/// ticks is sub-metre, so the muzzle→caught-up segment tracks the true arc. It is deliberately NOT a
/// per-tick penetration march — the client deposits no HP or impulse here (`ClientReplica`), so the
/// full march would resolve nothing the server hasn't; reusing it would only thread the volume / health
/// / spall machinery into the spawn path for a purely cosmetic shell. A skipped shot still registers:
/// barrel recoil is enqueued independently (`net::client::receive_fire_events`) and damage is
/// server-authoritative.
fn on_fire_shell(
    fire: On<FireShell>,
    assets: Res<ProjectileAssets>,
    tracer_assets: Res<TracerAssets>,
    // The FIXED timestep, NOT `Res<Time>`: this observer can fire from `Update` (the net client
    // re-raises `FireShell` at render rate), where `Res<Time>` is `Time<Virtual>` (a render-frame dt).
    // The catch-up counts fixed SERVER ticks, so it must step the fixed timestep the live march also
    // uses in `Real` mode. Unused when `catch_up_ticks == 0` (the loop never runs).
    fixed_time: Res<Time<Fixed>>,
    // The already-landed test below; inert for a local shell (guarded on `catch_up_ticks > 0`).
    spatial: SpatialQuery,
    // Volume ancestry, to classify the catch-up hit's surface (armor vs terrain) the same way the
    // live march does (`hit_ancestor`). Cheap to thread through the observer; only read when a
    // catch-up hit actually lands (guarded on `catch_up_ticks > 0`).
    volumes: Query<&BallisticVolume>,
    // Volume OWNERSHIP, for the shooter self-exclusion the already-landed test needs (see
    // [`not_own_volume`]): a muzzle that sits inside its own tank's geometry — the coax, whose
    // recoiling barrel retracts its muzzle behind the STATIC mantlet on every round after the first —
    // would otherwise report "already landed" 1 cm out and swallow the shell whole.
    owners: Query<&VolumeOf>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let drag = drag_k(fire.caliber, fire.mass);
    let dt = fixed_time.timestep().as_secs_f32();
    let (position, velocity, points) = fast_forward_shell(
        fire.origin,
        fire.direction * fire.speed,
        drag,
        dt,
        fire.catch_up_ticks,
    );

    // Net catch-up only: if the round already flew into terrain or a hull during the skipped flight it
    // impacted on the authority — skip the phantom in-flight tracer (see the doc). One segment raycast,
    // started a hair off the muzzle (matching the live march's `+ dir*EPS`) so a muzzle flush with a
    // collider face can't self-trip it.
    if fire.catch_up_ticks > 0 {
        let skipped = position - fire.origin;
        if let Ok(dir) = Dir3::new(skipped) {
            const EPS: f32 = 1.0e-3;
            let filter = SpatialQueryFilter::from_mask(
                LayerMask::from(Layer::Terrain) | LayerMask::from(Layer::Armor),
            );
            // The shooter's own volumes are transparent to its own round — the same rule the live
            // march applies (see [`not_own_volume`]). Without it this segment, which starts AT the
            // muzzle, reports the shooter's own mantlet as an already-landed hit and the observer's
            // shell is never spawned.
            let shooter = fire.shooter.map(|source| source.tank);
            let not_own = |entity: Entity| not_own_volume(entity, shooter, &owners, &parents);
            let reach = (skipped.length() - EPS).max(0.0);
            if let Some(hit) = spatial.cast_ray_predicate(
                fire.origin + Vec3::from(dir) * EPS,
                dir,
                reach,
                true,
                &filter,
                &not_own,
            ) {
                // The round already landed during the skipped flight — no in-flight tracer, but the
                // IMPACT still reads: spark the same view-side `Impact` seam a live march would have
                // (the dust billow + sparks, `vfx::impact`), where the segment says it hit. Without
                // this, close-range remote fire (whose whole flight fits inside the catch-up skip)
                // shows nothing at all on the observing client.
                //
                // STALENESS GATE (shares `STALE_FIRE_TICKS` with the muzzle dressing so the flash and
                // this impact phantom fall stale together): past the bound the flash moment is long
                // over on the shooter's screen, and erupting a full-scale splash + a multi-second
                // ground scar late from bare ground — with no shell or muzzle flash attached — reads as
                // a phantom (the catch-up accepts up to `CATCH_UP_MAX_TICKS` = 100, a ~1.5 s stall).
                // So we still `return` (the shell landed on the authority — no in-flight tracer
                // either), but suppress the cosmetic read. Damage is unaffected.
                if fire.catch_up_ticks <= STALE_FIRE_TICKS {
                    // Surface is resolved properly from the hit's volume ancestry (armor plate ⇒
                    // Armor, else Terrain); penetration is unknown in this cosmetic-phantom context
                    // (no march ran), so `penetrated: false` — a catch-up armor read shows the
                    // spark/spall but never the flame lick.
                    let surface = if hit_ancestor(hit.entity, &volumes, &parents).is_some() {
                        ImpactSurface::Armor
                    } else {
                        ImpactSurface::Terrain
                    };
                    commands.trigger(Impact {
                        position: fire.origin + Vec3::from(dir) * (EPS + hit.distance),
                        normal: hit.normal,
                        caliber: fire.caliber,
                        surface,
                        penetrated: false,
                        deflection: None,
                    });
                }
                return;
            }
        }
    }

    // Travel direction after any catch-up (gravity/drag bend it); fall back to the bore for a
    // degenerate zero velocity so a spent-to-rest catch-up never trips `Dir3`.
    let travel = Dir3::new(velocity).unwrap_or(fire.direction);
    let speed = velocity.length();
    // The sim shell is IDENTICAL for every round — it flies and raycasts the same whether or not it is
    // visible (a non-tracer MG round still bounces, ricochets, and lands; dead-reckoned streaks were
    // rejected). Only the ATTACHED VISUAL differs, gated below at the RENDER layer.
    let mut shell = commands.spawn((
        Projectile {
            velocity,
            caliber: fire.caliber,
            mass: fire.mass,
            drag_k: drag,
        },
        ShellPath { points },
        PenetrationMarks::default(),
        SpallMarks::default(),
        ShellReadout {
            speed,
            capability: capability(fire.mass, speed),
        },
        // Root visibility so an attached streak child inherits it (harmless on the shell-scene path).
        Visibility::default(),
        Transform::from_translation(position).looking_to(travel, Vec3::Y),
    ));

    // Network identity, when the shot has one. An OBSERVER shell carries its wire [`Shot`]
    // (keyframe-eligible — it re-seeds at armor contact); any attributed locally-fired shell carries
    // its [`ShotSource`] so the shared `net::protocol::stamp_shot_ids` can complete the `Shot` after
    // spawn from the fire tick — on the server AND the shooter's own client alike. Inserted here (not
    // the spawn tuple, which is already at the bundle-arity ceiling) — flushed with the spawn, and the
    // shell is never marched until a later schedule point, so it never runs the march without these.
    if let Some(shot) = fire.shot {
        shell.insert(Shot(shot));
    }
    if let Some(source) = fire.shooter {
        shell.insert(source);
    }

    // Visual policy (interim — a per-weapon visual style would supersede the caliber split):
    //   * Main-gun-calibre round (the 88): keep the `shell.glb` scene. Its own glow/trail dressing is
    //     a separate upcoming slice; the tracer flag doesn't drive its look yet.
    //   * MG-calibre TRACER round: an emissive streak child, elongated along velocity.
    //   * MG-calibre NON-tracer round: NO visual entity at all — it still flew above (raycast + all),
    //     it's just invisible (a future "wake/trace through optics" effect will dress these).
    if fire.caliber >= TRACER_MAX_CALIBER {
        shell.insert(WorldAssetRoot(assets.scene.clone()));
    } else if fire.tracer {
        // Streak length ≈ one render frame of travel, so successive frames fuse into a continuous
        // line (floored so a slow, spent round still reads as a streak).
        let streak = TracerStreak {
            nominal_len: (speed * 0.018).max(2.0),
        };
        // SEED THE STREAK ALREADY CLAMPED to what the round has flown since the muzzle — for a local
        // shell that is 0 (`catch_up_ticks == 0`, so it draws nothing until it moves); for a net
        // observer's it is the catch-up distance. The clamp cannot be left to the view layer's
        // per-frame maintainer alone: an observer's shell is born in `Update`, i.e. after that
        // maintainer has already run, so its FIRST rendered frame draws exactly what we write here.
        // Seeding the full `nominal_len` is what dragged a ~13 m tail back through the shooter's
        // turret on every remote MG round. See [`TracerStreak::drawn_transform`].
        let flown = position.distance(fire.origin);
        let transform = streak.drawn_transform(flown);
        shell.with_child((
            Mesh3d(tracer_assets.mesh.clone()),
            MeshMaterial3d(tracer_assets.material.clone()),
            transform,
            // A light streak neither casts nor receives shadow — without these the sun dragged a
            // long capsule shadow across the terrain under every tracer.
            NotShadowCaster,
            NotShadowReceiver,
            streak,
        ));
    }
}

fn integrate_projectiles(
    mut projectiles: Query<(
        Entity,
        &mut Transform,
        &mut Projectile,
        &mut ShellPath,
        &mut PenetrationMarks,
        &mut ShellReadout,
        &mut SpallMarks,
        // OBSERVER-only, both `Option`: `Shot` is the shell's network identity (keyframe-eligible when
        // present), `Held` marks a shell frozen at armor waiting for its sanctioned bounce keyframe.
        Option<&Shot>,
        Option<&mut Held>,
        // The tank that fired this shell, when it was fired BY one — `on_fire_shell` attaches it from
        // `FireShell::shooter` on every attributed shot (the authority's, the shooter's own predicted
        // round, AND an observer's replica). Read here ONLY to exclude that tank's own volumes from the
        // march ([`not_own_volume`]) — never to deposit anything. `None` for the sandbox's tank-less
        // camera fire, which excludes nothing.
        Option<&ShotSource>,
    )>,
    volumes: Query<&BallisticVolume>,
    owners: Query<&VolumeOf>,
    mut health: Query<&mut ComponentHealth>,
    parents: Query<&ChildOf>,
    retain: Res<RetainSpentShells>,
    // Present only on the net client (a replica): shells still fly, raycast, spark, and despawn, but
    // HP deposition and hit impulse are the server's authority. Absent in SP / sandbox / server.
    replica: Option<Res<ClientReplica>>,
    // The observer's server-sanctioned bounce buffer (`net::client` fills it). Present only on a net
    // client; absent in SP/sandbox/server, where the authority march resolves bounces for real and
    // never consults it. An observer shell at armor contact re-seeds from here or holds for it.
    sanctioned: Option<Res<SanctionedShots>>,
    // F1: set (to `true`) only while lightyear is REPLAYING a rollback on a net client. The whole
    // cosmetic march below is skipped on a replayed tick — see the early return. Absent on the
    // authority (never rolls back).
    replaying: Option<Res<Replaying>>,
    // F3: this net client's predicted present tick `P` (every cosmetic shell lives at `P`). Read to
    // consume an OVERDUE sanctioned outcome for a shell that MISSED the plate the server resolved on.
    // Present only on a net client; absent on the authority (it resolves shots for real).
    present: Option<Res<PredictedPresent>>,
    // EXPERIMENTAL cost-attribution A/B lever (`SPIKE_MG_SHORTCIRCUIT`, default off — see the type).
    shortcircuit: Res<MgShortCircuit>,
    // Sim-cost recorder attribution sink (`SPIKE_COST_TRACE`): absent unless the recorder is armed, so
    // an unmeasured run pays only the `Option` check. This system's whole wall-time is stamped into it.
    mut cost: Option<ResMut<crate::cost::CostTrace>>,
    spatial: SpatialQuery,
    time: Res<Time>,
    mut commands: Commands,
) {
    // F1 (rollback-safe cosmetics): on a net client, lightyear replays FixedMain N times per
    // rollback. Every shell this system marches is VIEW-ONLY (`deposit == false` — HP and impulse are
    // the server's authority; see `ClientReplica`) and its picture must advance exactly ONE step per
    // FORWARD tick. Re-marching on each replayed tick would teleport every in-flight shell forward by
    // the rollback depth (with duplicate `ShellPath` points) and age every `Held` shell one extra
    // tick per replay — burning the grace window in a single frame and corrupting the
    // `present − bounce_tick` re-seed arithmetic that `Held` depends on. So skip the whole march on a
    // replayed tick; the shells resume untouched on the next forward tick. The DETERMINISTIC sim state
    // a rollback exists to correct (`TankSim`, physics) is not here — it re-runs in `GameplaySet`
    // normally. The authority (server/SP/sandbox) never sets `Replaying`, so it is never skipped.
    if replaying.is_some_and(|r| r.0) {
        return;
    }
    // March-cost attribution timer (`SPIKE_COST_TRACE`): only sampled when the recorder is armed, so an
    // unmeasured run never touches the clock. Covers the whole march (query iteration + every cast).
    let march_t0 = cost.as_ref().map(|_| Instant::now());
    let dt = time.delta_secs();
    // Authority = not a replica: only then does a hit actually mutate health here.
    let deposit = replica.is_none();
    // F3: the predicted present tick, if this is a net client — the clock the overdue-consumption
    // check below compares each sanctioned outcome's server tick against. Absent on the authority.
    let present = present.map(|p| p.0);
    // The march casts against terrain (which stops the shell) and ballistic volumes (which it
    // crosses); the struck entity being a `BallisticVolume` is what tells the two apart.
    let world = SpatialQueryFilter::from_mask(
        LayerMask::from(Layer::Terrain) | LayerMask::from(Layer::Armor),
    );
    let armor = SpatialQueryFilter::from_mask(Layer::Armor);
    // Nudge past each boundary we resolve so we don't immediately re-hit it.
    const EPS: f32 = 1.0e-3;
    // How far ahead to search for a volume's far face — its full geometric thickness, even past the
    // end of this step (thin plates resolve well within it).
    const PROBE: f32 = 50.0;
    // Steeper than this from the surface normal, an un-overmatched round ricochets (rad, ~70°).
    const RICOCHET_ANGLE: f32 = 1.221;
    // Speed retained through a ricochet.
    const RICOCHET_BLEED: f32 = 0.6;
    // Shock a glancing bounce jars into an *exposed component* (not armor): scaled by impact energy
    // (capability) × squareness (cos incidence). A graze chips structural integrity without one-
    // shotting; a faint graze barely registers; small arms barely scratch. Armor has no HP → shrugs.
    const SHOCK_K: f32 = 0.045;
    // Share of the impact angle the round straightens toward the normal on entry (normalization).
    const NORMALIZATION: f32 = 0.2;
    // Overmatch when calibre ≥ this × the plate's thickness: ricochet suppressed, slope cancelled.
    const OVERMATCH_RATIO: f32 = 3.0;
    // Spall (design §5). Budget = (material chewed / ref) × (residual energy / ref) × (caliber /
    // ref), capped — both a fragment supply (cost) and a push (v_res²) are needed, so a thin/soft
    // body or a barely-through round throws little. The cone's shape is fixed; only density scales.
    const SPALL_MAX_FRAGMENTS: usize = 24;
    const SPALL_COST_REF: f32 = 100.0; // ref-mm (≈ a 100 mm steel plate)
    const SPALL_VRES_REF: f32 = 500.0; // m/s
    const SPALL_CALIBER_REF: f32 = 0.088; // m (the 88)
    const SPALL_HALF_ANGLE: f32 = 0.35; // rad (~20°)
    const SPALL_RANGE: f32 = 6.0; // m — fragments are short-range
    // Main-penetrator transit damage = cost paid crossing the component × this (design §6).
    const TRANSIT_K: f32 = 1.0;

    for (
        entity,
        mut transform,
        mut projectile,
        mut path,
        mut marks,
        mut readout,
        mut spall,
        shot,
        held,
        source,
    ) in &mut projectiles
    {
        // SHOOTER SELF-EXCLUSION (see [`not_own_volume`]): this round is transparent to the tank that
        // fired it, for every cast below. Identical on the authority and on a replica — the one place
        // both ends must agree about the shooter's own geometry, or the server's damage model and the
        // client's cosmetic model describe different worlds.
        let shooter = source.map(|source| source.tank);
        let not_own = |entity: Entity| not_own_volume(entity, shooter, &owners, &parents);
        // NET-CLIENT HOLD: a `Shot`-carrying shell — an observer's replica OR the shooter's own
        // predicted round — frozen (and hidden, see the hold insert below) at armor contact, waiting
        // the grace window for the server's verdict on this contact. It does NOT free-flight while
        // held — it resolves from whichever sanctioned outcome arrives (a bounce keyframe → re-seed
        // and continue; a terminal confirm → the full honest armor read at the server's position) or,
        // past the window, degrades to fail-closed truncation. The authority never holds (it resolves
        // shots for real), and a shell with no `Shot` never enters this state (it fail-closes on
        // first contact below).
        if let Some(mut held) = held {
            // The bounce we are waiting on (the next unconsumed ordinal for this shot), if it arrived.
            let arrived = shot
                .zip(sanctioned.as_ref())
                .and_then(|(s, buf)| buf.next(s.0, marks.ricochets.len()));
            if let Some(bounce) = arrived {
                // RE-SEED. Fast-forward the sanctioned post-bounce state by the ticks we held —
                // exactly present − bounce_tick, for the shooter's own shell and an observer's alike
                // (both live on the P timeline; the arithmetic is in `Held`'s doc) — through the same
                // integrator the initial `fire_tick` catch-up uses. The returned arc feeds `ShellPath`
                // so the trail runs seamlessly through the bounce; the bounce point re-anchors the
                // tracer clamp (`marks.ricochets`); the ember rides the same (re-shown) entity.
                let seed_vel = Dir3::new(bounce.direction)
                    .map_or(projectile.velocity, |d| Vec3::from(d) * bounce.speed);
                let (pos, vel, arc) =
                    fast_forward_shell(bounce.origin, seed_vel, projectile.drag_k, dt, held.ticks);
                for point in arc {
                    path.points.push(point);
                }
                marks.ricochets.push(bounce.origin);
                // The bounce now reads with its TRUE (server-sanctioned) directional spark fan,
                // biased along the deflected travel — the same read the authority showed.
                commands.trigger(Impact {
                    position: bounce.origin,
                    normal: held.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: false,
                    deflection: Some(bounce.direction),
                });
                transform.translation = pos;
                if let Ok(d) = Dir3::new(vel) {
                    transform.look_to(d, Vec3::Y);
                }
                projectile.velocity = vel;
                readout.speed = vel.length();
                readout.capability = capability(projectile.mass, vel.length());
                // Un-hide (the hold's invisible-stop) and resume marching next tick.
                commands
                    .entity(entity)
                    .remove::<Held>()
                    .insert(Visibility::Inherited);
                continue;
            }
            // The shot's TERMINAL (embed / perforation confirm), gated on every preceding bounce
            // having been consumed (`after_bounces` — a late bounce keyframe must re-seed first; the
            // buffer returns `None` until the counts match). Resolve NOW at the SERVER's position and
            // normal with the full honest armor read — `penetrated` gates the flame lick exactly as
            // the authority's own `Impact` did — typically ≈(P−S)+OWL after contact, well inside (and
            // instead of) the fail-closed window. The trail gets the server position so it reaches the
            // read point.
            let terminal = shot
                .zip(sanctioned.as_ref())
                .and_then(|(s, buf)| buf.terminal(s.0, marks.ricochets.len()));
            if let Some(terminal) = terminal {
                path.points.push(terminal.position);
                commands.trigger(Impact {
                    position: terminal.position,
                    normal: terminal.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: terminal.penetrated,
                    deflection: None,
                });
                commands.entity(entity).despawn();
                continue;
            }
            // Still waiting. Past the grace window, the shell degrades to the fail-closed fallback — a
            // dropped keyframe/confirm must never leave a round frozen forever, and correctness never
            // depends on delivery. Otherwise stay frozen this tick (no advance, no impact yet).
            held.ticks += 1;
            if held.ticks > RICOCHET_HOLD_TICKS {
                // F3(ii) — QUIET DISSOLVE, not a fabricated spark. A held shell that never received a
                // sanctioned outcome is one of two things and we cannot tell which: a genuinely LOST
                // verdict (the server DID resolve this contact, but every redundant confirm/keyframe
                // dropped) or a pose-divergent MISS (this client contacted a plate the server's round
                // sailed past — the interpolated target pose differs at grazing geometry). A neutral
                // spark here reads as a confirmed contact, but in the miss case the authority resolved
                // NO contact at this point — the spark fabricates geometry/verdict the server never
                // sanctioned, the exact thing invariant 2 and the honesty doctrine forbid. So end the
                // shell silently: the (already hidden) round is despawned and its trail simply stops at
                // the last free-flight point. No spark for a contact the authority never confirmed.
                // (A server-confirmed contact whose confirm merely arrived LATE is consumed by the
                // tick-triggered overdue path below or the pre-armed/hold paths — this fallback is only
                // reached when NO sanctioned outcome exists for the shot at all.)
                commands.entity(entity).despawn();
            }
            continue;
        }

        // F3: TICK-TRIGGERED CONSUMPTION of an overdue sanctioned outcome — the pose-divergence MISS
        // case. A net client flies its cosmetic shell against the interpolated (~100 ms-stale) target
        // pose; where that pose differs from the server's at grazing geometry (exactly where ricochets
        // live) the client shell can sail PAST the plate the server's round resolved on. It then never
        // contacts, so it neither pre-arms nor holds — the sanctioned bounce/terminal would age out
        // unconsumed and the observer would watch the round fly on through where the authoritative
        // round bounced or terminated (the precise divergence invariant 2 exists to kill, resurrected
        // by sub-metre pose lag). So once our predicted present has passed the sanctioned outcome's
        // server tick by `OVERDUE_MARGIN_TICKS` WITHOUT a local contact, consume it by the clock:
        // re-seed at the server bounce, or finalize at the server impact. This is also the seam a
        // future NON-CONTACT outcome — an airburst-HE detonation with no armor contact at all — slots
        // into: an outcome resolved by its tick, not by a ray. Net-client only (`!deposit`); the
        // authority resolves shots for real and never reads `present`.
        if !deposit
            && let (Some(shot), Some(buf), Some(present)) = (shot, sanctioned.as_ref(), present)
        {
            let consumed = marks.ricochets.len();
            if let Some(bounce) = buf.next(shot.0, consumed) {
                if present.saturating_sub(bounce.bounce_tick) > OVERDUE_MARGIN_TICKS {
                    // Re-seed onto server truth, re-aged forward to the present by exactly
                    // present − bounce_tick — the SAME catch-up the hold path applies (`Held`'s doc),
                    // here driven by the clock rather than a hold count. The shell jumps to the
                    // sanctioned bounce and flies on from there next tick; it is already visible (it
                    // never held), so there is no hidden state to clear. The spark reads with the
                    // sanctioned deflection fan; the surface normal is not on the keyframe wire, so it
                    // orients off the post-bounce direction (the same approximation the pre-armed path's
                    // client raycast stands in for).
                    let re_age = present - bounce.bounce_tick;
                    let seed_vel = Dir3::new(bounce.direction)
                        .map_or(projectile.velocity, |d| Vec3::from(d) * bounce.speed);
                    let (rpos, rvel, arc) =
                        fast_forward_shell(bounce.origin, seed_vel, projectile.drag_k, dt, re_age);
                    for point in arc {
                        path.points.push(point);
                    }
                    marks.ricochets.push(bounce.origin);
                    commands.trigger(Impact {
                        position: bounce.origin,
                        normal: bounce.direction,
                        caliber: projectile.caliber,
                        surface: ImpactSurface::Armor,
                        penetrated: false,
                        deflection: Some(bounce.direction),
                    });
                    transform.translation = rpos;
                    if let Ok(d) = Dir3::new(rvel) {
                        transform.look_to(d, Vec3::Y);
                    }
                    projectile.velocity = rvel;
                    readout.speed = rvel.length();
                    readout.capability = capability(projectile.mass, rvel.length());
                    continue;
                }
            } else if let Some(terminal) = buf.terminal(shot.0, consumed)
                && present.saturating_sub(terminal.impact_tick) > OVERDUE_MARGIN_TICKS
            {
                // Finalize at the server's read — position, normal, and the `penetrated` verdict
                // that gates the flame lick — the full honest armor read the authority resolved,
                // even though this client's shell never touched the (mis-posed) plate. The trail
                // reaches the server impact point, then the shell ends. (The `else if` is keyed on NO
                // bounce being owed: `buf.terminal` would return `None` anyway while a bounce's
                // keyframe is still in flight, by its `after_bounces` gate.)
                path.points.push(terminal.position);
                commands.trigger(Impact {
                    position: terminal.position,
                    normal: terminal.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: terminal.penetrated,
                    deflection: None,
                });
                commands.entity(entity).despawn();
                continue;
            }
        }

        // Advance free-flight (gravity + drag on the velocity, then the position step) through the
        // shared per-tick kernel — the SAME [`advance_shell`] the FireEvent catch-up folds, so a
        // caught-up shell and a natively-flown one can't diverge. `freeflight_pos` is this tick's
        // free-flight landing point; the ray-march below overrides it only where the segment hits
        // something. The march may *bend* the direction (normalization / ricochet), so we carry
        // direction + speed and rebuild the velocity at the end rather than assuming a straight step.
        let (freeflight_pos, stepped) = advance_shell(
            transform.translation,
            projectile.velocity,
            projectile.drag_k,
            dt,
        );
        let Ok(mut dir) = Dir3::new(stepped) else {
            continue;
        };
        let mut speed = stepped.length();
        let mut pos = transform.translation;
        let mut remaining = speed * dt;
        let mut stopped = false;
        // Set when an observer shell freezes at armor contact to await its bounce keyframe (see the
        // `!deposit` branch): the shell keeps its entity and velocity, and the `Held` handler above
        // drives the wait on subsequent ticks — distinct from `stopped` (which despawns).
        let mut holding = false;
        // Whether the march has bent the shell off its original free-flight segment. Until it does,
        // an open-air fly-out lands exactly on `freeflight_pos` (the shared advance); after a bend the
        // leftover budget flies along the new direction instead.
        let mut bent = false;
        // AUTHORITY: whether this shell's ONE terminal (`ShellTerminal` — embed/perforation) was
        // emitted THIS march step. A perforated shell keeps marching (interior crossings can embed the
        // same tick), and the deferred `Shot` removal below only lands at flush — this local flag is
        // the same-tick half of the at-most-one-terminal invariant, the strip the cross-tick half. It
        // also mutes post-terminal `ShellRicochet`s: the client's cosmetic shell ended at the terminal,
        // so an interior bounce after it must not ride the wire.
        let mut terminal_emitted = false;

        // Ray-march the step: free flight until a surface, then resolve it — terrain stops the
        // shell; a ballistic volume ricochets (too oblique) or is crossed (normalize → spend cost →
        // perforate or embed) — and keep marching the leftover budget along the new direction.
        while remaining > EPS {
            let origin = pos + dir * EPS;
            let Some(hit) =
                spatial.cast_ray_predicate(origin, dir, remaining, true, &world, &not_own)
            else {
                // Open air — fly out the rest of the step. On the original (unbent) segment this is
                // exactly the shared `advance_shell` landing point; a `continue` past this point only
                // ever follows a bend, so `bent` is the exact discriminant.
                pos = if bent {
                    pos + dir * remaining
                } else {
                    freeflight_pos
                };
                break;
            };
            let entry = origin + dir * hit.distance;
            let travelled = EPS + hit.distance;

            // EXPERIMENTAL B-arm (`SPIKE_MG_SHORTCIRCUIT`): a sub-20 mm round stops dead at the first
            // surface, skipping the entire penetration-resolution march below (thickness/span probes,
            // ricochet, spall, HP). Population-preserving — same despawn-on-contact as the live path —
            // so the A−B tick-cost delta isolates the resolution machinery. Default off (see the type).
            if shortcircuit.0 && projectile.caliber < MG_SHORTCIRCUIT_CALIBER_MAX {
                // Classify the first surface from the hit's volume ancestry (the same `hit_ancestor`
                // rule the full march uses just below) so the read is honest even in the B-arm. The
                // short-circuit stops dead without resolving penetration, so `penetrated: false`.
                let surface = if hit_ancestor(hit.entity, &volumes, &parents).is_some() {
                    ImpactSurface::Armor
                } else {
                    ImpactSurface::Terrain
                };
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: projectile.caliber,
                    surface,
                    penetrated: false,
                    deflection: None,
                });
                pos = entry;
                stopped = true;
                break;
            }

            // The struck `BallisticVolume` sits on the hit's ancestry (`hit_ancestor`, the shared
            // hierarchy-resolution rule), keeping the node entity so transit damage and spall can
            // address the component. No volume in the ancestry ⇒ terrain.
            let resolved = hit_ancestor(hit.entity, &volumes, &parents)
                .map(|(node, volume)| (node, volume.material_factor));
            let Some((node_entity, factor)) = resolved else {
                // Terrain: stop here.
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Terrain,
                    penetrated: false,
                    deflection: None,
                });
                pos = entry;
                stopped = true;
                break;
            };

            // NET-CLIENT ARMOR CONTACT (`!deposit`, i.e. `ClientReplica` present) — every cosmetic
            // shell this client flies: an observer's replica AND the shooter's own predicted round. A
            // client must NEVER re-simulate an authoritative armor collision against the
            // non-authoritative (interpolated, ~100 ms stale) target pose it renders — a round's real
            // fate at a plate (ricochet, perforate, embed) is the server's to decide. Deterministic
            // local flight is honest only UP TO the first surface; past it the client would improvise
            // a deflection the server never sanctioned and the cosmetic shell would wander off where
            // the authoritative round did not ("post-ricochet round wanders / trail is lost"). The
            // shot state machine at contact, in resolution order:
            //   1.  The server ALREADY sanctioned this shot's next bounce → RE-SEED from truth and
            //       keep marching the leftover budget (the exact shape of the authority ricochet
            //       branch below, so the trail and tracer clamp continue through the bounce
            //       identically). Opportunistic, not the norm: every client shell lives at the
            //       predicted present P, AHEAD of server-now, so first contact usually precedes the
            //       keyframe's arrival (the hold arithmetic in `Held`'s doc) — this pre-arms mainly
            //       when contact is late (target-motion pose lag, frame hitches) or on later bounces
            //       of a multi-bounce shot.
            //   1b. The shot's TERMINAL (embed/perforation `ImpactConfirm`) already arrived → resolve
            //       INSTANTLY at contact with the server's full honest armor read (`penetrated` gates
            //       the flame lick), no hold. Gated on `after_bounces` like the held path, so a
            //       terminal never fires while a bounce keyframe is still owed.
            //   2.  No verdict yet, but this shell HAS a network identity (`Shot` — observer shells
            //       from the wire, own shells via `net::protocol::stamp_shot_ids`) → HOLD: freeze
            //       hidden at contact (invisible-stop — no impact VFX, and no frozen round hanging on
            //       the plate while it waits), for the grace window (the `Held` handler at the top of
            //       the loop resolves bounce/terminal when it lands, or fail-closes past the window).
            //       THE EXPECTED PATH at first contact, ≈(P−S)+OWL ticks of hold.
            //   3.  No `Shot` at all (an unattributed shot; SP/sandbox never reach here — they are
            //       authorities) → FAIL CLOSED immediately: stop dead with a neutral armor spark (no
            //       bounce fan, no flame lick — the client cannot know the outcome), trail ends there.
            // Terrain already stops BOTH ends identically (static, pose-independent geometry) and
            // confirms nothing, so only this pose-dependent armor contact needs the guard. The
            // authority (`deposit == true`) runs the full resolution below unchanged. Correctness
            // NEVER depends on keyframe/confirm delivery: a dropped one degrades case 2 to the case-3
            // truncation. A shot thus ends in exactly one of {terrain stop (local), confirmed armor
            // terminal, fail-closed truncation} — every armor visual a net client renders is
            // server-sanctioned.
            if !deposit {
                let next_bounce = shot
                    .zip(sanctioned.as_ref())
                    .and_then(|(s, buf)| buf.next(s.0, marks.ricochets.len()));
                if let Some(bounce) = next_bounce {
                    // 1. PRE-ARMED re-seed onto server truth; keep marching the leftover budget. The
                    //    bounce reads with its TRUE directional spark (biased along the deflected
                    //    travel) — the client now knows the sanctioned outcome, so this is the real
                    //    ricochet fan, not the neutral fail-closed spark.
                    commands.trigger(Impact {
                        position: bounce.origin,
                        normal: hit.normal,
                        caliber: projectile.caliber,
                        surface: ImpactSurface::Armor,
                        penetrated: false,
                        deflection: Some(bounce.direction),
                    });
                    if let Ok(new_dir) = Dir3::new(bounce.direction) {
                        dir = new_dir;
                    }
                    speed = bounce.speed;
                    pos = bounce.origin;
                    bent = true;
                    path.points.push(bounce.origin);
                    marks.ricochets.push(bounce.origin);
                    remaining -= travelled;
                    continue;
                }
                // 1b. PRE-ARMED TERMINAL: the shot's confirmed embed/perforation already arrived
                //     (a late contact, or the confirm outran the shell) → resolve INSTANTLY at
                //     contact with the server's read — position, normal, and the `penetrated` flag
                //     that gates the flame lick — no hold, no hidden phase. Same `after_bounces`
                //     ordering gate as the held path.
                let terminal = shot
                    .zip(sanctioned.as_ref())
                    .and_then(|(s, buf)| buf.terminal(s.0, marks.ricochets.len()));
                if let Some(terminal) = terminal {
                    path.points.push(terminal.position);
                    commands.trigger(Impact {
                        position: terminal.position,
                        normal: terminal.normal,
                        caliber: projectile.caliber,
                        surface: ImpactSurface::Armor,
                        penetrated: terminal.penetrated,
                        deflection: None,
                    });
                    pos = entry;
                    stopped = true;
                    break;
                }
                if shot.is_some() {
                    // 2. HOLD for the verdict: invisible-stop (hidden so no round hangs frozen on the
                    //    plate — the shooter is watching this one), no impact yet; the `Held` handler
                    //    drives the wait, keeping the contact normal for the eventual bounce /
                    //    terminal / neutral spark.
                    commands.entity(entity).insert((
                        Held {
                            ticks: 0,
                            normal: hit.normal,
                        },
                        Visibility::Hidden,
                    ));
                    pos = entry;
                    holding = true;
                    break;
                }
                // 3. No identity — fail closed immediately (pre-slice behaviour).
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: false,
                    deflection: None,
                });
                pos = entry;
                stopped = true;
                break;
            }

            // Momentum bookkeeping for this crossing: the incoming velocity (before any bend/bleed)
            // and the body that owns the struck volume. Each resolution branch below hands the body
            // its share of the shell's momentum, `m·(v_in − v_out)` — a shell that stops dumps it all,
            // a perforation less (it carries momentum out), a ricochet a partial normal-ward kick.
            let v_in = Vec3::from(dir) * speed;
            let body = owners.get(node_entity).ok().map(|owner| owner.tank());

            // Outward surface normal; angle of incidence is measured from it (0 = head-on).
            let normal = Dir3::new(hit.normal).unwrap_or(-dir);
            let incidence = Vec3::from(dir).angle_between(-Vec3::from(normal));

            // Plate thickness *along its normal* (perpendicular, face to face) — the overmatch test:
            // a round whose calibre dwarfs the plate cannot be deflected by it.
            let thickness = spatial
                .cast_ray_predicate(
                    entry - Vec3::from(normal) * EPS,
                    -normal,
                    PROBE,
                    false,
                    &armor,
                    &|e| e == hit.entity,
                )
                .map(|back| EPS + back.distance)
                .unwrap_or(0.0);
            let overmatched = thickness > 0.0 && projectile.caliber >= OVERMATCH_RATIO * thickness;

            // Ricochet: too oblique → deflect off the face (no entry, no spall) — unless overmatch
            // suppresses it (design §4).
            if !overmatched && incidence > RICOCHET_ANGLE {
                // Shock: even a deflected hit jars an exposed component (barrel, optic) — scaled by
                // impact energy (capability) and how square the graze was. Armor has no HP, so it
                // shrugs the bounce off; a fragile module loses integrity without being one-shot.
                if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                    let shock = SHOCK_K * capability(projectile.mass, speed) * incidence.cos();
                    hp.current = (hp.current - shock).max(0.0);
                }
                dir = reflect(dir, normal);
                bent = true; // off the original free-flight segment (see the open-air break)
                speed *= RICOCHET_BLEED;
                if let Some(body) = body {
                    commands.trigger(HitImpulse {
                        body,
                        impulse: projectile.mass * (v_in - Vec3::from(dir) * speed),
                        point: entry,
                    });
                }
                // The bounce reads on the struck face: a hard bright spark fan, biased along the
                // deflected (outgoing) direction — a ricochet throws its sparks the way it kicked off.
                // It bit no steel, so `penetrated: false` (no flame lick — a bounce doesn't ignite).
                commands.trigger(Impact {
                    position: entry,
                    normal: Vec3::from(normal),
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: false,
                    deflection: Some(Vec3::from(dir)),
                });
                marks.ricochets.push(entry);
                path.points.push(entry);
                // AUTHORITY → CLIENTS: replicate this sanctioned bounce as the cause (ADR-0016) so
                // every client re-seeds its cosmetic shell from truth rather than improvising. Only
                // for a net-attributed shell (`Shot` present, stamped by the shared protocol stamp);
                // SP/sandbox shells carry none and raise nothing. Muted after this shot's terminal
                // (`!terminal_emitted`) — an interior bounce past a perforation is invisible to the
                // client shell, which ended at the terminal. `dir`/`speed` are already
                // post-reflect/bleed (the outgoing state); `entry` is the bounce point. `net::server`
                // observes this and stamps the bounce tick from its timeline. The ordinal is this
                // ricochet's 0-based index — the SAME count a client shell derives from its own
                // `ricochets` — so multi-bounce shots re-seed in order. (Only reachable on the
                // authority: the client `!deposit` branch above already broke before here.)
                if let Some(shot) = shot
                    && !terminal_emitted
                {
                    commands.trigger(ShellRicochet {
                        shot: shot.0,
                        origin: entry,
                        direction: Vec3::from(dir),
                        speed,
                        sequence: (marks.ricochets.len() - 1) as u32,
                    });
                }
                pos = entry;
                remaining -= travelled;
                continue;
            }

            // Normalize: a modest bend toward the inward normal as the round bites in (shortens the
            // path it cuts and nudges the exit). Overmatch does NOT bend it further — the round drives
            // through in roughly the same direction; overmatch instead cancels the *slope cost* below.
            dir = bend_toward(dir, -normal, NORMALIZATION * incidence);
            bent = true; // off the original free-flight segment (see the open-air break)
            let span = spatial
                .cast_ray_predicate(entry + dir * EPS, dir, PROBE, false, &armor, &|e| {
                    e == hit.entity
                })
                .map(|exit| EPS + exit.distance)
                .unwrap_or(0.0);

            // Cost = effective metres × the material's reference-mm-per-metre. An overmatched plate
            // can't present its oblique line-of-sight to a round that dwarfs it, so it charges only
            // the perpendicular thickness; otherwise the full slope span.
            let cap = capability(projectile.mass, speed);
            let effective = if overmatched { thickness } else { span };
            let cost = effective * factor;
            if cap <= cost {
                // Defeated: embed partway through (depth scaled by the capability it could pay).
                let embed = entry + dir * span * (cap / cost);
                marks.events.push(PenetrationEvent {
                    entry,
                    exit: embed,
                    overmatched,
                });
                path.points.push(embed);
                // It buried itself here, spending all it had (`cap`) — deposit that as transit damage
                // if the volume is a damageable component (design §6). No exit, so no spall.
                if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                    hp.current = (hp.current - cap * TRANSIT_K).max(0.0);
                }
                // The embed's visible face is the ENTRY surface — its normal is what sparks kick
                // off of (the embed point itself is inside the plate). The round buried itself in the
                // steel (`penetrated: true`): the hot-metal signature earns the brief flame lick, even
                // though the plate ultimately defeated it — it bit in, it didn't bounce.
                commands.trigger(Impact {
                    position: embed,
                    normal: Vec3::from(normal),
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: true,
                    deflection: None,
                });
                // AUTHORITY → CLIENTS: the shot's TERMINAL (`ImpactConfirm` on the wire) — the same
                // read the `Impact` above showed locally, so every client renders the identical honest
                // embed (position/normal/flame lick). The `!terminal_emitted` guard covers the
                // perforate-then-embed-same-tick chain (the perforation already terminated the shot);
                // no flag SET or `Shot` strip is needed here — the embedded shell stops and despawns,
                // so nothing after this can emit.
                if let Some(shot) = shot
                    && !terminal_emitted
                {
                    commands.trigger(ShellTerminal {
                        shot: shot.0,
                        position: embed,
                        normal: Vec3::from(normal),
                        penetrated: true,
                        after_bounces: marks.ricochets.len() as u32,
                    });
                }
                // Stopped: the body absorbs the full remaining momentum (v_out = 0).
                if let Some(body) = body {
                    commands.trigger(HitImpulse {
                        body,
                        impulse: projectile.mass * v_in,
                        point: entry,
                    });
                }
                pos = embed;
                stopped = true;
                break;
            }

            // Perforate: spend the cost (residual speed) and continue along the bent direction.
            // The struck FACE reads here — the entry point, its outward normal — where the round
            // punched through. This is the one place "penetrated" is unambiguously true (the round
            // breached the plate into the interior), so the armor read earns its flame lick. Without
            // this trigger a clean perforation was visually silent on the struck face.
            commands.trigger(Impact {
                position: entry,
                normal: Vec3::from(normal),
                caliber: projectile.caliber,
                surface: ImpactSurface::Armor,
                penetrated: true,
                deflection: None,
            });
            // AUTHORITY → CLIENTS: the shot's TERMINAL (`ImpactConfirm` on the wire). A perforation
            // ends the COSMETIC shell at this entry-face read even though the authoritative shell
            // marches on into the interior — the documented choice on [`ShellTerminal`]: what an
            // external viewer sees at the struck plate IS this read, and the client cannot march the
            // interior. Emitted for the FIRST perforation/embed only (the flag mutes same-tick
            // interior crossings; the deferred `Shot` strip mutes later ticks).
            if let Some(shot) = shot
                && !terminal_emitted
            {
                terminal_emitted = true;
                commands.trigger(ShellTerminal {
                    shot: shot.0,
                    position: entry,
                    normal: Vec3::from(normal),
                    penetrated: true,
                    after_bounces: marks.ricochets.len() as u32,
                });
                commands.entity(entity).remove::<Shot>();
            }
            speed = speed_for(projectile.mass, cap - cost);
            // The body keeps the momentum the shell lost crossing it; the shell carries the rest on.
            if let Some(body) = body {
                commands.trigger(HitImpulse {
                    body,
                    impulse: projectile.mass * (v_in - Vec3::from(dir) * speed),
                    point: entry,
                });
            }
            let exit = entry + dir * span;
            marks.events.push(PenetrationEvent {
                entry,
                exit,
                overmatched,
            });
            path.points.push(exit);

            // Transit damage: the main penetrator drove through this volume — if it's a damageable
            // component, deposit the cost it paid crossing (design §6). Armor has no HP, so no-op.
            if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                hp.current = (hp.current - cost * TRANSIT_K).max(0.0);
            }

            // Spall: the exit face throws a cone of fragments. The *count* comes from the material
            // chewed (cost) and the hole size (caliber) — the fragment supply; each fragment's
            // *energy* comes from the shot's residual (v_res²) and its position in the cone (on-axis
            // strongest). So a thin/soft body throws few fragments and a barely-through round throws
            // weak ones — both extremes low (design §5). Each fragment then penetrates per its energy.
            let count_f = SPALL_MAX_FRAGMENTS as f32
                * (cost / SPALL_COST_REF)
                * (projectile.caliber / SPALL_CALIBER_REF);
            let count = (count_f.round() as i32).clamp(0, SPALL_MAX_FRAGMENTS as i32) as usize;
            if count > 0 {
                // Residual energy sets how hard each fragment is thrown (full at the reference exit
                // speed); the on-axis fragments (`t→0`) keep the most of it.
                let shot_energy = (speed / SPALL_VRES_REF).powi(2).clamp(0.0, 1.0);
                let mut burst = SpallBurst {
                    origin: exit,
                    axis: dir,
                    half_angle: SPALL_HALF_ANGLE,
                    fragments: Vec::with_capacity(count),
                };
                for (fdir, t) in spall_directions(dir, SPALL_HALF_ANGLE, count) {
                    let birth_pen = FRAG_PEN_MAX * shot_energy * (1.0 - t);
                    burst.fragments.push(cast_spall_fragment(
                        exit + Vec3::from(fdir) * EPS,
                        fdir,
                        birth_pen,
                        SPALL_RANGE,
                        &spatial,
                        &volumes,
                        &parents,
                        &mut health,
                        &armor,
                        deposit,
                    ));
                }
                spall.bursts.push(burst);
            }

            pos = exit;
            remaining -= travelled + span;
        }

        // Reorient the shell to its travel direction so the mesh follows the (gravity-curved,
        // ricochet-bent) path instead of holding its launch heading.
        transform.translation = pos;
        transform.look_to(dir, Vec3::Y);
        path.points.push(pos);

        if holding {
            // Frozen at armor awaiting the bounce keyframe: keep the entity and its velocity untouched
            // (the `Held` handler at the top of the loop drives the wait next tick). The freeze point
            // was just recorded once, so the trail ends cleanly at the plate while the shell holds.
        } else if stopped {
            if retain.0 {
                // Sandbox: freeze where it stopped — drop the live components so it is no longer
                // integrated or labelled, but keep the stuck shell, its path, and its penetration
                // marks on screen for study (the sandbox's `c` command clears them).
                commands
                    .entity(entity)
                    .remove::<(Projectile, ShellReadout)>();
            } else {
                // Game: the spent shell is done.
                commands.entity(entity).despawn();
            }
        } else if pos.y < KILL_FLOOR {
            // Left the world: cleared the map edge and fell into the void below the terrain. Despawn
            // outright — there is no impact to inspect, so this ignores the sandbox's retain (unlike a
            // real impact). This is what bounds a shell that never hits terrain; see `KILL_FLOOR`.
            commands.entity(entity).despawn();
        } else {
            projectile.velocity = Vec3::from(dir) * speed;
            readout.speed = speed;
            readout.capability = capability(projectile.mass, speed);
        }
    }

    // Attribute this system's whole wall-time to the current fixed tick (`SPIKE_COST_TRACE`). Inert
    // (both `Option`s empty) unless the recorder is armed.
    if let (Some(cost), Some(t0)) = (cost.as_mut(), march_t0) {
        cost.record_march(t0.elapsed().as_secs_f64() * 1.0e6);
    }
}

/// Apply a crossing's momentum share to the struck body (immediate velocity change; the off-CoM
/// entry point also imparts the angular rock). A static or non-rigid owner simply won't match.
fn on_hit_impulse(
    hit: On<HitImpulse>,
    // Authority-only: on the net client (a replica) the struck body's motion is server-owned and
    // arrives by replication — applying a local impulse here would fight it (a divergent shove).
    replica: Option<Res<ClientReplica>>,
    mut bodies: Query<Forces>,
) {
    if replica.is_some() {
        return;
    }
    if let Ok(mut forces) = bodies.get_mut(hit.body) {
        forces.apply_linear_impulse_at_point(hit.impulse, hit.point);
    }
}

fn on_impact(impact: On<Impact>) {
    info!("shell impact at {:?}", impact.position);
    // The sim-side seam: the armor penetration march/spall hook in here. The debug marker is a
    // separate, view-side observer on this same event (`debug::spawn_impact_marker`), kept out of
    // the sim per ADR-0014.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `drag_k` is calibrated so the 88 keeps its hand-tuned coefficient, and a light-for-bore round
    /// (the 7.9 mm coax) bleeds far faster from sectional density alone — the reason a coax drops more
    /// than the main gun at the same range, with no per-weapon drag field.
    #[test]
    fn drag_k_calibration() {
        let main = drag_k(0.088, 10.2); // 88 mm, 10.2 kg
        let coax = drag_k(0.0079, 0.0118); // 7.9 mm, 11.8 g
        assert!(
            (main - 2.0e-4).abs() < 1.0e-5,
            "88 drag k should be ≈ 2e-4, got {main}"
        );
        assert!(
            coax > 6.0 * main,
            "coax should bleed far faster than the 88 (got {coax} vs {main})"
        );
    }

    /// Drag only slows a shell — never speeds it up or reverses it — and gravity always pulls the
    /// vertical component down. Guards the analytic drag step against a sign or stability slip.
    #[test]
    fn freeflight_step_bleeds_speed_and_falls() {
        let v0 = Vec3::new(700.0, 0.0, 0.0);
        let v1 = freeflight_step(v0, drag_k(0.088, 10.2), 0.01);
        assert!(v1.length() < v0.length(), "drag must reduce speed");
        assert!(v1.y < 0.0, "gravity must pull the shell down");
    }

    /// `advance_shell` IS the live march's open-air step: new velocity is the shared `freeflight_step`,
    /// and the position advances by that new velocity over `dt` (`p += v·dt`) — the exact `pos += dir *
    /// remaining` the ray-march does when a step hits nothing. Pinning it keeps the catch-up and the
    /// live march provably ONE implementation (ADR-0016) even if the march is later refactored.
    #[test]
    fn advance_shell_is_the_freeflight_step() {
        let pos = Vec3::new(2.0, 30.0, 5.0);
        let v = Vec3::new(500.0, -10.0, 40.0);
        let k = drag_k(0.088, 10.2);
        let dt = 1.0 / 64.0;
        let (p, nv) = advance_shell(pos, v, k, dt);
        let expected_v = freeflight_step(v, k, dt);
        assert_eq!(nv, expected_v, "velocity is the shared free-flight kernel");
        assert_eq!(
            p,
            pos + expected_v * dt,
            "position steps by the new velocity"
        );
    }

    /// The "one integrator" property (the test that matters): fast-forwarding a shell N ticks lands it
    /// in the SAME state as N single-tick advances. `fast_forward_shell` folds the shared
    /// `advance_shell` — the exact per-tick kernel the live march steps in open air — so a caught-up
    /// shell can't diverge from a natively integrated one. Guards against re-deriving the catch-up as a
    /// closed-form trajectory.
    #[test]
    fn fast_forward_matches_single_tick_advances() {
        let origin = Vec3::new(1.0, 50.0, -3.0);
        let v0 = Vec3::new(600.0, 20.0, 0.0);
        let k = drag_k(0.088, 10.2);
        let dt = 1.0 / 64.0;
        let n = 7;

        // N single-tick advances by hand.
        let (mut pos, mut vel) = (origin, v0);
        for _ in 0..n {
            (pos, vel) = advance_shell(pos, vel, k, dt);
        }

        let (ff_pos, ff_vel, path) = fast_forward_shell(origin, v0, k, dt, n);
        assert_eq!(ff_pos, pos, "fast-forward position == N single advances");
        assert_eq!(ff_vel, vel, "fast-forward velocity == N single advances");
        // One point per stepped tick plus the origin, and the trail starts AT the muzzle (requirement:
        // the tracer trail must not start 64 m behind the shell).
        assert_eq!(path.len(), n as usize + 1);
        assert_eq!(path[0], origin, "the trail starts at the muzzle");
        assert_eq!(*path.last().unwrap(), ff_pos, "the trail ends at the shell");
    }

    /// Zero catch-up is an exact no-op: the shell stays at the muzzle with its launch velocity and a
    /// one-point trail — byte-identical to a locally fired shell (SP / sandbox / own predicted), which
    /// always passes `catch_up_ticks: 0`.
    #[test]
    fn zero_catch_up_is_noop() {
        let origin = Vec3::new(0.0, 2.0, 0.0);
        let v0 = Vec3::new(800.0, 0.0, 0.0);
        let k = drag_k(0.088, 10.2);
        let (pos, vel, path) = fast_forward_shell(origin, v0, k, 1.0 / 64.0, 0);
        assert_eq!(pos, origin, "no catch-up leaves the shell at the muzzle");
        assert_eq!(vel, v0, "no catch-up leaves the launch velocity");
        assert_eq!(path, vec![origin], "no catch-up traces only the muzzle");
    }
}

/// Physics-backed march tests: an Avian world with a single steel `BallisticVolume` plate, an 88
/// round marched into it, and every `Impact` captured — so the new armor triggers (ricochet +
/// perforation) and the surface classification are exercised through the REAL `integrate_projectiles`
/// resolution, not mocked. Modelled on the sandbox's plate targets (`sandbox::spawn_targets`).
#[cfg(test)]
mod march_tests {
    use std::time::Duration;

    use avian3d::prelude::{Collider, CollisionLayers, PhysicsPlugins, RigidBody};
    use bevy::prelude::*;
    use bevy::time::TimeUpdateStrategy;

    use super::*;

    /// One captured impact — the fields the armor read branches on.
    #[derive(Clone, Copy)]
    struct Captured {
        position: Vec3,
        surface: ImpactSurface,
        penetrated: bool,
        deflection: Option<Vec3>,
    }

    /// The capture sink: every `Impact` the march fires lands here (view-only observer stand-in).
    #[derive(Resource, Default)]
    struct ImpactLog(Vec<Captured>);

    fn capture_impact(impact: On<Impact>, mut log: ResMut<ImpactLog>) {
        log.0.push(Captured {
            position: impact.position,
            surface: impact.surface,
            penetrated: impact.penetrated,
            deflection: impact.deflection,
        });
    }

    /// Steel: reference-mm of armor per metre of material, so a plate's cost ≈ its thickness in mm
    /// (matches `sandbox::spawn_targets`).
    const STEEL: f32 = 1000.0;

    /// Build an Avian world with one static steel plate (full extents `size`, centred at `at`, facing
    /// ±Z) on the `Armor` layer, register the real march + the impact capture, and settle the physics
    /// so the spatial-query pipeline includes the plate before any shell is marched.
    fn world_with_plate(size: Vec3, at: Vec3) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            // Avian's collider cache reads `AssetEvent<Mesh>`, so the asset system must be present
            // even though these cuboid colliders carry no mesh handle.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .init_resource::<RetainSpentShells>()
        .insert_resource(MgShortCircuit(false))
        .init_resource::<ImpactLog>()
        .add_observer(capture_impact)
        // The real march, run every Update (the `march_demo`/`march_real` run-if only selects the
        // clock; here `Res<Time>` is the virtual clock the manual duration steps).
        .add_systems(Update, integrate_projectiles);

        // Drive plugin finish/cleanup by hand (a bare `update()` loop skips it) — Avian registers its
        // diagnostics resources in `Plugin::finish`, and the spatial-query systems require them.
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();

        app.world_mut().spawn((
            Transform::from_translation(at),
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material_factor: STEEL,
            },
        ));

        // Settle: let Avian register the static collider and build the spatial-query pipeline before
        // a shell is marched against it.
        for _ in 0..8 {
            app.update();
        }
        app
    }

    /// Spawn an 88 round at `origin` travelling `dir` (unit) at `speed`, then march until an impact is
    /// captured (or the bound trips). Returns every impact fired.
    fn fire_and_capture(app: &mut App, origin: Vec3, dir: Vec3, speed: f32) -> Vec<Captured> {
        app.world_mut().spawn((
            Projectile {
                velocity: dir * speed,
                caliber: 0.088,
                mass: 10.2,
                drag_k: drag_k(0.088, 10.2),
            },
            ShellPath {
                points: vec![origin],
            },
            PenetrationMarks::default(),
            SpallMarks::default(),
            ShellReadout {
                speed,
                capability: capability(10.2, speed),
            },
            Transform::from_translation(origin).looking_to(dir, Vec3::Y),
        ));
        for _ in 0..8 {
            app.update();
            if !app.world().resource::<ImpactLog>().0.is_empty() {
                break;
            }
        }
        app.world().resource::<ImpactLog>().0.clone()
    }

    /// SHOOTER SELF-EXCLUSION ([`not_own_volume`]): a round is transparent to the tank that FIRED it.
    ///
    /// The tiger's coax fires from inside its own mantlet on every round after a burst's first (its
    /// recoiling barrel retracts the muzzle ~10 cm; the muzzle clears the mantlet by ~7 cm), so the
    /// march's very first cast struck the shooter's own armour a centimetre out — embedding the round on
    /// the authority, and fail-closing the tracer on every net client. The plate here stands for that
    /// mantlet: a round fired from INSIDE it must fly straight out if the plate belongs to its shooter,
    /// and must still be stopped by the very same plate if it does not. That second half is the point —
    /// this is an exclusion, not a hole in the armour.
    #[test]
    fn a_shell_ignores_the_tank_that_fired_it() {
        use crate::damage::VolumeOf;

        // A thick plate the shell starts INSIDE (origin at z = 0, the plate's centre) — the muzzle
        // buried in its own mask.
        for own in [true, false] {
            let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.4), Vec3::new(0.0, 2.0, 0.0));
            // The "tank" that owns the plate, and the shooter — the same entity or not, per the arm.
            let tank = app.world_mut().spawn(Transform::default()).id();
            let shooter = if own {
                tank
            } else {
                app.world_mut().spawn(Transform::default()).id()
            };
            let mut plates = app
                .world_mut()
                .query_filtered::<Entity, With<BallisticVolume>>();
            let plate = plates.single(app.world()).expect("one plate");
            app.world_mut().entity_mut(plate).insert(VolumeOf(tank));

            let origin = Vec3::new(0.0, 2.0, 0.0);
            let dir = Vec3::NEG_Z;
            let shell = app
                .world_mut()
                .spawn((
                    Projectile {
                        velocity: dir * 755.0,
                        caliber: 0.0079,
                        mass: 0.0118,
                        drag_k: drag_k(0.0079, 0.0118),
                    },
                    ShellPath {
                        points: vec![origin],
                    },
                    PenetrationMarks::default(),
                    SpallMarks::default(),
                    ShellReadout {
                        speed: 755.0,
                        capability: capability(0.0118, 755.0),
                    },
                    // The attribution the shell carries from `FireShell::shooter` — on the authority's
                    // shell, the shooter's own predicted shell, AND (since the coax fix) an observer's
                    // replica shell alike.
                    ShotSource {
                        tank: shooter,
                        weapon: 0,
                    },
                    Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                ))
                .id();
            app.update();

            let hits = app.world().resource::<ImpactLog>().0.clone();
            if own {
                assert!(
                    hits.is_empty(),
                    "a round fired from inside its OWN tank's armour must pass straight through it — \
                     the coax fires from inside its own mantlet every burst; got {} impact(s)",
                    hits.len(),
                );
                let flown = app
                    .world()
                    .get::<Transform>(shell)
                    .expect("the shell survives its own tank")
                    .translation
                    .distance(origin);
                assert!(
                    flown > 10.0,
                    "the round should fly a full step (~11.8 m at 755 m/s) out of its own tank; it \
                     moved {flown:.2} m",
                );
            } else {
                assert_eq!(
                    hits.len(),
                    1,
                    "the SAME plate must still stop a round from any other source — self-exclusion is \
                     an exclusion, not a hole in the armour",
                );
                assert_eq!(hits[0].surface, ImpactSurface::Armor);
            }
        }
    }

    /// A head-on 88 into a 50 mm steel plate cleanly perforates: exactly ONE armor impact at the entry
    /// face, flagged `penetrated` (it breached the plate), with no deflection. This exercises the new
    /// clean-perforation trigger (slice-1 fired no Impact on a perforated face).
    #[test]
    fn head_on_perforation_fires_one_penetrating_armor_impact() {
        // 50 mm plate: cost ≈ 50 ref-mm, far below the 88's ~263 capability at 800 m/s → perforates.
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_and_capture(&mut app, Vec3::new(0.0, 2.0, 2.0), Vec3::NEG_Z, 800.0);
        assert_eq!(
            hits.len(),
            1,
            "a clean perforation fires exactly one impact"
        );
        let hit = hits[0];
        assert_eq!(
            hit.surface,
            ImpactSurface::Armor,
            "the struck face is armor"
        );
        assert!(
            hit.penetrated,
            "a clean perforation is a penetration (flame lick earned)"
        );
        assert!(hit.deflection.is_none(), "a perforation does not deflect");
        // Entry is on the +Z face of the plate (half-thickness 0.025 above centre z=0).
        assert!(
            (hit.position.z - 0.025).abs() < 0.05,
            "the impact reads at the entry face, got z={}",
            hit.position.z
        );
    }

    /// A very oblique 88 (≈75° from the normal) into a 100 mm plate ricochets: exactly ONE armor
    /// impact at the bounce point, NOT flagged `penetrated` (a bounce ignites no flame lick), carrying
    /// the outgoing deflected direction for the view's directional spark fan. This exercises the new
    /// ricochet trigger (slice-1's ricochet branch was visually silent).
    #[test]
    fn oblique_ricochet_fires_one_deflecting_non_penetrating_impact() {
        // 100 mm plate is not overmatched by the 88 (0.088 < 3 × 0.10), so a steep graze ricochets.
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        // ≈75° from the +Z face normal: mostly along +X with a shallow −Z bite.
        let dir = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let hits = fire_and_capture(&mut app, Vec3::new(-1.0, 2.0, 0.6), dir, 800.0);
        assert_eq!(hits.len(), 1, "a ricochet fires exactly one impact");
        let hit = hits[0];
        assert_eq!(
            hit.surface,
            ImpactSurface::Armor,
            "the struck face is armor"
        );
        assert!(!hit.penetrated, "a ricochet bit no steel — no flame lick");
        let deflect = hit
            .deflection
            .expect("a ricochet carries its outgoing direction");
        // It bounced off the +Z face, so the deflected direction kicks back out along +Z.
        assert!(
            deflect.z > 0.0,
            "the bounce deflects back off the face (+Z), got {deflect:?}"
        );
    }

    /// Raise a `FireShell` (88, given `catch_up_ticks`) whose fast-forwarded flight overshoots a plate
    /// 2 m ahead, so the catch-up already-landed raycast in `on_fire_shell` hits — then return every
    /// impact the observer fired. Registers `on_fire_shell` with dummy asset resources (a catch-up hit
    /// returns before the shell scene is ever spawned, so the handles are never dereferenced).
    fn fire_shell_catch_up(app: &mut App, catch_up_ticks: u32) -> Vec<Captured> {
        app.insert_resource(ProjectileAssets {
            scene: Handle::default(),
        });
        app.insert_resource(TracerAssets {
            mesh: Handle::default(),
            material: Handle::default(),
        });
        app.add_observer(on_fire_shell);
        // Muzzle at z=+2, firing straight down −Z through the plate at the origin at high speed, so a
        // single-digit catch-up already overshoots it and the segment raycast finds the plate.
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
            shooter: None,
            tracer: true,
            catch_up_ticks,
            shot: None,
        });
        app.world_mut().flush();
        app.world().resource::<ImpactLog>().0.clone()
    }

    /// A FRESH catch-up (≤ STALE_FIRE_TICKS) whose flight fully resolves in the skip still fires the
    /// cosmetic impact read — the close-range remote-fire case the phantom exists to cover.
    #[test]
    fn fresh_catch_up_fires_the_phantom_impact() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_shell_catch_up(&mut app, 5);
        assert_eq!(hits.len(), 1, "a fresh catch-up hit reads once");
        assert_eq!(hits[0].surface, ImpactSurface::Armor, "the plate is armor");
    }

    /// A STALE catch-up (> STALE_FIRE_TICKS) whose flight fully resolves in the skip fires NO impact:
    /// the flash moment is long over, so the phantom would erupt a full splash + ground scar late from
    /// bare ground. It is suppressed by the same staleness bound the muzzle dressing uses.
    #[test]
    fn stale_catch_up_suppresses_the_phantom_impact() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_shell_catch_up(&mut app, STALE_FIRE_TICKS + 1);
        assert!(
            hits.is_empty(),
            "a stale catch-up must fire no late phantom impact, got {}",
            hits.len()
        );
    }

    /// Count the live `Projectile`s in the world (shells still in flight).
    fn live_projectiles(app: &mut App) -> usize {
        app.world_mut()
            .query::<&Projectile>()
            .iter(app.world())
            .count()
    }

    /// The oblique-ricochet setup from `oblique_ricochet_fires_one_deflecting_non_penetrating_impact`,
    /// but marched to resolution: how many `Impact`s, and does the shell SURVIVE the bounce (continue
    /// in flight)? Parameterised by whether the world is a net client (`ClientReplica` present).
    fn oblique_ricochet_outcome(replica: bool) -> (Vec<Captured>, usize) {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        if replica {
            app.insert_resource(crate::ClientReplica);
        }
        let dir = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let hits = fire_and_capture(&mut app, Vec3::new(-1.0, 2.0, 0.6), dir, 800.0);
        let survivors = live_projectiles(&mut app);
        (hits, survivors)
    }

    /// REPRO (symptom 2 — post-ricochet trail loss). AUTHORITY path (no `ClientReplica`): the server
    /// ricochets the 88 off the plate and the shell CONTINUES in flight (survives), carrying its trail
    /// on past the bounce. This is correct on the authority.
    #[test]
    fn authority_ricochet_shell_survives_and_continues() {
        let (hits, survivors) = oblique_ricochet_outcome(false);
        assert_eq!(hits.len(), 1, "one bounce impact");
        assert!(hits[0].deflection.is_some(), "authority reads a ricochet");
        assert_eq!(survivors, 1, "the ricocheted shell continues in flight");
    }

    /// FIX (symptom 2). REPLICA path (`ClientReplica` present — the remote observer): the client must
    /// NOT re-simulate the authoritative bounce against interpolated geometry. Fail-closed — the
    /// cosmetic shell STOPS dead at first armor contact (despawned, no survivor), firing a NEUTRAL
    /// armor spark (no deflection fan, no flame lick), so its trail ends at contact instead of chasing
    /// an improvised deflection the server never sanctioned.
    #[test]
    fn replica_ricochet_fails_closed_at_first_armor_contact() {
        let (hits, survivors) = oblique_ricochet_outcome(true);
        assert_eq!(hits.len(), 1, "one armor-contact spark");
        assert_eq!(hits[0].surface, ImpactSurface::Armor, "it hit armor");
        assert!(
            hits[0].deflection.is_none(),
            "no improvised bounce fan — the client cannot know the outcome"
        );
        assert!(!hits[0].penetrated, "no flame lick — neutral spark");
        assert_eq!(
            survivors, 0,
            "the cosmetic shell stops dead at contact (trail ends there)"
        );
    }

    /// The fail-closed guard is REPLICA-ONLY: a head-on perforation on the AUTHORITY still drives the
    /// round through the plate and out the far side (the server's shell continues), unaffected by the
    /// guard. Pins that the `!deposit` gate did not leak into the authority march.
    #[test]
    fn authority_perforation_still_drives_through() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_and_capture(&mut app, Vec3::new(0.0, 2.0, 2.0), Vec3::NEG_Z, 800.0);
        assert_eq!(hits.len(), 1, "one perforation impact");
        assert!(hits[0].penetrated, "authority perforates (flame lick)");
    }

    // --- Keyframe carry-through (piece 2) ------------------------------------------------------------

    /// The fields of a `ShellRicochet` the authority raised — the server-sanctioned bounce.
    #[derive(Clone, Copy)]
    struct CapturedBounce {
        origin: Vec3,
        direction: Vec3,
        speed: f32,
        sequence: u32,
    }

    #[derive(Resource, Default)]
    struct RicochetLog(Vec<CapturedBounce>);

    fn capture_ricochet(ricochet: On<ShellRicochet>, mut log: ResMut<RicochetLog>) {
        log.0.push(CapturedBounce {
            origin: ricochet.origin,
            direction: ricochet.direction,
            speed: ricochet.speed,
            sequence: ricochet.sequence,
        });
    }

    /// The oblique-ricochet geometry (the same steep graze the existing ricochet test uses), started at
    /// the standard point and carrying `shot` so the authority march raises a `ShellRicochet`.
    fn spawn_oblique_shell(app: &mut App, shot: ShotId) -> Entity {
        let origin = Vec3::new(-1.0, 2.0, 0.6);
        let dir = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let speed = 800.0;
        app.world_mut()
            .spawn((
                Projectile {
                    velocity: dir * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                },
                ShellPath {
                    points: vec![origin],
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id()
    }

    /// A shell fired into OPEN AIR (away from any plate), carrying `shot` — used by the F1/F3 tests
    /// that need a cosmetic round which free-flies without ever contacting armor.
    fn spawn_free_shell(
        app: &mut App,
        origin: Vec3,
        dir: Vec3,
        speed: f32,
        shot: ShotId,
    ) -> Entity {
        app.world_mut()
            .spawn((
                Projectile {
                    velocity: dir.normalize() * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                },
                ShellPath {
                    points: vec![origin],
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id()
    }

    /// Run the AUTHORITY (no `ClientReplica`) oblique ricochet and return the sanctioned bounce it
    /// raises — the server truth an observer must re-seed from.
    fn authority_bounce(shot: ShotId) -> CapturedBounce {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<RicochetLog>();
        app.add_observer(capture_ricochet);
        spawn_oblique_shell(&mut app, shot);
        for _ in 0..8 {
            app.update();
            if !app.world().resource::<RicochetLog>().0.is_empty() {
                break;
            }
        }
        *app.world()
            .resource::<RicochetLog>()
            .0
            .first()
            .expect("the authority raised a ShellRicochet for the oblique shot")
    }

    fn a_shot() -> ShotId {
        ShotId {
            shooter: Entity::PLACEHOLDER,
            weapon: 0,
            fire_tick: 100,
        }
    }

    /// A replica world (`ClientReplica` + a `SanctionedShots` buffer + the plate) ready to march an
    /// observer shell.
    fn replica_world(bounces: SanctionedShots) -> App {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        app.insert_resource(crate::ClientReplica);
        app.insert_resource(bounces);
        app
    }

    /// CARRY-THROUGH (pre-armed, the common case). The keyframe is already buffered when the observer
    /// shell reaches armor contact: it re-seeds onto server truth and CONTINUES — the trail runs through
    /// the exact bounce point (no gap), the post-bounce direction matches the server's within
    /// integration tolerance, one directional (sanctioned) spark reads, and the shell survives.
    #[test]
    fn observer_reseeds_from_prearmed_keyframe_and_continues() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);
        assert_eq!(bounce.sequence, 0, "the first bounce is ordinal 0");

        let mut buf = SanctionedShots::default();
        buf.insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                // Inert in these tests (no `PredictedPresent` resource → the F3 overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
            },
        );
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        // March until the shell re-seeds (a ricochet recorded) or the bound trips.
        for _ in 0..8 {
            app.update();
            if !app
                .world()
                .get::<PenetrationMarks>(shell)
                .unwrap()
                .ricochets
                .is_empty()
            {
                break;
            }
        }

        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(marks.ricochets.len(), 1, "exactly one re-seed");
        assert!(
            marks.ricochets[0].distance(bounce.origin) < 1.0e-3,
            "re-seeded at the exact server bounce point (tracer clamp re-anchors here)",
        );
        let path = app.world().get::<ShellPath>(shell).unwrap();
        assert!(
            path.points
                .iter()
                .any(|p| p.distance(bounce.origin) < 1.0e-3),
            "the trail runs THROUGH the bounce point — no gap in the ribbon",
        );
        let velocity = app.world().get::<Projectile>(shell).unwrap().velocity;
        let angle = velocity
            .normalize()
            .angle_between(bounce.direction.normalize());
        assert!(
            angle < 0.05,
            "post-bounce direction matches server truth within integration tolerance (got {angle} rad)",
        );
        assert!(
            velocity.z > 0.0,
            "the shell deflects back off the +Z face and flies on — the server's outcome",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shell survives the bounce (it continues, it does not truncate)",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "one bounce spark");
        assert!(
            impacts[0].deflection.is_some(),
            "the sanctioned bounce reads with its directional fan, not the neutral fail-closed spark",
        );
    }

    /// HOLD-THEN-ARRIVE. The keyframe is delayed past contact but lands inside the grace window: the
    /// shell FREEZES at the plate (no spark yet), then re-seeds when the bounce arrives — re-aged
    /// forward by exactly the ticks it held, so its resumed position is consistent with the present
    /// timeline (matches the shared integrator advanced by the hold duration).
    #[test]
    fn observer_holds_then_reseeds_when_keyframe_arrives_in_window() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);

        let mut app = replica_world(SanctionedShots::default()); // buffer starts EMPTY
        let shell = spawn_oblique_shell(&mut app, shot);

        // March until the shell freezes at armor (no keyframe yet).
        let mut froze = false;
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                froze = true;
                break;
            }
        }
        assert!(
            froze,
            "the shell holds at armor contact awaiting its keyframe"
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "a held shell fires NO spark — it waits (no improvised impact)",
        );

        // Hold a few more ticks (still inside the grace window), then the keyframe lands.
        const HELD_EXTRA: u32 = 4;
        for _ in 0..HELD_EXTRA {
            app.update();
        }
        assert_eq!(
            app.world().get::<Held>(shell).unwrap().ticks,
            HELD_EXTRA,
            "the hold accumulates one tick per frozen tick (the catch-up it will re-age by)",
        );
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                // Inert in these tests (no `PredictedPresent` resource → the F3 overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
            },
        );
        app.update(); // the Held handler re-seeds this tick

        assert!(
            app.world().get::<Held>(shell).is_none(),
            "the hold clears on re-seed",
        );
        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(marks.ricochets.len(), 1);
        assert!(
            marks.ricochets[0].distance(bounce.origin) < 1.0e-3,
            "the tracer clamp re-anchors at the true bounce point",
        );
        // Re-aged forward by the hold: the resumed position is the sanctioned state fast-forwarded
        // HELD_EXTRA ticks — the exact shared integrator, so it is consistent with the present timeline.
        let dt = 0.016; // the test world's ManualDuration
        let (expected_pos, _, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_EXTRA,
        );
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(expected_pos) < 1.0e-3,
            "resumed position = sanctioned bounce fast-forwarded by the hold duration (got {pos}, want {expected_pos})",
        );
        assert!(
            pos.distance(bounce.origin) > 1.0e-2,
            "the re-aged shell is downrange of the bounce point (it caught back up to the present)",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shell survives and continues after the delayed re-seed",
        );
    }

    // --- F1: the cosmetic march is rollback-safe -----------------------------------------------------

    /// Put the replica world into (`true`) or out of (`false`) a lightyear rollback replay — the same
    /// sim-visible flag `net::client::mark_replaying` maintains. The march reads it as
    /// `Option<Res<Replaying>>` and skips the whole cosmetic advance while it is `true`.
    fn set_replaying(app: &mut App, replaying: bool) {
        app.insert_resource(crate::Replaying(replaying));
    }

    /// ROLLBACK STORM over a free-flying shell. A replay re-runs `FixedMain` (hence this march) N times
    /// in one frame; every one of those replayed ticks must leave the cosmetic shell EXACTLY where it
    /// was — no double-march teleport, no duplicate `ShellPath` points, no spurious impact. The next
    /// FORWARD tick resumes the march.
    #[test]
    fn rollback_replay_freezes_the_cosmetic_march() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        // A shell fired into OPEN AIR, away from the plate (at z≈0), so it free-flies for the whole
        // test and never enters the hold path — isolating the pure free-flight march.
        let origin = Vec3::new(0.0, 2.0, 5.0);
        let dir = Vec3::Z;
        let speed = 800.0;
        let shell = app
            .world_mut()
            .spawn((
                Projectile {
                    velocity: dir * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                },
                ShellPath {
                    points: vec![origin],
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id();

        // One forward tick: the shell is in open air (it has not reached any plate).
        app.update();
        assert!(
            app.world().get::<Held>(shell).is_none(),
            "baseline: the shell is still free-flying, not yet at contact",
        );
        let pos_before = app.world().get::<Transform>(shell).unwrap().translation;
        let vel_before = app.world().get::<Projectile>(shell).unwrap().velocity;
        let points_before = app.world().get::<ShellPath>(shell).unwrap().points.len();

        // A storm of 8 replayed ticks: the march must be inert.
        set_replaying(&mut app, true);
        for _ in 0..8 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Transform>(shell).unwrap().translation,
            pos_before,
            "a replayed tick must not advance the shell (no double-march teleport)",
        );
        assert_eq!(
            app.world().get::<Projectile>(shell).unwrap().velocity,
            vel_before,
            "a replayed tick must not integrate the shell's velocity",
        );
        assert_eq!(
            app.world().get::<ShellPath>(shell).unwrap().points.len(),
            points_before,
            "a replayed tick must not append duplicate ShellPath points",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "a replayed tick fires no impact",
        );

        // Back on the forward timeline the march resumes.
        set_replaying(&mut app, false);
        app.update();
        assert_ne!(
            app.world().get::<Transform>(shell).unwrap().translation,
            pos_before,
            "a forward tick resumes the march",
        );
    }

    /// ROLLBACK STORM over a HELD shell. `Held.ticks` is the exact catch-up the re-seed re-ages by
    /// (`present − bounce_tick`); a replay must not increment it, or the re-seed lands the shell AHEAD
    /// of the present. Assert the hold is frozen across a storm, and that the eventual re-seed is
    /// re-aged by the TRUE forward hold count — unchanged by the replays.
    #[test]
    fn rollback_replay_does_not_age_the_hold_and_reseed_stays_exact() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);
        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);

        // March until the shell freezes at armor (no keyframe yet).
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        assert!(app.world().get::<Held>(shell).is_some(), "held at contact");

        // Accumulate the TRUE hold over some FORWARD ticks.
        const HELD_FWD: u32 = 4;
        for _ in 0..HELD_FWD {
            app.update();
        }
        assert_eq!(app.world().get::<Held>(shell).unwrap().ticks, HELD_FWD);

        // A storm of 8 replayed ticks WHILE held: the grace-window counter must not move.
        set_replaying(&mut app, true);
        for _ in 0..8 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Held>(shell).unwrap().ticks,
            HELD_FWD,
            "a replay must not age the hold window (it would burn the grace window and over-age the re-seed)",
        );

        // The keyframe lands; on the forward timeline the shell re-seeds, re-aged by exactly the true
        // forward hold — the replays did not inflate it.
        set_replaying(&mut app, false);
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                // Inert in these tests (no `PredictedPresent` resource → the F3 overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
            },
        );
        app.update();
        let dt = 0.016;
        let (expected_pos, _, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_FWD,
        );
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(expected_pos) < 1.0e-3,
            "re-seed re-aged by the TRUE hold count, not the storm's replays (got {pos}, want {expected_pos})",
        );
    }

    /// HOLD-THEN-EXPIRE (F3(ii) — QUIET DISSOLVE). The keyframe NEVER arrives: past the grace window
    /// the shell ends SILENTLY — despawned, its trail simply stops, and NO spark. A held shell that
    /// got no sanctioned outcome is either a lost verdict or a pose-divergent miss, and a neutral spark
    /// at a client-computed contact would fabricate a contact the authority never confirmed (invariant
    /// 2 / the honesty doctrine). Correctness never depended on delivery.
    #[test]
    fn observer_hold_expires_to_quiet_dissolve() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default()); // no keyframe ever arrives
        let shell = spawn_oblique_shell(&mut app, shot);

        for _ in 0..(RICOCHET_HOLD_TICKS + 4) {
            app.update();
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "the shell is finalized (despawned) once the grace window expires",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "NO spark — a fabricated contact the authority never confirmed would violate the honesty \
             doctrine; the shell dissolves quietly",
        );
    }

    /// MULTI-BOUNCE. Two sanctioned bounces (ordinals 0 then 1) are consumed strictly in order: the
    /// shell re-seeds through bounce 0 (redirected back toward the plate), hits armor again, and
    /// re-seeds through bounce 1 (redirected away) — the trail carrying both bounce points in sequence.
    /// The bounces are fabricated (not authority-derived) so the geometry deterministically forces the
    /// second contact.
    #[test]
    fn observer_consumes_two_bounces_in_order() {
        let shot = a_shot();
        // Bounce 0: re-seed just off the +Z face, aimed BACK toward the plate so the leftover march
        // budget carries the shell into a second contact.
        let b0 = SanctionedBounce {
            origin: Vec3::new(0.0, 2.0, 0.5),
            direction: Vec3::new(0.12, 0.0, -1.0).normalize(),
            speed: 480.0,
            bounce_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
            sequence: 0,
        };
        // Bounce 1: re-seed on the face, aimed AWAY (+Z) so the shell flies out and survives.
        let b1 = SanctionedBounce {
            origin: Vec3::new(0.0, 2.0, 0.05),
            direction: Vec3::Z,
            speed: 300.0,
            bounce_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
            sequence: 1,
        };
        let mut buf = SanctionedShots::default();
        buf.insert(shot, b0);
        buf.insert(shot, b1);
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        for _ in 0..8 {
            app.update();
            if app.world().get_entity(shell).is_err()
                || app
                    .world()
                    .get::<PenetrationMarks>(shell)
                    .is_some_and(|m| m.ricochets.len() >= 2)
            {
                break;
            }
        }

        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(
            marks.ricochets.len(),
            2,
            "both sanctioned bounces were consumed",
        );
        assert!(
            marks.ricochets[0].distance(b0.origin) < 1.0e-3,
            "bounce 0 (ordinal 0) re-seeds first",
        );
        assert!(
            marks.ricochets[1].distance(b1.origin) < 1.0e-3,
            "bounce 1 (ordinal 1) re-seeds second — strict order",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "after the second bounce the shell flies on",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 2, "two directional bounce sparks");
        assert!(impacts.iter().all(|i| i.deflection.is_some()));
    }

    /// THE SHOOTER'S OWN SHELL (predicted-timeline variant of the hold-then-arrive test). An own shell
    /// carries `ShotSource` (attributed local fire) + the `Shot` the shared stamp completed — and,
    /// living AT the predicted present, it always reaches the plate before the server's keyframe can
    /// arrive, so HOLD is its expected path: it freezes HIDDEN at contact (the invisible-stop — the
    /// shooter is watching this round; a frozen shell hanging on the plate would read as a bug), then
    /// re-seeds and re-shows when the keyframe lands, re-aged by the held ticks so its resumed
    /// position is consistent with the present timeline. The fall-of-shot read on a bounced round.
    #[test]
    fn own_shell_holds_hidden_then_reseeds_when_keyframe_arrives() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);

        let mut app = replica_world(SanctionedShots::default()); // keyframe not yet arrived
        let shell = spawn_oblique_shell(&mut app, shot);
        // The own-shell shape: attributed local fire — `ShotSource` rides the shell alongside the
        // stamped `Shot` (`net::protocol::stamp_shot_ids`); the march must treat it identically.
        app.world_mut().entity_mut(shell).insert(ShotSource {
            tank: Entity::PLACEHOLDER,
            weapon: 0,
        });

        // March to contact: the shell holds — hidden, no spark, entity kept.
        let mut froze = false;
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                froze = true;
                break;
            }
        }
        assert!(
            froze,
            "the own shell holds at armor contact like any Shot-carrying shell"
        );
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Hidden),
            "the hold is an INVISIBLE stop — no frozen round hanging on the plate",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "no spark while holding",
        );

        // The keyframe lands (inside the grace window) → re-seed, re-show, continue.
        const HELD_EXTRA: u32 = 3;
        for _ in 0..HELD_EXTRA {
            app.update();
        }
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                // Inert in these tests (no `PredictedPresent` resource → the F3 overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
            },
        );
        app.update();

        assert!(app.world().get::<Held>(shell).is_none(), "hold cleared");
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Inherited),
            "the re-seeded shell is shown again",
        );
        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(
            marks.ricochets.len(),
            1,
            "the sanctioned bounce re-anchored the clamp"
        );
        // Re-aged by the held ticks — the same present − bounce_tick arithmetic as an observer shell.
        let dt = 0.016;
        let (expected_pos, _, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_EXTRA,
        );
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(expected_pos) < 1.0e-3,
            "own shell resumes at the sanctioned state fast-forwarded by the hold (got {pos}, want {expected_pos})",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shooter's own bounced round flies on — the fall-of-shot read",
        );
    }

    /// The own shell's keyframe LOST: same honest F3(ii) QUIET DISSOLVE as an observer shell — no
    /// spark at expiry, shell finalized silently. (The own shell is not special-cased anywhere in the
    /// march; this pins that its `ShotSource` changes nothing about the degradation.)
    #[test]
    fn own_shell_keyframe_lost_dissolves_quietly() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default()); // no keyframe, ever
        let shell = spawn_oblique_shell(&mut app, shot);
        app.world_mut().entity_mut(shell).insert(ShotSource {
            tank: Entity::PLACEHOLDER,
            weapon: 0,
        });

        for _ in 0..(RICOCHET_HOLD_TICKS + 4) {
            app.update();
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "the own shell finalizes once the grace window expires",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "no spark — the own shell dissolves quietly too (no fabricated contact)",
        );
    }

    // --- F3: tick-triggered consumption of an overdue sanctioned outcome (the pose-divergent miss) ----

    /// CLIENT-MISS / SERVER-HIT (bounce). The client's shell flies against a target pose the server's
    /// didn't share, so it sails PAST the plate and never contacts — but the server DID resolve a
    /// bounce there. Once the predicted present passes the bounce's server tick by the margin, the
    /// overdue path re-seeds the shell at the server bounce (with the sanctioned directional spark)
    /// instead of letting it fly on through where the authoritative round bounced (invariant 2).
    #[test]
    fn overdue_bounce_reseeds_a_pose_divergent_miss() {
        let shot = a_shot(); // fire_tick = 100
        let mut app = replica_world(SanctionedShots::default());
        // Present well past the server bounce tick — the client shell missed and flew on.
        app.insert_resource(crate::PredictedPresent(shot.fire_tick + 20));
        let bounce_origin = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce_origin,
                direction: Vec3::Z,
                speed: 500.0,
                // present(120) − bounce_tick(105) = 15 > OVERDUE_MARGIN_TICKS.
                bounce_tick: shot.fire_tick + 5,
                sequence: 0,
            },
        );
        // A shell flying in open air high above the plate — it never contacts.
        let shell = spawn_free_shell(&mut app, Vec3::new(0.0, 20.0, 5.0), Vec3::Z, 800.0, shot);

        app.update(); // the overdue check consumes the bounce this tick

        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(marks.ricochets.len(), 1, "the overdue bounce is consumed");
        assert!(
            marks.ricochets[0].distance(bounce_origin) < 1.0e-3,
            "re-seeded at the SERVER bounce point, not where the client's round flew",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shell survives and flies on from the server bounce",
        );
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(bounce_origin) > 1.0e-2,
            "re-aged forward of the bounce (caught up to the present)",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "the sanctioned bounce sparks");
        assert!(
            impacts[0].deflection.is_some(),
            "with its directional fan (server truth), not a fabricated neutral spark",
        );
    }

    /// CLIENT-MISS / SERVER-HIT (terminal). Same pose-divergence, but the server resolved an
    /// embed/perforation. The overdue path finalizes the shell at the SERVER's impact with the honest
    /// armor read (position, normal, `penetrated`) rather than holding for a contact that never comes.
    #[test]
    fn overdue_terminal_finalizes_a_pose_divergent_miss() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        app.insert_resource(crate::PredictedPresent(shot.fire_tick + 20));
        let impact_pos = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: impact_pos,
                    normal: Vec3::Z,
                    penetrated: true,
                    impact_tick: shot.fire_tick + 5,
                    after_bounces: 0,
                },
            );
        let shell = spawn_free_shell(&mut app, Vec3::new(0.0, 20.0, 5.0), Vec3::Z, 800.0, shot);

        app.update();

        assert!(
            app.world().get_entity(shell).is_err(),
            "the shell finalizes at the server terminal",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "the server terminal reads");
        assert!(
            impacts[0].position.distance(impact_pos) < 1.0e-3,
            "at the SERVER's impact position",
        );
        assert!(
            impacts[0].penetrated,
            "with the server's penetration verdict (the flame lick MP would otherwise miss)",
        );
    }

    /// The margin guard: a sanctioned outcome only a tick or two old is NOT force-consumed — the shell
    /// is still allowed to reach the plate and hold/contact normally, so a legitimately-imminent
    /// contact isn't snapped away mid-air.
    #[test]
    fn a_sanctioned_outcome_within_the_margin_is_not_force_consumed() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        app.insert_resource(crate::PredictedPresent(shot.fire_tick + 5));
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: Vec3::new(1.0, 2.0, 3.0),
                direction: Vec3::Z,
                speed: 500.0,
                // present(105) − bounce_tick(104) = 1 <= OVERDUE_MARGIN_TICKS — inside the margin.
                bounce_tick: shot.fire_tick + 4,
                sequence: 0,
            },
        );
        let shell = spawn_free_shell(&mut app, Vec3::new(0.0, 20.0, 5.0), Vec3::Z, 800.0, shot);

        app.update();

        assert!(
            app.world()
                .get::<PenetrationMarks>(shell)
                .unwrap()
                .ricochets
                .is_empty(),
            "inside the margin the shell is not force-consumed — it flies on toward a real contact",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "no premature spark inside the margin",
        );
    }

    // --- Terminal confirms (ImpactConfirm carry-through) ---------------------------------------------

    /// The fields of a `ShellTerminal` the authority raised — the server-sanctioned armor end.
    #[derive(Clone, Copy)]
    struct CapturedTerminal {
        position: Vec3,
        penetrated: bool,
        after_bounces: u32,
    }

    #[derive(Resource, Default)]
    struct TerminalLog(Vec<CapturedTerminal>);

    fn capture_terminal(terminal: On<ShellTerminal>, mut log: ResMut<TerminalLog>) {
        log.0.push(CapturedTerminal {
            position: terminal.position,
            penetrated: terminal.penetrated,
            after_bounces: terminal.after_bounces,
        });
    }

    /// A head-on shell at the standard plate (fired from z=+2 down −Z), carrying `shot`.
    fn spawn_headon_shell(app: &mut App, shot: ShotId) -> Entity {
        let origin = Vec3::new(0.0, 2.0, 2.0);
        let dir = Vec3::NEG_Z;
        let speed = 800.0;
        app.world_mut()
            .spawn((
                Projectile::test_88(dir * speed),
                ShellPath {
                    points: vec![origin],
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id()
    }

    /// AUTHORITY EMISSION — embed. A 500 mm plate defeats the 88 head-on (cost 500 > ~263 cap): the
    /// march raises exactly ONE `ShellTerminal`, penetrated (an embed bit steel), zero prior bounces.
    #[test]
    fn authority_embed_emits_one_penetrated_terminal() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.5), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        spawn_headon_shell(&mut app, a_shot());
        for _ in 0..8 {
            app.update();
            if !app.world().resource::<TerminalLog>().0.is_empty() {
                break;
            }
        }
        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(terminals.len(), 1, "an embed emits exactly one terminal");
        assert!(terminals[0].penetrated, "an embed bit steel — flame lick");
        assert_eq!(terminals[0].after_bounces, 0);
    }

    /// AUTHORITY EMISSION — perforation, the documented choice: the terminal reads at the ENTRY face
    /// and the AUTHORITATIVE shell continues (it is not truncated by the cosmetic-terminal decision);
    /// a later embed of the same shot (a 500 mm backstop behind the plate) emits NO second terminal —
    /// at most one per shot, even across the same march step.
    #[test]
    fn authority_perforation_emits_one_terminal_and_marches_on() {
        // 50 mm plate at z=0 (perforates head-on) + a 500 mm backstop at z=-2 (embeds the residual).
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(0.0, 2.0, -2.0)),
            RigidBody::Static,
            Collider::cuboid(3.0, 3.0, 0.5),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material_factor: STEEL,
            },
        ));
        for _ in 0..8 {
            app.update(); // settle the new collider into the spatial pipeline
        }
        spawn_headon_shell(&mut app, a_shot());
        // March to full resolution (the backstop embed despawns the shell).
        for _ in 0..8 {
            app.update();
        }
        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(
            terminals.len(),
            1,
            "one terminal per shot — the perforation; the later embed is muted",
        );
        assert!(terminals[0].penetrated, "a perforation breached the plate");
        assert!(
            (terminals[0].position.z - 0.025).abs() < 0.05,
            "the terminal reads at the plate's ENTRY face, got z={}",
            terminals[0].position.z
        );
        // The authoritative shell marched past the plate (into the backstop) — the cosmetic-terminal
        // choice truncates nothing on the authority: the impact log shows the perforation AND the
        // backstop embed.
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            impacts.len() >= 2,
            "the authority resolves the perforation and then the backstop embed (got {})",
            impacts.len()
        );
    }

    /// HELD SHELL + CONFIRM: the full honest armor read, at the server's position, on receipt — the
    /// neutral fail-closed spark never fires. This is the read that gives MP the SP-grade armor
    /// feedback (flame lick via `penetrated: true`), typically ≈(P−S)+OWL after contact instead of
    /// the fail-closed window.
    #[test]
    fn held_shell_resolves_on_confirm_with_full_honest_read() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);

        // March to contact; the shell holds (hidden, no spark).
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        assert!(app.world().get::<Held>(shell).is_some(), "shell held");
        assert!(app.world().resource::<ImpactLog>().0.is_empty());

        // The server's terminal arrives (an embed read at ITS position, slightly off the local
        // contact) → resolve immediately with the full read.
        let server_pos = Vec3::new(0.05, 2.0, 0.08);
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: server_pos,
                    normal: Vec3::Z,
                    penetrated: true,
                    impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                    after_bounces: 0,
                },
            );
        app.update();

        assert!(
            app.world().get_entity(shell).is_err(),
            "the confirmed shell resolves (despawns) on receipt — not at the window",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(
            impacts.len(),
            1,
            "exactly one impact — no neutral spark ever"
        );
        assert!(
            impacts[0].penetrated,
            "the SERVER's penetration verdict rides through — the flame lick is earned in MP",
        );
        assert!(
            impacts[0].position.distance(server_pos) < 1.0e-4,
            "the read lands at the SERVER's position",
        );
        assert_eq!(impacts[0].surface, ImpactSurface::Armor);
        assert!(
            impacts[0].deflection.is_none(),
            "a terminal is not a bounce"
        );
    }

    /// PRE-ARMED CONFIRM: the terminal is already buffered when the shell reaches the plate → resolve
    /// INSTANTLY at contact (no hold, never hidden), with the server's full read.
    #[test]
    fn prearmed_confirm_resolves_at_contact_instantly() {
        let shot = a_shot();
        let mut buf = SanctionedShots::default();
        let server_pos = Vec3::new(0.02, 2.0, 0.09);
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: server_pos,
                normal: Vec3::Z,
                penetrated: true,
                impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                after_bounces: 0,
            },
        );
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        for _ in 0..8 {
            app.update();
            if app.world().get_entity(shell).is_err() {
                break;
            }
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "a pre-armed confirm resolves the shell at contact",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "one impact, immediately at contact");
        assert!(impacts[0].penetrated, "server verdict rides through");
        assert!(impacts[0].position.distance(server_pos) < 1.0e-4);
    }

    /// ORDERING: a terminal that follows a bounce (`after_bounces: 1`) must NOT resolve a shell that
    /// has not re-seeded through that bounce yet — the shell keeps holding for the (late) bounce
    /// keyframe, re-seeds when it lands, and only its NEXT contact consumes the terminal.
    #[test]
    fn terminal_waits_for_owed_bounce_then_resolves() {
        let shot = a_shot();
        let mut buf = SanctionedShots::default();
        // The terminal is ordered AFTER one bounce; the bounce keyframe is late (not yet buffered).
        let server_pos = Vec3::new(0.0, 2.0, 0.06);
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: server_pos,
                normal: Vec3::Z,
                penetrated: false,
                impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                after_bounces: 1,
            },
        );
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        // Contact: the shell must HOLD (the terminal is not its next event — a bounce is owed).
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        assert!(
            app.world().get::<Held>(shell).is_some(),
            "an after-bounces terminal must not resolve a shell that still owes a bounce",
        );
        assert!(app.world().resource::<ImpactLog>().0.is_empty());

        // The late bounce keyframe lands — fabricated to throw the shell back at the plate so a
        // second contact happens (the multi-bounce test's trick).
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: Vec3::new(0.0, 2.0, 0.5),
                direction: Vec3::new(0.12, 0.0, -1.0).normalize(),
                speed: 480.0,
                bounce_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                sequence: 0,
            },
        );
        for _ in 0..8 {
            app.update();
            if app.world().get_entity(shell).is_err() {
                break;
            }
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "after the bounce re-seed, the next contact consumes the terminal",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 2, "the bounce spark, then the terminal read");
        assert!(
            impacts[0].deflection.is_some(),
            "first: the sanctioned bounce"
        );
        assert!(impacts[1].deflection.is_none(), "second: the terminal");
        assert!(impacts[1].position.distance(server_pos) < 1.0e-4);
    }

    /// THE SHOOTER'S OWN SHELL consumes its confirm exactly like an observer's (no special-casing) —
    /// the own-shape shell (`ShotSource` riding it) held at contact resolves on the confirm with the
    /// full honest read.
    #[test]
    fn own_shell_confirm_applies() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);
        app.world_mut().entity_mut(shell).insert(ShotSource {
            tank: Entity::PLACEHOLDER,
            weapon: 0,
        });

        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: Vec3::new(0.0, 2.0, 0.07),
                    normal: Vec3::Z,
                    penetrated: true,
                    impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                    after_bounces: 0,
                },
            );
        app.update();

        assert!(
            app.world().get_entity(shell).is_err(),
            "the shooter's own shell resolves on its confirm — the honest read on their own hit",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1);
        assert!(impacts[0].penetrated, "own hit shows the flame lick in MP");
    }

    /// TERMINAL DEDUP: a shot has at most one terminal — the first insert wins, and a redundantly
    /// retransmitted confirm (the sliding window re-carries it) is a no-op.
    #[test]
    fn terminal_insert_is_first_wins_idempotent() {
        let shot = a_shot();
        let mut buf = SanctionedShots::default();
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: Vec3::X,
                normal: Vec3::Z,
                penetrated: true,
                impact_tick: 0, // inert (dedup test — buffer semantics, not the march)
                after_bounces: 0,
            },
        );
        // The redundancy window re-offers it (and even a corrupt divergent duplicate must not win).
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: Vec3::Y * 99.0,
                normal: Vec3::Z,
                penetrated: false,
                impact_tick: 0, // inert (dedup test — buffer semantics, not the march)
                after_bounces: 3,
            },
        );
        let stored = buf.terminal(shot, 0).expect("terminal stored");
        assert_eq!(stored.position, Vec3::X, "first insert wins");
        assert!(stored.penetrated, "first insert's verdict kept");
    }
}
