//! Consequences in the game's damage model: module condition and emptied HP pools. A crew member
//! whose HP pool is emptied is knocked out (latched as `Dead`); an ammunition volume whose pool is
//! emptied cooks off; a module whose pool is emptied is out of action and stops supplying the
//! capabilities that need it; and a tank carries a knocked-out label. Ballistics owns *how* a dose
//! is deposited; this module owns what an emptied pool means.
//!
//! A tank is knocked out when fewer than two crew members remain in action. Capability
//! effectiveness is per-capability: a capability scores in [0, 1] from its requirement groups
//! against the live world — crew still in action and modules whose pools are not empty supply
//! quality, combined `min` across groups. The
//! requirements are per-tank RON data. `TankKnockedOut` survives as a *label only* — it never gates
//! sim capability (on the client it takes the player's device from the tank:
//! `command::DeviceOwner`). A cookoff empties every crew HP pool of its tank at once and launches
//! the turret; that hook triggers off `CookedOff` directly.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, LinearVelocity, Mass, NoAutoAngularInertia, NoAutoMass,
    Position, Rotation,
};
use bevy::ecs::query::QueryData;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub(crate) mod repair;

use crate::ShotId;
use crate::ballistics::ComponentHealth;
use crate::command::{ConsumeCommandEdges, CrewSwap, TankCommand};
use crate::state::GameplaySet;
use crate::tank::{Controlled, Rig, ServoCommand, ServoIndex, ServoSpec, Tank, Turret, TurretBody};

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

/// Why a [`TankVolumes`] entry always resolves in a query whose terms are all optional: the
/// relationship's despawn hook removes a volume from its tank's list as the volume goes, so every
/// listed entity is live.
const LIVE_VOLUME: &str = "a TankVolumes entry is a live entity: the VolumeOf relationship drops it \
                           on despawn";

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

/// The game's damage model: native competence is full; foreign competence is the shared
/// stand-in until authored training supplies the crew/station relation (design §7b, §9).
pub fn competence(home: CrewStation, seat: CrewStation) -> f32 {
    if home == seat {
        1.0
    } else {
        foreign_competence()
    }
}

fn foreign_competence() -> f32 {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CrewData {
        foreign_competence: crate::provenance::Sourced<f32>,
    }
    static DATA: std::sync::LazyLock<CrewData> = std::sync::LazyLock::new(|| {
        let data: CrewData = crate::provenance::from_shipped_str(
            "assets/damage/crew.ron",
            include_str!("../assets/damage/crew.ron"),
        )
        .expect("assets/damage/crew.ron: foreign_competence needs a value and provenance");
        assert!(
            data.foreign_competence.0 > 0.0 && data.foreign_competence.0 <= 1.0,
            "assets/damage/crew.ron: foreign_competence must be finite and in (0, 1]; author a positive work-rate share"
        );
        data
    });
    DATA.foreign_competence.0
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
/// Identity only — gating keys off [`Capability`] requirements (via [`Part`]), not the function.
///
/// A requirement names a function, and [`part_facts`] max-combines every volume serving one, so
/// a weapon gated on its own barrel needs a barrel function no other weapon's barrel serves: the
/// main gun's ([`Self::GunBarrel`]), the coaxial machine gun's and the bow machine gun's.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[require(repair::ModuleRepair)]
pub enum FunctionRole {
    Engine,
    Transmission,
    Breech,
    GunBarrel,
    CoaxMgBarrel,
    BowMgBarrel,
}

/// A gameplay verb the tank can perform (design §7b). Closed enum — consuming systems (driving,
/// shooting, cameras) reference variants directly. Adding one is a variant, its entry in
/// [`Self::ALL`] and one consuming system. The *requirements* for each capability are per-tank RON data
/// ([`TankCapabilities`]); each consumer reads its requirement through [`resolve`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Capability {
    Drive,
}

impl Capability {
    /// Every capability: a tank's sheet states each one (`TankSpec::validate`).
    pub const ALL: [Self; 1] = [Self::Drive];

    pub fn label(self) -> &'static str {
        match self {
            Capability::Drive => "Drive",
        }
    }
}

/// A reference to any quality-bearing thing on the tank — a crew station or a module function, in
/// one flat vocabulary (design §7b). No `Crew(..)`/`Module(..)` wrapper: crew-vs-module is intrinsic
/// to the resolved volume (it carries a [`CrewStation`] or [`FunctionRole`]), not the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
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
    CoaxMgBarrel,
    BowMgBarrel,
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
            FunctionRole::CoaxMgBarrel => Part::CoaxMgBarrel,
            FunctionRole::BowMgBarrel => Part::BowMgBarrel,
        }
    }
}

/// One contributor within a [`GradedGroup`]: `(coefficient, parts)`. `coefficient` is the member's
/// share (`Pool`) or ceiling (`Backup`), in (0, 1] (the sheet's `validate` refuses any other); the
/// qualities of the `parts` it depends on multiply into it. An empty `parts` list = a pure ceiling
/// with no dependency (e.g. a hand-crank backup path).
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
/// entity at bind time. Its consumer is the drive selector, `track::sim::drive_target`.
#[derive(Component, Clone, Debug)]
pub struct TankCapabilities {
    pub requirements: std::collections::HashMap<Capability, Requirement>,
    pub drive_controls: ControlStaffing,
}

/// The game's damage model: staffing grants control authority independently of task rate.
/// Each alternative lists all stations that must be in action; no module condition or
/// contribution coefficient can grant or revoke that authority.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum ControlStaffing {
    Unstaffed,
    Any(Vec<Vec<CrewStation>>),
}

impl ControlStaffing {
    pub fn permits(&self, staffed: &std::collections::HashSet<CrewStation>) -> bool {
        match self {
            Self::Unstaffed => true,
            Self::Any(paths) => paths
                .iter()
                .any(|stations| stations.iter().all(|station| staffed.contains(station))),
        }
    }
}

impl TankCapabilities {
    pub fn new(
        requirements: std::collections::HashMap<Capability, Requirement>,
        drive_controls: ControlStaffing,
    ) -> Self {
        Self {
            requirements,
            drive_controls,
        }
    }

    /// The requirement the sheet states for `capability`.
    pub fn requirement(&self, capability: Capability) -> &Requirement {
        self.requirements.get(&capability).unwrap_or_else(|| {
            panic!(
                "{capability:?}: TankSpec::validate refuses a sheet that does not state every capability"
            )
        })
    }
}

/// An ammunition volume. Depletion triggers a cookoff: every crew HP pool is emptied.
#[derive(Component)]
pub struct Ammo;

/// Latched crew state: this crewman is knocked out. Inserted once when HP ≤ 0; never removed in normal
/// play. The staffing query filters `Without<Dead>`. Slice-2 backfill/respawn may clear it.
#[derive(Component)]
pub struct Dead;

/// Latched ammo event: this ammunition volume has cooked off. Prevents repeated cookoff processing.
#[derive(Component)]
pub struct CookedOff;

/// Latched tank knockout label (not a capability gate). Latches by the one knockout rule,
/// [`knockout_from_counts`]: fewer than two crew members in action. Read by HUD/scoring and the
/// client's death flow (which owns the player's device while it stands: `command::DeviceOwner`);
/// no sim system gates drive/fire on this. Normal play never removes it; sandbox reset may.
#[derive(Component)]
pub struct TankKnockedOut {
    pub reason: KnockoutReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// The authority's knockout transition: `tank` gained [`TankKnockedOut`] this tick (once per life)
/// and `cause` is what decided it, by the pen cam recorder's rule that also names a record's kill
/// (`damage_cam::recorder`, which holds the tick's emptied pools). Raised on the authority only,
/// for every knocked-out tank; the match score reads it (`net::score`).
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct TankKnockout {
    pub(crate) tank: Entity,
    pub(crate) cause: KnockoutCause,
}

/// What decided a knockout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KnockoutCause {
    /// The keyed round whose own deposition emptied the box that decided it.
    Round(ShotId),
    /// No keyed round emptied that box this tick: a round with no network identity (the sandbox's
    /// tank-less fire), or a knockout no round's deposition decided.
    NoKeyedRound,
}

/// The authority's one knockout rule (#227): a tank is knocked out when fewer than two of its crew
/// members are in action. A cook-off is a cause, not a second rule: it empties every crew HP pool
/// of its tank at once ([`process_cookoffs`]), so this rule decides it, and `cooked` (an ammunition
/// volume of the tank has cooked off) names it as the cause. A tank whose every module is out of
/// action stays in the Battle while two crew members are in action. Replicas receive the reason
/// through public `NetTankStatus`; they do not re-decide it from private crew detail.
pub(crate) fn knockout_from_counts(crew_in_action: u32, cooked: bool) -> Option<KnockoutReason> {
    (crew_in_action < 2).then_some(if cooked {
        KnockoutReason::Cookoff
    } else {
        KnockoutReason::CrewLoss
    })
}

/// The game's damage-model consequence chain (cookoff → crew knockout → tank label → turret launch). Labeled
/// so any producer of this tick's `ComponentHealth` can order itself BEFORE it: the authority's local
/// deposition (`ballistics`, same-frame in `GameplaySet`) lands before the consequences read HP. On
/// the net client this whole chain is gated off. Owner-private `NetCrew` realizes detail, while
/// public `NetTankStatus` realizes the authority's knockout reason.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DamageConsequences;

pub fn plugin(app: &mut App) {
    foreign_competence();
    repair::install(app);
    // A launched turret's colliders change body; the rule keeps their contact pairs naming it.
    app.add_plugins(crate::collider_rehome::plugin);
    // The crew-swap seam: commands start/cancel swaps on the fixed clock (consuming the edge);
    // `tick_swaps` runs the in-flight timer. The whole consequence chain is sim — it decides who
    // lives and what works — so it steps on the fixed clock with the rest of the simulation.
    //
    // **AUTHORITY-ONLY on a net client.** Both this seam and the whole [`DamageConsequences`] chain
    // are the *deciding* systems (start/time/apply a swap; latch crew knockout; cook off; launch the turret)
    // — server-owned under authoritative multiplayer. On the net client, `NetCrew` realizes private
    // crew detail and `NetTankStatus` realizes public knockout state. Running these systems there
    // would re-decide authority outcomes from re-assertable local state. The gate is
    // the `ClientReplica` resource (present only on the net client — single-player, the sandbox, and
    // the dedicated server are all authorities and keep running the chain). Same discriminator
    // `ballistics` and `launch_turrets_on_cookoff` already use.
    app.add_systems(
        FixedUpdate,
        apply_crew_swap_commands
            .before(ConsumeCommandEdges)
            .in_set(GameplaySet)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
    )
    .add_systems(
        FixedPostUpdate,
        (
            tick_swaps,
            process_cookoffs,
            kill_crew,
            mark_dead_tanks,
            launch_turrets_on_cookoff,
        )
            .chain()
            .in_set(GameplaySet)
            .in_set(DamageConsequences)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
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
        // Read both occupants, then overwrite each with the other's state.
        let [(ca, ha, da), (cb, hb, db)] = [swap.a, swap.b].map(|seat| {
            seats.get(seat).expect(
                "a swap's seats are its tank's crew volumes, spawned with occupant and HP pool, \
                 and no system despawns a volume apart from its tank",
            )
        });
        let occ_a = (*ca, *ha, da.is_some());
        let occ_b = (*cb, *hb, db.is_some());
        place_occupant(&mut commands, swap.a, occ_b);
        place_occupant(&mut commands, swap.b, occ_a);
        commands.entity(tank).remove::<PendingSwap>();
    }
}

/// Write an occupant (its `home`, HP, and knocked-out state) onto a seat-volume, overwriting whoever
/// was there. Helper for [`tick_swaps`].
fn place_occupant(
    commands: &mut Commands,
    seat: Entity,
    (crewman, health, dead): (Crewman, ComponentHealth, bool),
) {
    let mut e = commands.entity(seat);
    e.insert((crewman, health));
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
    pub repair: Option<&'static repair::ModuleRepair>,
}

/// The game's damage model: independently observed source qualities and station staffing.
pub struct PartFacts {
    pub quality: std::collections::HashMap<Part, f32>,
    pub staffed: std::collections::HashSet<CrewStation>,
}

/// The game's damage model: collect each tank's facts once. A station in action supplies its
/// occupant's competence; a module supplies its held HP share. Absent sources supply zero.
/// Duplicate functions retain their best provider; staffing records station presence directly.
pub fn part_facts(tank_volumes: &TankVolumes, volumes: &Query<VolumeFacets>) -> PartFacts {
    let mut quality: std::collections::HashMap<Part, f32> = std::collections::HashMap::new();
    let mut staffed = std::collections::HashSet::new();
    for volume in tank_volumes.iter() {
        let facets = volumes.get(volume).expect(LIVE_VOLUME);
        // A crew station in action supplies its role at the occupant's competence (native 1.0 / foreign
        // degraded).
        if let (Some(role), None) = (facets.crew, facets.dead) {
            staffed.insert(*role);
            let home = facets
                .crewman
                .expect("a crew station is spawned with its occupant (tank::spawn)")
                .home;
            let q = quality.entry(Part::from(*role)).or_insert(0.0);
            *q = q.max(competence(home, *role));
        }
        if let (Some(func), Some(hp)) = (facets.function, facets.health)
            && !hp.is_empty()
        {
            let q = quality.entry(Part::from(*func)).or_insert(0.0);
            *q = q.max(module_share(hp));
        }
    }
    PartFacts { quality, staffed }
}

/// The game's damage model: the shared held share for simulation and rate presentation.
/// Computing the ratio before narrowing avoids rounding both integer operands to whole.
/// A reduced pool stays below whole even when its ratio is between adjacent f32 values.
pub fn module_share(health: &ComponentHealth) -> f32 {
    if health.is_empty() {
        0.0
    } else if health.held() == health.capacity() {
        1.0
    } else {
        let ratio = (f64::from(health.held()) / f64::from(health.capacity())) as f32;
        // Resolution: the adjacent representable fraction preserves the reduced/whole boundary.
        ratio.min(f32::from_bits(1.0_f32.to_bits() - 1))
    }
}

/// The game's damage model: a complete authored path and its evaluated contribution.
/// `index` addresses the member in its group; a Single group has one member.
#[derive(Debug)]
pub struct ResolvedMember<'a> {
    pub index: usize,
    pub parts: &'a [Part],
    pub rate: f32,
}

/// The game's damage model: Pool retains every contribution; Backup retains its selected
/// complete alternative, including an unavailable path whose rate is zero.
#[derive(Debug)]
pub struct ResolvedGroup<'a> {
    pub rate: f32,
    pub contributors: Vec<ResolvedMember<'a>>,
}

/// The game's damage model: task throughput beside the complete paths that supplied it.
#[derive(Debug)]
pub struct ResolvedRequirement<'a> {
    pub rate: f32,
    pub groups: Vec<ResolvedGroup<'a>>,
}

pub fn resolve<'a>(
    requirement: &'a Requirement,
    quality: &std::collections::HashMap<Part, f32>,
) -> ResolvedRequirement<'a> {
    let groups: Vec<_> = requirement
        .iter()
        .map(|group| combine_group(group, quality))
        .collect();
    let mut result = ResolvedRequirement { rate: 1.0, groups };
    result.rate = result
        .groups
        .iter()
        .map(|group| group.rate)
        .fold(1.0, f32::min);
    result
}

/// The pure combine core (design §7b): member coefficient times its parts' qualities;
/// group Single / Pool capped-sum / Backup best whole path; minimum across groups.
/// An absent part supplies no quality.
pub fn evaluate(requirement: &Requirement, quality: &std::collections::HashMap<Part, f32>) -> f32 {
    resolve(requirement, quality).rate
}

pub(crate) fn live_quality(quality: &std::collections::HashMap<Part, f32>, part: Part) -> f32 {
    // Absent parts are a legitimate unavailable source, including an out-of-action station.
    quality.get(&part).copied().unwrap_or(0.0)
}

fn member_quality(member: &Member, quality: impl Fn(Part) -> f32) -> f32 {
    member.1.iter().fold(member.0, |q, p| q * quality(*p))
}

fn combine_group<'a>(
    group: &'a Group,
    quality: &std::collections::HashMap<Part, f32>,
) -> ResolvedGroup<'a> {
    let member = |index, parts, rate| ResolvedMember { index, parts, rate };
    match group {
        Group::Single(part) => {
            let rate = live_quality(quality, *part);
            ResolvedGroup {
                rate,
                contributors: vec![member(0, std::slice::from_ref(part), rate)],
            }
        }
        Group::Graded(GradedGroup::Pool(members)) => {
            let contributors: Vec<_> = members
                .iter()
                .enumerate()
                .map(|(index, m)| {
                    member(
                        index,
                        m.1.as_slice(),
                        member_quality(m, |part| live_quality(quality, part)),
                    )
                })
                .collect();
            let mut result = ResolvedGroup {
                rate: 0.0,
                contributors,
            };
            result.rate = result
                .contributors
                .iter()
                .map(|member| member.rate)
                .sum::<f32>()
                .min(1.0);
            result
        }
        Group::Graded(GradedGroup::Backup(members)) => {
            let selected = members
                .iter()
                .enumerate()
                .map(|(index, m)| {
                    member(
                        index,
                        m.1.as_slice(),
                        member_quality(m, |part| live_quality(quality, part)),
                    )
                })
                .max_by(|a, b| {
                    let canonical = |parts: &[Part]| {
                        let mut parts = parts.to_vec();
                        parts.sort_unstable();
                        parts
                    };
                    a.rate
                        .total_cmp(&b.rate)
                        .then_with(|| members[a.index].0.total_cmp(&members[b.index].0))
                        .then_with(|| canonical(a.parts).cmp(&canonical(b.parts)))
                })
                .expect(
                    "TankSpec::validate refuses empty Backup member lists in every requirement",
                );
            ResolvedGroup {
                rate: selected.rate,
                contributors: vec![selected],
            }
        }
    }
}

/// Whether an arbitrary tank *meets* a requirement (effectiveness > 0) — the per-tank analog of
/// [`ControlledTank::meets`], for sim systems that iterate every tank (a weapon's `fire`
/// gate in `shooting`; loading consumes a rate). No volumes yet (spec still binding) → not met.
pub fn requirement_met(
    tank_volumes: Option<&TankVolumes>,
    requirement: &Requirement,
    volumes: &Query<VolumeFacets>,
) -> bool {
    requirement_effectiveness(tank_volumes, requirement, volumes) > 0.0
}

/// The game's damage model: the rate served by a weapon or other local requirement.
/// Before a tank's volumes exist it supplies no work.
pub fn requirement_effectiveness(
    tank_volumes: Option<&TankVolumes>,
    requirement: &Requirement,
    volumes: &Query<VolumeFacets>,
) -> f32 {
    let Some(tank_volumes) = tank_volumes else {
        return 0.0;
    };
    evaluate(requirement, &part_facts(tank_volumes, volumes).quality)
}

/// The player's tank, bundled for the control systems (drive input, aiming, sight, shooting). It
/// folds together the three things those systems always reach for as a unit — *which* tank is
/// controlled, *where its parts are* ([`Rig`]), and *what it can still do* (capabilities) — so a
/// system takes one `ControlledTank` param instead of repeating a controlled-tank query, a
/// [`VolumeFacets`] query, and a requirement evaluation. Scoped to the single [`Controlled`]
/// tank; per-tank consumers (`track::sim::apply_track_forces`, the HUD) keep using [`VolumeFacets`]
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
    /// per-weapon / per-part gates (a weapon's `fire` or sight access) that aren't tank-wide capabilities.
    /// An empty requirement is vacuously met. `None` controlled → not met.
    pub fn meets(&self, requirement: &Requirement) -> bool {
        let Ok((_, _, Some(tank_volumes))) = self.tank.single() else {
            return false;
        };
        evaluate(
            requirement,
            &part_facts(tank_volumes, &self.volumes).quality,
        ) > 0.0
    }
}

fn process_cookoffs(
    ammo: Query<(Entity, &ComponentHealth, &VolumeOf), (With<Ammo>, Without<CookedOff>)>,
    mut crew: Query<(&VolumeOf, &mut ComponentHealth), (With<CrewStation>, Without<Ammo>)>,
    mut commands: Commands,
) {
    for (ammo_entity, hp, owner) in &ammo {
        if !hp.is_empty() {
            continue;
        }
        commands.entity(ammo_entity).insert(CookedOff);
        for (crew_owner, mut crew_hp) in &mut crew {
            if crew_owner.tank() == owner.tank() {
                crew_hp.drain();
            }
        }
    }
}

#[derive(Component)]
pub struct LaunchedTurret;

fn launch_turrets_on_cookoff(
    // Launching the turret spawns a free PHYSICS body, and unlike a purely logical consequence (crew
    // death, capability loss) that the client derives from the replicated snapshot, a client-local
    // launch starts from an unsynced avian Position (pops to origin) and re-fires on every reconnect
    // via `Added<CookedOff>` (both observed). The net client instead shows the authoritative launched
    // turret via `net::protocol::LaunchedTurretPose`. This whole chain is already gated authority-only
    // at the plugin (`Without` `ClientReplica`), so no per-system replica guard is needed here.
    cooked_ammo: Query<&VolumeOf, (With<CookedOff>, Added<CookedOff>)>,
    turrets: Query<
        (Entity, &GlobalTransform, &TurretBody),
        (With<Turret>, Without<LaunchedTurret>),
    >,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for ammo_owner in &cooked_ammo {
        let tank = ammo_owner.tank();
        for (turret, global, body) in &turrets {
            if !is_descendant_of(turret, tank, &parents) {
                continue;
            }
            // Detached, the turret is the root avian composes its armour colliders' scale from
            // (`ColliderTransform`), and the collector refuses armour at any scale but a bit-exact
            // 1 (`ballistics::collect`). So it carries unit scale, never one read back out of the
            // world matrix: decomposing a rotated `GlobalTransform` rounds each column's length and
            // can perturb a unit scale. Unit scale is what the hierarchy composes, bit for bit:
            // the turret is the gunner chain's Yaw servo (`tank::spawn::resolve_rig_nodes`);
            // `bake::certify_asset` refuses an asset unless every servo node and each of its
            // ancestors is authored at scale exactly 1 (`L2.UNIT_SCALE` in
            // `bake::declared_roles_resolve`); the sim nodes are spawned with those authored
            // transforms (`tank::spawn::spawn_node_entities`) under a root asserted at unit scale
            // (`tank::spawn::assemble_tank_body`); and the servos write rotation only
            // (`tank::servo::drive_servos`, `tank::servo::restore_servo_truth`).
            let mut pose = global.compute_transform();
            pose.scale = Vec3::ONE;
            let up = Vec3::from(global.up());
            let right = Vec3::from(global.right());
            let forward = Vec3::from(global.forward());
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
                    crate::eased_dynamic_body(),
                    Mass(body.mass),
                    AngularInertia::from_shape(
                        &Cuboid::new(
                            body.inertia_extents.x,
                            body.inertia_extents.y,
                            body.inertia_extents.z,
                        ),
                        body.mass,
                    ),
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
        if hp.is_empty() {
            commands.entity(entity).insert(Dead);
        }
    }
}

/// Latch each tank's knockout label by the one rule ([`knockout_from_counts`]), its cause a
/// cook-off when one of its ammunition volumes has cooked off. This is a *label*, not a gameplay
/// gate — no system reads `TankKnockedOut` to disable capabilities. It runs after
/// [`process_cookoffs`] and [`kill_crew`], so a cook-off's emptied crew pools are out of action,
/// and before [`launch_turrets_on_cookoff`], so every tank whose turret launches is already
/// labelled: the shot recorder opens encounters only for unlabelled tanks and reads their servo
/// nodes as mounted (`damage_cam::record::servo_angles`).
fn mark_dead_tanks(
    tanks: Query<(Entity, &TankVolumes), Without<TankKnockedOut>>,
    volumes: Query<(Option<&CrewStation>, Option<&Dead>, Has<CookedOff>)>,
    mut commands: Commands,
) {
    for (tank, tank_volumes) in &tanks {
        let mut in_action = 0;
        let mut cooked = false;
        for volume in tank_volumes.iter() {
            let (station, dead, cooked_off) = volumes.get(volume).expect(LIVE_VOLUME);
            in_action += u32::from(station.is_some() && dead.is_none());
            cooked |= cooked_off;
        }
        if let Some(reason) = knockout_from_counts(in_action, cooked) {
            commands.entity(tank).insert(TankKnockedOut { reason });
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

    /// The game's damage model: rate consumers rely on a module's held share, including
    /// zero at knockout. A binary-quality mutant fails the half-full row.
    #[test]
    fn module_quality_is_its_held_share() {
        use super::*;
        use bevy::ecs::system::RunSystemOnce;
        // Fixture: a ten-point engine pool at full, half and empty.
        for share in [1.0, 0.5, 0.0] {
            let mut world = World::new();
            let tank = world.spawn_empty().id();
            let mut hp = ComponentHealth::full(10);
            hp.set_held((hp.capacity() as f32 * share) as u32);
            world.spawn((FunctionRole::Engine, hp, VolumeOf(tank)));
            world
                .run_system_once(
                    move |tanks: Query<&TankVolumes>, facets: Query<VolumeFacets>| {
                        let quality = part_facts(tanks.get(tank).unwrap(), &facets).quality;
                        assert_eq!(
                            evaluate(&vec![Group::Single(Part::Engine)], &quality),
                            share
                        );
                    },
                )
                .unwrap();
        }
    }

    /// The game's damage model's one knockout rule (#227), run through the consequence chain's
    /// systems in their order: the knockout label (read by scoring and the Battle) latches when
    /// fewer than two crew members are in action, whatever emptied their pools. A cook-off is a
    /// cause the rule decides, since it empties every crew pool of its tank; a tank with every
    /// module out of action stays in while two crew are in action. A mutant that restores a
    /// cook-off label of its own, counts modules, or knocks out at zero crew fails a row.
    #[test]
    fn the_crew_rule_decides_every_knockout() {
        use super::{
            Ammo, CookedOff, CrewStation, KnockoutReason, TankKnockedOut, VolumeOf, kill_crew,
            mark_dead_tanks, process_cookoffs,
        };
        use crate::ballistics::ComponentHealth;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::prelude::*;

        let pool = |emptied: bool| {
            let mut pool = ComponentHealth::full(10);
            if emptied {
                pool.drain();
            }
            pool
        };
        // (crew pools emptied, ammunition pool emptied, module pools emptied) → expected label.
        let rows: [(&[bool], bool, bool, Option<KnockoutReason>); 5] = [
            (&[false, false], true, false, Some(KnockoutReason::Cookoff)),
            (&[false, false], false, true, None),
            (&[false, true], false, false, Some(KnockoutReason::CrewLoss)),
            (&[false, false, true], false, true, None),
            (&[true, true], false, false, Some(KnockoutReason::CrewLoss)),
        ];
        let mut world = World::new();
        let tanks: Vec<Entity> = rows
            .iter()
            .map(|&(crew, ammo, modules, _)| {
                let tank = world.spawn_empty().id();
                for &emptied in crew {
                    world.spawn((CrewStation::Gunner, pool(emptied), VolumeOf(tank)));
                }
                world.spawn((Ammo, pool(ammo), VolumeOf(tank)));
                world.spawn((pool(modules), VolumeOf(tank)));
                world.spawn((pool(modules), VolumeOf(tank)));
                tank
            })
            .collect();
        world.run_system_once(process_cookoffs).unwrap();
        world.run_system_once(kill_crew).unwrap();
        world.run_system_once(mark_dead_tanks).unwrap();
        for (row, (tank, &(.., expected))) in tanks.iter().zip(&rows).enumerate() {
            let label = world.get::<TankKnockedOut>(*tank).map(|label| label.reason);
            assert_eq!(label, expected, "row {row}");
        }
        assert_eq!(
            world.query::<&CookedOff>().iter(&world).count(),
            1,
            "only the emptied ammunition pool cooks off"
        );
        // The game's damage model latches the cause read by scoring. A later cook-off empties
        // the remaining pools but cannot replace an already decided crew-loss label.
        let already_out = world
            .spawn(TankKnockedOut {
                reason: KnockoutReason::CrewLoss,
            })
            .id();
        world.spawn((CrewStation::Gunner, pool(false), VolumeOf(already_out)));
        world.spawn((CrewStation::Driver, pool(false), VolumeOf(already_out)));
        world.spawn((Ammo, pool(true), VolumeOf(already_out)));
        world.run_system_once(process_cookoffs).unwrap();
        world.run_system_once(kill_crew).unwrap();
        world.run_system_once(mark_dead_tanks).unwrap();
        assert_eq!(
            world.get::<TankKnockedOut>(already_out).unwrap().reason,
            KnockoutReason::CrewLoss,
            "a later cook-off cannot rewrite a latched cause"
        );
    }

    /// The game's damage model: controls require complete staffing alternatives, independent
    /// of task contributors, coefficients and module condition. Drive relies on this distinction.
    #[test]
    fn control_authority_is_a_staffing_fact() {
        use super::{ControlStaffing, CrewStation};
        // Fixture: two required stations or one alternative operator.
        let staffing = ControlStaffing::Any(vec![
            vec![CrewStation::Driver, CrewStation::Commander],
            vec![CrewStation::Gunner],
        ]);
        assert!(!staffing.permits(&[CrewStation::Driver].into()));
        assert!(staffing.permits(&[CrewStation::Driver, CrewStation::Commander].into()));
        assert!(staffing.permits(&[CrewStation::Gunner].into()));
        assert!(!staffing.permits(&[].into()));
        assert!(ControlStaffing::Unstaffed.permits(&[].into()));
    }

    /// The game's damage model: equivalent Backup alternatives retain a canonical complete
    /// path while control authority remains the separately authored staffing fact.
    #[test]
    fn backup_zero_score_ties_do_not_decide_authority() {
        use super::{ControlStaffing, CrewStation, resolve};
        // Fixture: both complete paths unavailable, with one staffed operator.
        let mut members = vec![
            (1.0, vec![Part::Driver, Part::Engine]),
            (1.0, vec![Part::Commander, Part::Transmission]),
        ];
        let quality = q(&[(Part::Driver, 1.0), (Part::Transmission, 1.0)]);
        let controls = ControlStaffing::Any(vec![vec![CrewStation::Driver]]);
        for _ in 0..2 {
            let req = vec![Group::Graded(GradedGroup::Backup(members.clone()))];
            let result = resolve(&req, &quality);
            assert_eq!(result.rate, 0.0);
            let selected = &result.groups[0].contributors[0];
            assert_eq!(selected.parts, &[Part::Driver, Part::Engine]);
            assert_eq!(selected.parts, members[selected.index].1);
            assert!(controls.permits(&[CrewStation::Driver].into()));
            members.reverse();
        }
    }

    /// The game's damage model: cooperative contributors cannot borrow one another's
    /// absent parts; task consumers rely on complete paths even when every contribution is zero.
    #[test]
    fn pool_does_not_splice_incomplete_members() {
        // Fixture: complementary unavailable dependencies on two complete paths.
        let req = vec![Group::Graded(GradedGroup::Pool(vec![
            (1.0, vec![Part::Driver, Part::Engine]),
            (1.0, vec![Part::Commander, Part::Transmission]),
        ]))];
        let quality = q(&[(Part::Commander, 1.0), (Part::Engine, 1.0)]);
        let result = super::resolve(&req, &quality);
        assert_eq!(result.rate, 0.0);
        assert!(
            result.groups[0]
                .contributors
                .iter()
                .all(|member| member.rate == 0.0)
        );
    }

    #[test]
    fn single_groups_are_and_all_or_nothing() {
        // Fixture: three mandatory contributors.
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

    /// The launched turret's rigid body is whatever its own `TurretBody` says: mass and the box
    /// its inertia is taken over. A launch that carried a constant would hand the second tank the
    /// first one's turret.
    #[test]
    fn a_launched_turret_takes_its_mass_and_inertia_from_its_body_data() {
        use super::*;
        let mut app = App::new();
        app.add_systems(Update, launch_turrets_on_cookoff);
        let tank = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((VolumeOf(tank), CookedOff));
        let extents = Vec3::new(2.0, 1.5, 3.0);
        let turret = app
            .world_mut()
            .spawn((
                Turret,
                TurretBody {
                    mass: 1234.0,
                    inertia_extents: extents,
                },
                ChildOf(tank),
                GlobalTransform::default(),
            ))
            .id();
        app.update();
        let world = app.world();
        assert!(world.get::<LaunchedTurret>(turret).is_some());
        assert_eq!(world.get::<Mass>(turret).unwrap().0, 1234.0);
        let expected = AngularInertia::from_shape(&Cuboid::new(2.0, 1.5, 3.0), 1234.0);
        assert_eq!(
            world.get::<AngularInertia>(turret).unwrap().tensor(),
            expected.tensor()
        );
    }

    /// A LAUNCHED TURRET COMES TO REST ON A SLOPE: its collision shapes carry their substance's
    /// friction from spawn (`tank::spawn::spawn_proxy_parts`), so off its tracks it is a body of
    /// that substance on the terrain's ground, and the launch changes nothing about its grip. Two
    /// identical turrets launch from tanks standing on a wide static floor inclined at 20° and
    /// carrying the terrain's friction; one's shape carries the steel row the tank file names, the
    /// other's is frictionless. After the flight and the
    /// landing the first has stopped and the second still slides, so the test fails if a proxy
    /// carries no friction, and the control keeps it from passing vacuously. The pair is the
    /// geometric mean of both rows, which must exceed tan 20° ≈ 0.364 (DERIVED) for the rest to
    /// be possible at all.
    ///
    /// FIXTURE DATA: a 1000 kg turret, the RHA row, a 20° slope, 30 s of 16 ms frames.
    #[test]
    fn a_launched_turret_comes_to_rest_on_a_slope() {
        use super::*;
        use avian3d::prelude::{Collider, CollisionLayers, LayerMask, PhysicsPlugins, RigidBody};
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .add_systems(Update, launch_turrets_on_cookoff);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();

        let slope = Quat::from_rotation_x(20f32.to_radians());
        let ground = crate::ground_types::GroundTypes::load_shipped().terrain_friction();
        let steel = crate::substances::SubstanceRegistry::shipped_static()
            .contact_friction("RHA")
            .unwrap();
        assert!(steel.combine(ground).dynamic_coefficient > 20f32.to_radians().tan());
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(2000.0, 1.0, 2000.0),
            Transform::from_rotation(slope).with_translation(slope * Vec3::new(0.0, -0.5, 0.0)),
            ground,
        ));
        let mut turrets = Vec::new();
        let frictionless = crate::substances::contact_friction(0.0);
        for (i, friction) in [steel, frictionless].into_iter().enumerate() {
            let tank = app
                .world_mut()
                .spawn((
                    Transform::from_rotation(slope)
                        .with_translation(slope * Vec3::new(100.0 * i as f32, 1.0, 0.0)),
                    RigidBody::Static,
                ))
                .id();
            let turret = app
                .world_mut()
                .spawn((
                    Turret,
                    TurretBody {
                        mass: 1000.0,
                        inertia_extents: Vec3::new(2.0, 1.0, 2.0),
                    },
                    Transform::from_xyz(0.0, 0.5, 0.0),
                    ChildOf(tank),
                ))
                .id();
            app.world_mut().spawn((
                ChildOf(turret),
                Collider::cuboid(2.0, 1.0, 2.0),
                CollisionLayers::new([crate::Layer::Vehicle], LayerMask::ALL),
                friction,
            ));
            turrets.push((tank, turret));
        }
        for _ in 0..4 {
            app.update();
        }
        for &(tank, ..) in &turrets {
            app.world_mut().spawn((VolumeOf(tank), CookedOff));
        }
        for _ in 0..1900 {
            app.update();
        }
        let speed = |app: &App, turret: Entity| {
            app.world()
                .get::<LinearVelocity>(turret)
                .unwrap()
                .0
                .length()
        };
        let launched = speed(&app, turrets[0].1);
        let control = speed(&app, turrets[1].1);
        println!("launched turret on 20°: steel {launched} m/s, frictionless {control} m/s");
        assert!(control > 0.5, "the control must still slide, got {control}");
        assert!(
            launched < 0.05,
            "the launched turret must rest, got {launched}"
        );
    }

    /// A launched turret's armour reaches the world at a bit-exact unit scale, whatever pose the
    /// tank and turret were in. The collector refuses armour at any other scale
    /// (`ballistics::collect`), and avian composes each collider's scale from its root's
    /// `Transform` (`ColliderTransform`). Decomposing a rotated world matrix can perturb a unit
    /// scale, so a turret whose scale is read back out of it fails closed every round that meets
    /// it.
    ///
    /// FIXTURE DATA: sixteen poses, at least one of whose world matrix decomposes off 1 (asserted,
    /// so the test cannot pass vacuously).
    #[test]
    fn a_launched_turret_keeps_its_armour_at_unit_scale() {
        use super::*;
        use avian3d::prelude::{Collider, PhysicsPlugins, RigidBody};
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .add_systems(Update, launch_turrets_on_cookoff);
        // Avian registers its diagnostics resources in `Plugin::finish`, which a bare `update()`
        // loop skips.
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();

        let mut tanks = Vec::new();
        let mut armour = Vec::new();
        let mut decomposes_off_one = false;
        for i in 0..16 {
            let i = i as f32;
            let hull = Transform::from_xyz(20.0 * i, 0.0, 0.0).with_rotation(Quat::from_euler(
                EulerRot::YXZ,
                0.37 * i,
                0.05 * i,
                -0.03 * i,
            ));
            let ring =
                Transform::from_xyz(0.0, 1.5, 0.3).with_rotation(Quat::from_rotation_y(0.2 * i));
            decomposes_off_one |= (GlobalTransform::from(hull) * ring)
                .compute_transform()
                .scale
                .to_array()
                .map(f32::to_bits)
                != [1.0f32.to_bits(); 3];
            let tank = app.world_mut().spawn((hull, RigidBody::Static)).id();
            let turret = app
                .world_mut()
                .spawn((
                    Turret,
                    TurretBody {
                        mass: 1000.0,
                        inertia_extents: Vec3::ONE,
                    },
                    ring,
                    ChildOf(tank),
                ))
                .id();
            armour.push(
                app.world_mut()
                    .spawn((
                        Collider::cuboid(1.0, 0.5, 1.0),
                        Transform::IDENTITY,
                        ChildOf(turret),
                    ))
                    .id(),
            );
            tanks.push(tank);
        }
        assert!(
            decomposes_off_one,
            "no fixture pose decomposes off a unit scale, so nothing here is tested"
        );
        // The world matrices are propagated before anything cooks off.
        for _ in 0..4 {
            app.update();
        }
        for &tank in &tanks {
            app.world_mut().spawn((VolumeOf(tank), CookedOff));
        }
        for _ in 0..4 {
            app.update();
        }
        for collider in armour {
            let parent = app.world().get::<ChildOf>(collider).unwrap().parent();
            assert!(app.world().get::<LaunchedTurret>(parent).is_some());
            let scale = app.world().get::<Collider>(collider).unwrap().scale();
            assert_eq!(
                scale.to_array().map(f32::to_bits),
                [1.0f32.to_bits(); 3],
                "a launched turret's armour reached the world at scale {scale:?}"
            );
        }
    }
}
