//! Damage consequences: interpret component HP loss as crew death, ammo cookoff, tank death label,
//! and function loss. Ballistics owns *how* damage is deposited; this module owns what depleted
//! damageable volumes mean.
//!
//! **Kill model (design §1a-A, under test):** there is no crew-count knockout gate. A tank is fully
//! dead only at 0 living crew. Capability effectiveness is per-capability (§7b): a capability scores
//! in [0, 1] from its requirement groups against the live world — living crew and intact modules
//! supply quality, combined `min` across groups. The requirements are per-tank RON data.
//! `TankKnockedOut` survives as a *label only* — it never gates gameplay. Cookoff still kills all
//! crew and launches the turret; that hook triggers off `CookedOff` directly.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, LinearVelocity, Mass, NoAutoAngularInertia, NoAutoMass,
    Position, RigidBody, Rotation,
};
use bevy::ecs::query::QueryData;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ballistics::ComponentHealth;
use crate::command::{ConsumeCommandEdges, CrewSwap, TankCommand};
use crate::state::GameplaySet;
use crate::tank::{Controlled, Rig, ServoCommand, ServoIndex, ServoSpec, Tank, Turret};

/// Semantic ownership: a ballistic volume belongs to a tank for gameplay aggregation. This is
/// separate from `ChildOf`, which remains the model/transform hierarchy.
#[derive(Component, Debug)]
#[relationship(relationship_target = TankVolumes)]
pub struct VolumeOf(pub Entity);

impl VolumeOf {
    pub fn tank(&self) -> Entity {
        self.0
    }
}

/// Reverse collection for [`VolumeOf`]. Bevy keeps this synchronized; read it, don't mutate it.
#[derive(Component, Debug)]
#[relationship_target(relationship = VolumeOf)]
pub struct TankVolumes(Vec<Entity>);

/// Resolve a spatial-query hit to the nearest ancestor carrying `T`. Colliders (armor trimeshes,
/// collision proxies) are spawned as bare children of their named node, so a raycast hit lands on
/// the mesh-primitive entity while the semantic components (`BallisticVolume`, [`VolumeOf`]) live
/// on an ancestor. This is the ONE hierarchy-resolution rule, shared by the shell march, the spall
/// fragments, and the aim rays — one walk, so a change to the spawned shape cannot make the aim
/// dots silently disagree with the shells they predict. `None` (no such ancestor) ⇒ terrain.
pub fn hit_ancestor<'a, T: Component>(
    hit: Entity,
    components: &'a Query<&T>,
    parents: &Query<&ChildOf>,
) -> Option<(Entity, &'a T)> {
    let mut probe = hit;
    loop {
        if let Ok(found) = components.get(probe) {
            return Some((probe, found));
        }
        probe = parents.get(probe).ok()?.parent();
    }
}

impl TankVolumes {
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.0.iter().copied()
    }
}

/// A crew volume's station identity — which seat this volume is ("Commander", "Gunner", …). Used
/// for diagnostic readouts and slice-2 backfill ("who is where"). This is identity/label only —
/// gating keys off [`Capability`] requirements (via [`Part`]), not the role itself. `Ord` is by
/// declaration order (Commander < … < BowGunner) — the deterministic seat order the crew bar uses.
#[derive(
    Component,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Reflect,
)]
pub enum CrewStation {
    Commander,
    Gunner,
    Loader,
    Driver,
    BowGunner,
}

impl CrewStation {
    pub fn label(self) -> &'static str {
        match self {
            CrewStation::Commander => "Commander",
            CrewStation::Gunner => "Gunner",
            CrewStation::Loader => "Loader",
            CrewStation::Driver => "Driver",
            CrewStation::BowGunner => "Bow gunner",
        }
    }
}

/// The occupant currently manning a station-volume (topology B, design §7b). Its HP/[`Dead`] *are*
/// the volume's — body and seat share the entity — but `home` is the occupant's native station
/// (specialty), which after a backfill swap may differ from the seat's [`CrewStation`]. A swap
/// exchanges occupant state (HP, `Dead`, `home`) between two seats, so the *living* hitbox moves with
/// the person. `competence(home, seat)` then degrades whatever foreign seat the occupant serves.
#[derive(Component, Clone, Copy, Debug)]
pub struct Crewman {
    pub home: CrewStation,
}

/// How well a crewman serves a station, ∈ [0, 1]: native (`home == seat`) → 1.0; any foreign seat →
/// a flat 0.6 backfill penalty for now. Later keyed by per-crewman skill/training (design §7b, §9) —
/// this is the seam that stays, with only the body of the function growing.
pub fn competence(home: CrewStation, seat: CrewStation) -> f32 {
    if home == seat { 1.0 } else { 0.6 }
}

/// A backfill swap in flight on a tank (design §7b): when `remaining` reaches 0 the occupants of
/// seats `a` and `b` exchange. **Source-live falls out for free** — nothing changes until it fires,
/// so the crewman keeps serving his old seat throughout the transition. Cancellable (remove the
/// component); not pausable.
#[derive(Component)]
pub struct PendingSwap {
    pub a: Entity,
    pub b: Entity,
    pub remaining: f32,
}

/// Seconds a crew swap takes — the hardcore time-cost (design §7b, register F1). A live tuning knob.
pub const SWAP_SECONDS: f32 = 4.0;

/// A repairable module function served by this volume. Function loss is derived from its HP.
/// Identity/label only — gating keys off [`Capability`] requirements (via [`Part`]), not the function.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum FunctionRole {
    Engine,
    Transmission,
    Breech,
    GunBarrel,
}

impl FunctionRole {
    pub fn label(self) -> &'static str {
        match self {
            FunctionRole::Engine => "Engine",
            FunctionRole::Transmission => "Transmission",
            FunctionRole::Breech => "Breech",
            FunctionRole::GunBarrel => "Gun barrel",
        }
    }
}

/// A gameplay verb the tank can perform (design §7b). Closed enum — consuming systems (driving,
/// shooting, cameras) reference variants directly. Adding one is a one-line enum change + one
/// consuming system. The *requirements* for each capability are per-tank RON data
/// ([`TankCapabilities`]); its current degree is its effectiveness ([`capability_effectiveness`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Capability {
    Drive,
}

impl Capability {
    pub fn label(self) -> &'static str {
        match self {
            Capability::Drive => "Drive",
        }
    }
}

/// A reference to any quality-bearing thing on the tank — a crew station or a module function, in
/// one flat vocabulary (design §7b). No `Crew(..)`/`Module(..)` wrapper: crew-vs-module is intrinsic
/// to the resolved volume (it carries a [`CrewStation`] or [`FunctionRole`]), not the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Part {
    Commander,
    Gunner,
    Loader,
    Driver,
    BowGunner,
    Engine,
    Transmission,
    Breech,
    GunBarrel,
}

impl From<CrewStation> for Part {
    fn from(s: CrewStation) -> Self {
        match s {
            CrewStation::Commander => Part::Commander,
            CrewStation::Gunner => Part::Gunner,
            CrewStation::Loader => Part::Loader,
            CrewStation::Driver => Part::Driver,
            CrewStation::BowGunner => Part::BowGunner,
        }
    }
}

impl From<FunctionRole> for Part {
    fn from(f: FunctionRole) -> Self {
        match f {
            FunctionRole::Engine => Part::Engine,
            FunctionRole::Transmission => Part::Transmission,
            FunctionRole::Breech => Part::Breech,
            FunctionRole::GunBarrel => Part::GunBarrel,
        }
    }
}

/// One contributor within a [`GradedGroup`]: `(coefficient, parts)`. `coefficient` is the member's
/// share (`Pool`) or ceiling (`Backup`); the qualities of the `parts` it depends on multiply into
/// it. An empty `parts` list = a pure ceiling with no dependency (e.g. a hand-crank backup path).
pub type Member = (f32, Vec<Part>);

/// A requirement group within a [`Capability`] (design §7b). Groups AND together (`min` across
/// groups). `Single` is a mandatory part (sugar for a one-member group at coefficient 1.0).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Group {
    /// A single mandatory part — missing → capability 0.
    Single(Part),
    /// A graded group: cooperative (`Pool`) or substitutive (`Backup`).
    Graded(GradedGroup),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum GradedGroup {
    /// Cooperative redundancy: present members' contributions sum, capped at 1.0 (e.g. two engines
    /// at 0.5 each; two loaders on a heavy gun).
    Pool(Vec<Member>),
    /// Substitutive redundancy: the best available path wins, `max` (powered vs hand traverse;
    /// autoloader vs hand-load). The primary path's own dependencies fold into its member.
    Backup(Vec<Member>),
}

/// A capability's requirement: a list of [`Group`]s combined by AND (`min` across groups).
pub type Requirement = Vec<Group>;

/// Per-tank capability requirements, loaded from the `.tank.ron` spec sheet and inserted on the tank
/// entity at bind time. Drives [`capability_effectiveness`] — the single gate consuming systems query.
#[derive(Component, Clone, Debug)]
pub struct TankCapabilities(pub std::collections::HashMap<Capability, Requirement>);

/// An ammunition volume. Depletion triggers a cookoff: all crew die immediately.
#[derive(Component)]
pub struct Ammo;

/// Latched crew state: this crewman is dead. Inserted once when HP ≤ 0; never removed in normal
/// play. The staffing query filters `Without<Dead>`. Slice-2 backfill/respawn may clear it.
#[derive(Component)]
pub struct Dead;

/// Latched ammo event: this ammunition volume has cooked off. Prevents repeated cookoff processing.
#[derive(Component)]
pub struct CookedOff;

/// Latched tank death label (not a gameplay gate — design §1a-A). Latches at 0 living crew or on
/// cookoff. Read by HUD/scoring; no system gates drive/fire on this. Normal play never removes it;
/// sandbox reset may.
#[derive(Component)]
pub struct TankKnockedOut {
    pub reason: KnockoutReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnockoutReason {
    CrewLoss,
    Cookoff,
}

impl KnockoutReason {
    pub fn label(self) -> &'static str {
        match self {
            KnockoutReason::CrewLoss => "crew loss",
            KnockoutReason::Cookoff => "ammo cookoff",
        }
    }
}

/// The damage-consequence chain (cookoff → crew death → tank-death label → turret launch). Labeled
/// so any producer of this tick's `ComponentHealth` can order itself BEFORE it: the authority's local
/// deposition (`ballistics`, same-frame in `GameplaySet`) and, on the net client, the replicated
/// health apply (`net::protocol::apply_net_health`) both land before the consequences read HP, so
/// death is interpreted from the freshest health on either end.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DamageConsequences;

pub fn plugin(app: &mut App) {
    // The crew-swap seam: commands start/cancel swaps on the fixed clock (consuming the edge);
    // `tick_swaps` runs the in-flight timer. The whole consequence chain is sim — it decides who
    // lives and what works — so it steps on the fixed clock with the rest of the simulation.
    app.add_systems(
        FixedUpdate,
        apply_crew_swap_commands
            .before(ConsumeCommandEdges)
            .in_set(GameplaySet),
    )
    .add_systems(
        FixedUpdate,
        (
            tick_swaps,
            process_cookoffs,
            kill_crew,
            mark_dead_tanks,
            launch_turrets_on_cookoff,
        )
            .chain()
            .in_set(GameplaySet)
            .in_set(DamageConsequences),
    );
}

/// Start or cancel a [`PendingSwap`] from each tank's command — the sim half of the crew-swap
/// seam (the crew bar's two-tap flow is client-side selection state). Stations resolve against
/// the tank's *own* seats, so the command is meaningful from any writer (local player or a
/// network peer) and the server owns the legality check: a `Start` needs both seats to exist and
/// no swap in flight; `Cancel` aborts an in-flight one.
fn apply_crew_swap_commands(
    tanks: Query<(Entity, &TankCommand, Option<&PendingSwap>), With<Tank>>,
    seats: Query<(Entity, &CrewStation, &VolumeOf)>,
    mut commands: Commands,
) {
    for (tank, command, pending) in &tanks {
        let Some(swap) = command.crew_swap else {
            continue;
        };
        match swap {
            CrewSwap::Cancel => {
                if pending.is_some() {
                    commands.entity(tank).remove::<PendingSwap>();
                }
            }
            CrewSwap::Start(a, b) => {
                if pending.is_some() || a == b {
                    continue;
                }
                let seat_of = |station: CrewStation| {
                    seats
                        .iter()
                        .find(|(_, s, owner)| **s == station && owner.tank() == tank)
                        .map(|(entity, ..)| entity)
                };
                let (Some(seat_a), Some(seat_b)) = (seat_of(a), seat_of(b)) else {
                    continue;
                };
                commands.entity(tank).insert(PendingSwap {
                    a: seat_a,
                    b: seat_b,
                    remaining: SWAP_SECONDS,
                });
            }
        }
    }
}

/// Tick pending crew swaps; on completion, exchange occupant state (`home`, HP, `Dead`) between the
/// two seats — so the *living* crewman's hitbox moves with the person (topology B, design §7b).
/// Read-then-`Commands`, so no disjoint mutable access is needed. The tank and its seats are distinct
/// entities, so the `&mut PendingSwap` (tank) and `&` seat reads never alias.
fn tick_swaps(
    time: Res<Time>,
    mut swaps: Query<(Entity, &mut PendingSwap)>,
    seats: Query<(&Crewman, &ComponentHealth, Option<&Dead>)>,
    mut commands: Commands,
) {
    for (tank, mut swap) in &mut swaps {
        swap.remaining -= time.delta_secs();
        if swap.remaining > 0.0 {
            continue;
        }
        // Read both occupants, then overwrite each with the other's state. If a seat vanished, drop
        // the swap rather than half-apply it.
        let (Ok((ca, ha, da)), Ok((cb, hb, db))) = (seats.get(swap.a), seats.get(swap.b)) else {
            commands.entity(tank).remove::<PendingSwap>();
            continue;
        };
        let occ_a = (*ca, ha.current, ha.max, da.is_some());
        let occ_b = (*cb, hb.current, hb.max, db.is_some());
        place_occupant(&mut commands, swap.a, occ_b);
        place_occupant(&mut commands, swap.b, occ_a);
        commands.entity(tank).remove::<PendingSwap>();
    }
}

/// Write an occupant (its `home`, HP, and alive/dead state) onto a seat-volume, overwriting whoever
/// was there. Helper for [`tick_swaps`].
fn place_occupant(
    commands: &mut Commands,
    seat: Entity,
    (crewman, current, max, dead): (Crewman, f32, f32, bool),
) {
    let mut e = commands.entity(seat);
    e.insert((crewman, ComponentHealth { current, max }));
    if dead {
        e.insert(Dead);
    } else {
        e.remove::<Dead>();
    }
}

/// The per-volume facets the capability system reads off each tank volume — a *named* form of the
/// 5-`Option` query that otherwise repeats verbatim across every control system. A volume carries
/// whatever subset applies: a crew seat has `crew` (+ `crewman`, maybe `dead`, `health`); a module
/// has `function` + `health`. Use it as `Query<VolumeFacets>`; `.get(e)` yields a struct with these
/// named fields.
#[derive(QueryData)]
pub struct VolumeFacets {
    pub crew: Option<&'static CrewStation>,
    pub crewman: Option<&'static Crewman>,
    pub dead: Option<&'static Dead>,
    pub function: Option<&'static FunctionRole>,
    pub health: Option<&'static ComponentHealth>,
}

/// How well this capability is currently served on this tank, ∈ [0, 1] (0 = unavailable, 1 = full)
/// — the *effectiveness* (design §7b). Resolves each [`Part`]'s live quality (living crew → 1.0,
/// intact module → 1.0; backfill competence and graded damage layer in later), then combines via
/// [`evaluate`]. Requirements are per-tank RON data ([`TankCapabilities`]); the query walks the
/// tank's volumes once.
pub fn capability_effectiveness(
    tank_volumes: Option<&TankVolumes>,
    tank_caps: Option<&TankCapabilities>,
    capability: Capability,
    volumes: &Query<VolumeFacets>,
) -> f32 {
    let (Some(tank_volumes), Some(tank_caps)) = (tank_volumes, tank_caps) else {
        return 0.0;
    };
    let Some(requirement) = tank_caps.0.get(&capability) else {
        return 0.0;
    };
    evaluate(requirement, &part_qualities(tank_volumes, volumes))
}

/// The live quality of each [`Part`] on a tank — the shared core behind *every* requirement check
/// (a tank-wide [`Capability`], a servo's slew gate, a weapon's fire/load gate): resolve once per
/// tank, then [`evaluate`] any number of requirements against it. Living crew → competence (native
/// 1.0 / foreign degraded); intact module (HP > 0) → 1.0 (graded damage layers in later); absent
/// → 0. Duplicate parts (two volumes of one station) take the best.
pub fn part_qualities(
    tank_volumes: &TankVolumes,
    volumes: &Query<VolumeFacets>,
) -> std::collections::HashMap<Part, f32> {
    let mut quality: std::collections::HashMap<Part, f32> = std::collections::HashMap::new();
    for volume in tank_volumes.iter() {
        let Ok(facets) = volumes.get(volume) else {
            continue;
        };
        // A living crew seat supplies its role at the occupant's competence (native 1.0 / foreign
        // degraded). `home` defaults to the seat when no occupant facet is present.
        if let (Some(role), None) = (facets.crew, facets.dead) {
            let home = facets.crewman.map(|c| c.home).unwrap_or(*role);
            let q = quality.entry(Part::from(*role)).or_insert(0.0);
            *q = q.max(competence(home, *role));
        }
        if let (Some(func), Some(hp)) = (facets.function, facets.health)
            && hp.current > 0.0
        {
            let q = quality.entry(Part::from(*func)).or_insert(0.0);
            *q = q.max(1.0);
        }
    }
    quality
}

/// The pure combine core (design §7b), split out so it is testable without a `World`:
/// `member = coeff × Π(part qualities)`; `group = Single part / Pool capped-sum / Backup max`;
/// `capability = min across groups`. A part absent from `quality` scores 0.
pub fn evaluate(requirement: &Requirement, quality: &std::collections::HashMap<Part, f32>) -> f32 {
    let part_quality = |p: Part| quality.get(&p).copied().unwrap_or(0.0);
    let member_quality =
        |(coeff, parts): &Member| parts.iter().fold(*coeff, |q, p| q * part_quality(*p));

    requirement.iter().fold(1.0_f32, |eff, group| {
        let group_value = match group {
            Group::Single(p) => part_quality(*p),
            Group::Graded(GradedGroup::Pool(members)) => {
                members.iter().map(member_quality).sum::<f32>().min(1.0)
            }
            Group::Graded(GradedGroup::Backup(members)) => {
                members.iter().map(member_quality).fold(0.0_f32, f32::max)
            }
        };
        eff.min(group_value)
    })
}

/// Is this capability usable at all (effectiveness > 0)? The boolean gate consuming systems use when
/// they only care about on/off; reach for [`capability_effectiveness`] when the *degree* matters.
pub fn capability_available(
    tank_volumes: Option<&TankVolumes>,
    tank_caps: Option<&TankCapabilities>,
    capability: Capability,
    volumes: &Query<VolumeFacets>,
) -> bool {
    capability_effectiveness(tank_volumes, tank_caps, capability, volumes) > 0.0
}

/// Whether an arbitrary tank *meets* a requirement (effectiveness > 0) — the per-tank analog of
/// [`ControlledTank::meets`], for sim systems that iterate every tank (a weapon's `fire`/`load`
/// gates in `shooting`). No volumes yet (spec still binding) → not met.
pub fn requirement_met(
    tank_volumes: Option<&TankVolumes>,
    requirement: &Requirement,
    volumes: &Query<VolumeFacets>,
) -> bool {
    let Some(tank_volumes) = tank_volumes else {
        return false;
    };
    evaluate(requirement, &part_qualities(tank_volumes, volumes)) > 0.0
}

/// The player's tank, bundled for the control systems (drive input, aiming, sight, shooting). It
/// folds together the three things those systems always reach for as a unit — *which* tank is
/// controlled, *where its parts are* ([`Rig`]), and *what it can still do* (capabilities) — so a
/// system takes one `ControlledTank` param instead of repeating a controlled-tank query, a
/// [`VolumeFacets`] query, and a `capability_available(..)` call. Scoped to the single [`Controlled`]
/// tank; per-tank consumers ([`apply_drive`](crate::driving), the HUD) keep using [`VolumeFacets`]
/// directly since they iterate every tank.
#[derive(SystemParam)]
pub struct ControlledTank<'w, 's> {
    tank: Query<'w, 's, (Entity, &'static Rig, Option<&'static TankVolumes>), With<Controlled>>,
    volumes: Query<'w, 's, VolumeFacets>,
}

impl ControlledTank<'_, '_> {
    /// The controlled tank's entity, or `None` if there isn't exactly one controlled tank.
    pub fn entity(&self) -> Option<Entity> {
        self.tank.single().ok().map(|(entity, ..)| entity)
    }

    /// The controlled tank's resolved rig-node handles (`rig.gun`, `rig.turret`, …), or `None`.
    pub fn rig(&self) -> Option<&Rig> {
        self.tank.single().ok().map(|(_, rig, ..)| rig)
    }

    /// Whether the controlled tank *meets* an arbitrary requirement (effectiveness > 0) — for the
    /// per-weapon / per-part gates (a weapon's `fire`/`load`) that aren't tank-wide capabilities.
    /// An empty requirement is vacuously met. `None` controlled → not met.
    pub fn meets(&self, requirement: &Requirement) -> bool {
        let Ok((_, _, Some(tank_volumes))) = self.tank.single() else {
            return false;
        };
        evaluate(requirement, &part_qualities(tank_volumes, &self.volumes)) > 0.0
    }
}

fn process_cookoffs(
    ammo: Query<(Entity, &ComponentHealth, &VolumeOf), (With<Ammo>, Without<CookedOff>)>,
    mut crew: Query<(&VolumeOf, &mut ComponentHealth), (With<CrewStation>, Without<Ammo>)>,
    mut commands: Commands,
) {
    for (ammo_entity, hp, owner) in &ammo {
        if hp.current > 0.0 {
            continue;
        }
        commands.entity(ammo_entity).insert(CookedOff);
        commands.entity(owner.tank()).insert(TankKnockedOut {
            reason: KnockoutReason::Cookoff,
        });
        for (crew_owner, mut crew_hp) in &mut crew {
            if crew_owner.tank() == owner.tank() {
                crew_hp.current = 0.0;
            }
        }
    }
}

#[derive(Component)]
pub struct LaunchedTurret;

fn launch_turrets_on_cookoff(
    // Authority-only: launching the turret spawns a free PHYSICS body, and unlike a purely logical
    // consequence (crew death, capability loss) that emerges correctly from replicated health, a
    // client-local launch starts from an unsynced avian Position (pops to origin) and re-fires on
    // every reconnect via `Added<CookedOff>` (both observed). So the net client does NOT launch
    // locally; the authoritative launched turret is replicated to it in a follow-up slice. SP /
    // sandbox / server (the authorities) still launch here.
    replica: Option<Res<crate::ClientReplica>>,
    cooked_ammo: Query<&VolumeOf, (With<CookedOff>, Added<CookedOff>)>,
    turrets: Query<(Entity, &GlobalTransform), (With<Turret>, Without<LaunchedTurret>)>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if replica.is_some() {
        return;
    }
    for ammo_owner in &cooked_ammo {
        let tank = ammo_owner.tank();
        for (turret, global) in &turrets {
            if !is_descendant_of(turret, tank, &parents) {
                continue;
            }
            let pose = global.compute_transform();
            let up = Vec3::from(global.up());
            let right = Vec3::from(global.right());
            let forward = Vec3::from(global.forward());
            const TURRET_MASS: f32 = 8_000.0;
            // Detach FIRST, then insert the free body — in one command batch so `ChildOf` is gone
            // before avian's `RigidBody` on-add hook (`init_physics_transform`) fires. That hook
            // runs at command-flush time via `DeferredWorld`, and if `ChildOf` is still present it
            // DERIVES `Position` by walking the parent chain and composing it with our `Transform`
            // — but `pose` is already the WORLD transform, so the parent's world transform gets
            // applied a second time (measured: a correct (4.03, 1.75, 11.21) turret published as a
            // double-transformed (-4.58, 1.73, 2.91)). We seed EXPLICIT world `Position`/`Rotation`
            // (before `RigidBody`, the placeholder-NaN discipline) so the hook uses them verbatim
            // instead of deriving; keeping `Transform` too for the view/mesh.
            commands
                .entity(turret)
                .remove::<(ChildOf, Turret, ServoCommand, ServoIndex, ServoSpec)>()
                .insert((
                    Position(pose.translation),
                    Rotation(pose.rotation),
                    pose,
                    RigidBody::Dynamic,
                    Mass(TURRET_MASS),
                    AngularInertia::from_shape(&Cuboid::new(3.0, 1.2, 2.4), TURRET_MASS),
                    NoAutoMass,
                    NoAutoAngularInertia,
                    LinearVelocity(up * 14.0 + forward * 3.0),
                    AngularVelocity(right * 3.0 + up * 1.2),
                    LaunchedTurret,
                ));
        }
    }
}

fn is_descendant_of(mut entity: Entity, ancestor: Entity, parents: &Query<&ChildOf>) -> bool {
    while let Ok(parent) = parents.get(entity) {
        entity = parent.parent();
        if entity == ancestor {
            return true;
        }
    }
    false
}

fn kill_crew(
    crew: Query<(Entity, &ComponentHealth), (With<CrewStation>, Without<Dead>)>,
    mut commands: Commands,
) {
    for (entity, hp) in &crew {
        if hp.current <= 0.0 {
            commands.entity(entity).insert(Dead);
        }
    }
}

/// Latch the tank-death label at 0 living crew (design §1a-A). This is a *label*, not a gameplay
/// gate — no system reads `TankKnockedOut` to disable capabilities. Cookoff inserts its own
/// `TankKnockedOut { reason: Cookoff }` directly (preserving the reason); this system only latches
/// the crew-loss reason for tanks that aren't already labeled.
fn mark_dead_tanks(
    tanks: Query<(Entity, &TankVolumes), Without<TankKnockedOut>>,
    volumes: Query<(Option<&CrewStation>, Option<&Dead>)>,
    mut commands: Commands,
) {
    for (tank, tank_volumes) in &tanks {
        let mut crew_total = 0;
        let mut crew_living = 0;
        for volume in tank_volumes.iter() {
            let Ok((station, dead)) = volumes.get(volume) else {
                continue;
            };
            if station.is_some() {
                crew_total += 1;
                if dead.is_none() {
                    crew_living += 1;
                }
            }
        }
        if crew_total > 0 && crew_living == 0 {
            commands.entity(tank).insert(TankKnockedOut {
                reason: KnockoutReason::CrewLoss,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GradedGroup, Group, Part, evaluate};
    use std::collections::HashMap;

    /// Build a part→quality map from `(part, quality)` pairs.
    fn q(pairs: &[(Part, f32)]) -> HashMap<Part, f32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn single_groups_are_and_all_or_nothing() {
        // Fire = [Gunner, Breech, GunBarrel] — the Tiger shape.
        let req = vec![
            Group::Single(Part::Gunner),
            Group::Single(Part::Breech),
            Group::Single(Part::GunBarrel),
        ];
        let all = q(&[
            (Part::Gunner, 1.0),
            (Part::Breech, 1.0),
            (Part::GunBarrel, 1.0),
        ]);
        assert_eq!(evaluate(&req, &all), 1.0);
        // Drop the breech → capability gone (min across groups).
        let no_breech = q(&[(Part::Gunner, 1.0), (Part::GunBarrel, 1.0)]);
        assert_eq!(evaluate(&req, &no_breech), 0.0);
    }

    #[test]
    fn pool_is_capped_cooperative_sum() {
        // Two engines at 0.5 each: both → 1.0, one → 0.5, neither → 0.
        let req = vec![Group::Graded(GradedGroup::Pool(vec![
            (0.5, vec![Part::Engine]),
            (0.5, vec![Part::Transmission]), // stand-in for a second engine part
        ]))];
        assert_eq!(
            evaluate(&req, &q(&[(Part::Engine, 1.0), (Part::Transmission, 1.0)])),
            1.0
        );
        assert_eq!(evaluate(&req, &q(&[(Part::Engine, 1.0)])), 0.5);
        assert_eq!(evaluate(&req, &q(&[])), 0.0);
    }

    #[test]
    fn pool_caps_boolean_redundancy_at_one() {
        // Two full-share members (firing circuit): any one → 1.0, both → still 1.0 (capped).
        let req = vec![Group::Graded(GradedGroup::Pool(vec![
            (1.0, vec![Part::Breech]),
            (1.0, vec![Part::GunBarrel]),
        ]))];
        assert_eq!(evaluate(&req, &q(&[(Part::Breech, 1.0)])), 1.0);
        assert_eq!(
            evaluate(&req, &q(&[(Part::Breech, 1.0), (Part::GunBarrel, 1.0)])),
            1.0
        );
    }

    #[test]
    fn backup_takes_the_best_path() {
        // Powered (1.0, [TraverseMotor stand-in]) vs hand-crank (0.1, []).
        let req = vec![Group::Graded(GradedGroup::Backup(vec![
            (1.0, vec![Part::Engine]), // stand-in for a powered-drive part
            (0.1, vec![]),             // hand-crank: pure ceiling, no dependency
        ]))];
        // Powered up → 1.0.
        assert_eq!(evaluate(&req, &q(&[(Part::Engine, 1.0)])), 1.0);
        // Powered down → falls back to the 0.1 hand path (max, not sum).
        assert!((evaluate(&req, &q(&[])) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn member_quality_multiplies_dependencies() {
        // A degraded part scales its member: coeff 1.0 × quality 0.6 = 0.6 (competence preview).
        let req = vec![Group::Graded(GradedGroup::Backup(vec![(
            1.0,
            vec![Part::Loader],
        )]))];
        assert!((evaluate(&req, &q(&[(Part::Loader, 0.6)])) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn competence_is_native_or_flat_foreign() {
        use super::{CrewStation, competence};
        // Native: a loader in the loader's seat is full.
        assert_eq!(competence(CrewStation::Loader, CrewStation::Loader), 1.0);
        // Foreign: a commander backfilling the loader's seat is degraded.
        assert_eq!(competence(CrewStation::Commander, CrewStation::Loader), 0.6);
    }
}
