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
    AngularInertia, AngularVelocity, CenterOfMass, LinearVelocity, Mass, Position, Rotation,
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
use crate::tank::{
    Controlled, Rig, ServoCommand, ServoIndex, ServoSpec, Tank, Turret, TurretBody, explicit_mass,
};

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
/// for diagnostic readouts and backfill ("who is where"). Task requirements name the station
/// through [`Part`]; [`ControlStaffing`] names it directly. `Ord` is by
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
        crew_data().foreign_competence.0
    }
}

/// The game's damage model: crew work facts shared by every tank (`assets/damage/crew.ron`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrewData {
    /// Work-rate share of a crew member at a station not their own.
    foreign_competence: crate::provenance::Sourced<f32>,
    /// Seconds a backfill swap takes.
    swap_secs: crate::provenance::Sourced<f32>,
}

fn crew_data() -> &'static CrewData {
    static DATA: std::sync::LazyLock<CrewData> = std::sync::LazyLock::new(|| {
        let data: CrewData = crate::provenance::from_shipped_str(
            "assets/damage/crew.ron",
            include_str!("../assets/damage/crew.ron"),
        )
        .unwrap_or_else(|err| panic!("{err}"));
        assert!(
            data.foreign_competence.0 > 0.0 && data.foreign_competence.0 <= 1.0,
            "assets/damage/crew.ron: foreign_competence must be finite and in (0, 1]; author a positive work-rate share"
        );
        assert!(
            data.swap_secs.0.is_finite() && data.swap_secs.0 > 0.0,
            "assets/damage/crew.ron: swap_secs must be finite and positive; author the seconds a swap takes"
        );
        data
    });
    &DATA
}

/// The game's damage model: the increment a cook-off's release adds to the detached turret's
/// carried hull motion, in the assembly's own frame at detachment (`assets/damage/cookoff.ron`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CookoffLaunch {
    /// Along its up and forward axes (m/s).
    launch_velocity: crate::provenance::Sourced<(f32, f32)>,
    /// About its right and up axes (rad/s).
    launch_spin: crate::provenance::Sourced<(f32, f32)>,
}

fn cookoff_launch() -> &'static CookoffLaunch {
    static DATA: std::sync::LazyLock<CookoffLaunch> = std::sync::LazyLock::new(|| {
        let data: CookoffLaunch = crate::provenance::from_shipped_str(
            "assets/damage/cookoff.ron",
            include_str!("../assets/damage/cookoff.ron"),
        )
        .unwrap_or_else(|err| panic!("{err}"));
        let ((lift, throw), (pitch, yaw)) = (data.launch_velocity.0, data.launch_spin.0);
        assert!(
            [lift, throw, pitch, yaw]
                .iter()
                .all(|value| value.is_finite()),
            "assets/damage/cookoff.ron: launch_velocity and launch_spin must be finite; author a representable launch"
        );
        data
    });
    &DATA
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

/// The game's damage model: what a module's HP pool lets it supply its consumers, declared on the
/// module's own volume (`supply` beside its `function` in the tank sheet) and read by
/// [`module_quality`]. `Held` supplies its held share; `Whole` supplies all of it until the module
/// is knocked out.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum ModuleSupply {
    Held,
    Whole,
}

/// A gameplay verb the tank can perform (design §7b). Closed enum — consuming systems (driving,
/// shooting, cameras) reference variants directly. Adding one is a variant, its entry in
/// [`Self::ALL`] and one consuming system. The *requirements* for each capability are per-tank RON data
/// ([`TankCapabilities`]); each consumer reads its requirement through [`evaluate`].
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

/// The game's damage model: task requirements and control staffing from the validated sheet,
/// built with the tank's root components. Read by `track::sim::drive_target`.
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
    NotRequired,
    Any(Vec<Vec<CrewStation>>),
}

impl ControlStaffing {
    pub fn permits(&self, staffed: &std::collections::HashSet<CrewStation>) -> bool {
        match self {
            Self::NotRequired => true,
            Self::Any(paths) => paths
                .iter()
                .any(|stations| stations.iter().all(|station| staffed.contains(station))),
        }
    }
}

impl TankCapabilities {
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
    // Shipped damage data is refused at start, not at its first swap or cook-off.
    crew_data();
    cookoff_launch();
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
    enum Effect {
        Cancel,
        Start(PendingSwap),
        Refused(SwapRefusal),
    }
    let mut effects = bevy::utils::Parallel::<Vec<(Entity, Effect)>>::default();
    tanks.par_iter().for_each(|(tank, command, pending)| {
        // No edge means the player made no swap request this tick.
        let Some(swap) = command.crew_swap else {
            return;
        };
        let effect = match swap {
            CrewSwap::Cancel => pending.map(|_| Effect::Cancel),
            // A start during an existing swap is an allowed no-op; cancellation is explicit.
            CrewSwap::Start(..) if pending.is_some() => None,
            CrewSwap::Start(a, b) => {
                let seat_of = |station: CrewStation| {
                    seats
                        .iter()
                        .find(|(_, s, owner)| **s == station && owner.tank() == tank)
                        .map(|(entity, ..)| entity)
                };
                Some(match swap_seats(a, b, seat_of) {
                    Ok((a, b)) => Effect::Start(PendingSwap {
                        a,
                        b,
                        remaining: crew_data().swap_secs.0,
                    }),
                    Err(refusal) => Effect::Refused(refusal),
                })
            }
        };
        if let Some(effect) = effect {
            effects.scope(|effects| effects.push((tank, effect)));
        }
    });
    let mut effects: Vec<_> = effects.drain().collect();
    effects.sort_unstable_by_key(|(tank, _)| *tank);
    // Structural writes and refusal logs commit in tank-entity order after independent evaluation.
    for (tank, effect) in effects {
        match effect {
            Effect::Cancel => {
                commands.entity(tank).remove::<PendingSwap>();
            }
            Effect::Start(swap) => {
                commands.entity(tank).insert(swap);
            }
            Effect::Refused(refusal) => warn!("refused a crew swap on tank {tank}: {refusal}"),
        }
    }
}

/// The game's damage model: why a crew swap a player's command started does not begin.
#[derive(Debug, PartialEq)]
enum SwapRefusal {
    OneStation(CrewStation),
    NoSeat(CrewStation),
}

impl std::fmt::Display for SwapRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OneStation(station) => write!(f, "both ends name the {station:?} station"),
            Self::NoSeat(station) => write!(f, "the tank has no {station:?} seat"),
        }
    }
}

/// The two seats a swap between stations `a` and `b` exchanges, through the tank's `seat_of`.
fn swap_seats(
    a: CrewStation,
    b: CrewStation,
    seat_of: impl Fn(CrewStation) -> Option<Entity>,
) -> Result<(Entity, Entity), SwapRefusal> {
    if a == b {
        return Err(SwapRefusal::OneStation(a));
    }
    let seat = |station| seat_of(station).ok_or(SwapRefusal::NoSeat(station));
    Ok((seat(a)?, seat(b)?))
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
/// has `function` + `supply` + `health`. Use it as `Query<VolumeFacets>`; `.get(e)` yields a struct with these
/// named fields.
#[derive(QueryData)]
pub struct VolumeFacets {
    pub crew: Option<&'static CrewStation>,
    pub crewman: Option<&'static Crewman>,
    pub dead: Option<&'static Dead>,
    pub function: Option<&'static FunctionRole>,
    pub supply: Option<&'static ModuleSupply>,
    pub health: Option<&'static ComponentHealth>,
    pub repair: Option<&'static repair::ModuleRepair>,
}

/// The game's damage model: independently observed source qualities and station staffing.
pub struct PartFacts {
    pub quality: std::collections::HashMap<Part, f32>,
    pub staffed: std::collections::HashSet<CrewStation>,
}

/// The game's damage model: collect each tank's facts once. A station in action supplies its
/// occupant's competence; a module supplies its [`module_quality`]. Absent sources supply zero.
/// Duplicate functions retain their best provider; staffing records station presence directly.
pub fn part_facts(tank_volumes: &TankVolumes, volumes: &Query<VolumeFacets>) -> PartFacts {
    let mut quality: std::collections::HashMap<Part, f32> = std::collections::HashMap::new();
    let mut staffed = std::collections::HashSet::new();
    for volume in tank_volumes.iter() {
        let facets = volumes.get(volume).expect(LIVE_VOLUME);
        if let (Some(role), None) = (facets.crew, facets.dead) {
            staffed.insert(*role);
            let home = facets
                .crewman
                .expect("a crew station is spawned with its occupant (tank::spawn)")
                .home;
            let q = quality.entry(Part::from(*role)).or_insert(0.0);
            *q = q.max(competence(home, *role));
        }
        if let Some(func) = facets.function {
            let hp = facets
                .health
                .expect("a function volume is spawned with its HP pool (tank::spawn)");
            let supply = facets
                .supply
                .expect("a function volume is spawned with its declared supply (tank::spawn)");
            let q = quality.entry(Part::from(*func)).or_insert(0.0);
            *q = q.max(module_quality(*supply, hp));
        }
    }
    PartFacts { quality, staffed }
}

/// The game's damage model: what a module supplies its consumers under its declared
/// [`ModuleSupply`].
pub fn module_quality(supply: ModuleSupply, health: &ComponentHealth) -> f32 {
    match supply {
        ModuleSupply::Held => module_share(health),
        ModuleSupply::Whole if health.is_empty() => 0.0,
        ModuleSupply::Whole => 1.0,
    }
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

/// The game's damage model: complete member contributions combine by capped sum (Pool),
/// best alternative (Backup), then minimum across required groups. An empty requirement is
/// unconditional; an absent part is an unavailable source.
pub fn evaluate(requirement: &Requirement, quality: &std::collections::HashMap<Part, f32>) -> f32 {
    requirement
        .iter()
        .map(|group| match group {
            Group::Single(part) => live_quality(quality, *part),
            Group::Graded(GradedGroup::Pool(members)) => members
                .iter()
                .map(|member| member_quality(member, |part| live_quality(quality, part)))
                .sum::<f32>()
                .min(1.0),
            Group::Graded(GradedGroup::Backup(members)) => members
                .iter()
                .map(|member| member_quality(member, |part| live_quality(quality, part)))
                .reduce(f32::max)
                .expect(
                    "TankSpec::validate refuses empty Backup member lists in every requirement",
                ),
        })
        .fold(1.0, f32::min)
}

pub(crate) fn live_quality(quality: &std::collections::HashMap<Part, f32>, part: Part) -> f32 {
    // Absent parts are a legitimate unavailable source, including an out-of-action station.
    quality.get(&part).copied().unwrap_or(0.0)
}

fn member_quality(member: &Member, quality: impl Fn(Part) -> f32) -> f32 {
    member.1.iter().fold(member.0, |q, p| q * quality(*p))
}

/// Whether an arbitrary tank supplies a positive task rate — the per-tank analog of
/// [`ControlledTank::meets`], for sim systems that iterate every tank (a weapon's `fire`
/// gate in `shooting`; loading consumes a rate). No volumes yet (spec still binding) → not met.
pub fn requirement_met(
    tank_volumes: Option<&TankVolumes>,
    requirement: &Requirement,
    volumes: &Query<VolumeFacets>,
) -> bool {
    requirement_rate(tank_volumes, requirement, volumes) > 0.0
}

/// The game's damage model: the rate served by a weapon or other local requirement.
/// Before a tank's volumes exist it supplies no work.
pub fn requirement_rate(
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

    /// Whether the controlled tank supplies a positive task rate — for the
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

/// The game's damage model: the assembly has left this root, even if its body later despawns.
#[derive(Component)]
struct TurretDetached;

/// The game's damage model: each tank computes its split independently; the serial flush commits
/// mass, motion and hierarchy together. ChildOf must go before the RigidBody add hook runs.
/// The authority simulates both bodies; replicas receive poses, never mass properties.
/// The remaining hull depends on turret orientation: the full-vehicle box does not turn with it.
fn launch_turrets_on_cookoff(
    cooked_ammo: Query<&VolumeOf, (With<CookedOff>, Added<CookedOff>)>,
    turrets: Query<(&GlobalTransform, &TurretBody), With<Turret>>,
    hulls: Query<(
        Entity,
        &Rig,
        Option<&TurretDetached>,
        &GlobalTransform,
        &Position,
        &Rotation,
        &Mass,
        &CenterOfMass,
        &AngularInertia,
        &LinearVelocity,
        &AngularVelocity,
    )>,
    mut commands: Commands,
) {
    // Deduplicate simultaneous pools to avoid evaluating the same owner's split twice.
    let owners: std::collections::BTreeSet<_> = cooked_ammo.iter().map(VolumeOf::tank).collect();
    let launch = cookoff_launch();
    let ((lift, throw), (pitch, yaw)) = (launch.launch_velocity.0, launch.launch_spin.0);
    let mut effects = bevy::utils::Parallel::<Vec<_>>::default();
    hulls.par_iter_many(&owners).for_each(|hull| {
        let (
            tank,
            rig,
            detached,
            hull_pose,
            position,
            rotation,
            mass,
            centre,
            inertia,
            velocity,
            spin,
        ) = hull.expect(
            "a tank root carries its spawn-built rig, explicit mass properties \
                         and motion",
        );
        if detached.is_some() {
            // A later pool can cook off after the assembly has left.
            return;
        }
        let (global, body) = turrets.get(rig.turret).expect(
            "a mounted assembly has Turret, TurretBody and GlobalTransform: TankTemplate \
             spawns them together and detachment records TurretDetached on the root",
        );
        let frame = Transform::from_translation(position.0).with_rotation(rotation.0);
        // Both globals share a propagation moment. Rebase their relative pose onto Writeback's
        // current physics pose. The Assembly gate certifies unit scale; decomposition rounds it.
        let mut relative = global.reparented_to(hull_pose);
        relative.scale = Vec3::ONE;
        let pose = frame * relative;
        let turret_centre = relative.transform_point(body.centre_of_mass);
        let remainder = mass.0 - body.mass;
        assert!(
            remainder.is_finite() && remainder > 0.0,
            "TankSpec::validate admits mass > turret.mass; spawn must preserve that relation"
        );
        let new_centre = (mass.0 * centre.0 - body.mass * turret_centre) / remainder;
        // Parallel-axis subtraction about the old centre, then translation to the new one:
        // I_full = I_hull + m_hull PA(c_hull-c_full) + I_turret + m_turret PA(c_turret-c_full).
        let tensor = inertia.tensor().to_mat3()
            - body
                .angular_inertia
                .tensor()
                .rotated(relative.rotation)
                .shifted(body.mass, turret_centre - centre.0)
                .to_mat3()
            - AngularInertia::ZERO
                .tensor()
                .shifted(remainder, new_centre - centre.0)
                .to_mat3();
        // Sylvester's criterion catches a broken admitted model before diagonalization clamps it.
        assert!(
            tensor.is_finite()
                && tensor.x_axis.x > 0.0
                && tensor.x_axis.x * tensor.y_axis.y - tensor.x_axis.y * tensor.y_axis.x > 0.0
                && tensor.determinant() > 0.0,
            "TankTemplate admission guarantees positive remaining inertia at every servo angle; \
             spawn and motion must preserve its model"
        );
        let carried = velocity.0 + spin.0.cross(frame.rotation * (turret_centre - centre.0));
        effects.scope(|effects| {
            effects.push((
                tank,
                explicit_mass(
                    remainder,
                    new_centre,
                    AngularInertia::from_mat3_unchecked(tensor),
                ),
                // LinearVelocity is the mass-centre velocity, not the body origin's velocity.
                LinearVelocity(velocity.0 + spin.0.cross(frame.rotation * (new_centre - centre.0))),
                rig.turret,
                pose,
                explicit_mass(body.mass, body.centre_of_mass, body.angular_inertia),
                LinearVelocity(carried + *pose.up() * lift + *pose.forward() * throw),
                AngularVelocity(spin.0 + *pose.right() * pitch + *pose.up() * yaw),
            ))
        });
    });
    let mut effects: Vec<_> = effects.drain().collect();
    effects.sort_unstable_by_key(|(tank, ..)| *tank);
    for (tank, mass, velocity, turret, pose, turret_mass, carried, spin) in effects {
        commands
            .entity(tank)
            .insert((mass, velocity, TurretDetached));
        commands
            .entity(turret)
            .remove::<(ChildOf, Turret, ServoCommand, ServoIndex, ServoSpec)>()
            .insert((
                // Explicit physics pose prevents the add hook composing the former parent twice.
                Position(pose.translation),
                Rotation(pose.rotation),
                pose,
                crate::eased_dynamic_body(),
                turret_mass,
                carried,
                spin,
                LaunchedTurret,
            ));
    }
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
    use super::*;
    use avian3d::prelude::{ComputedAngularInertia, ComputedCenterOfMass, ComputedMass};
    use std::collections::HashMap;

    /// Build a part→quality map from `(part, quality)` pairs.
    fn q(pairs: &[(Part, f32)]) -> HashMap<Part, f32> {
        pairs.iter().copied().collect()
    }

    /// The game's damage model: rate consumers rely on a module supplying what its declared
    /// [`super::ModuleSupply`] says, whatever its role: a `Held` module its held share, including
    /// zero at knockout, a `Whole` one all of it until its HP pool is emptied. A per-role branch
    /// fails the crossed cases; a binary `Held` fails the half-full one, a scaled `Whole` its own.
    #[test]
    fn module_quality_follows_its_declared_supply_whatever_its_role() {
        use bevy::ecs::system::RunSystemOnce;
        // Fixture: ten-point pools at full, half and empty, each supply on two roles.
        for (role, supply, share, expected) in [
            (FunctionRole::Transmission, ModuleSupply::Held, 1.0, 1.0),
            (FunctionRole::Transmission, ModuleSupply::Held, 0.5, 0.5),
            (FunctionRole::Transmission, ModuleSupply::Held, 0.0, 0.0),
            (FunctionRole::Engine, ModuleSupply::Held, 0.5, 0.5),
            (FunctionRole::Engine, ModuleSupply::Whole, 1.0, 1.0),
            (FunctionRole::Engine, ModuleSupply::Whole, 0.5, 1.0),
            (FunctionRole::Engine, ModuleSupply::Whole, 0.0, 0.0),
            (FunctionRole::Transmission, ModuleSupply::Whole, 0.5, 1.0),
        ] {
            let mut world = World::new();
            let tank = world.spawn_empty().id();
            let mut hp = ComponentHealth::full(10);
            hp.set_held((hp.capacity() as f32 * share) as u32);
            world.spawn((role, supply, hp, VolumeOf(tank)));
            world
                .run_system_once(
                    move |tanks: Query<&TankVolumes>, facets: Query<VolumeFacets>| {
                        let quality = part_facts(tanks.get(tank).unwrap(), &facets).quality;
                        assert_eq!(
                            evaluate(&vec![Group::Single(Part::from(role))], &quality),
                            expected,
                            "{role:?} {supply:?} at {share}"
                        );
                    },
                )
                .unwrap();
        }
    }

    /// The game's damage model: the owner's decision that an engine supplies whole power until it
    /// is knocked out (provisional, playtest fork F20) is the shipped sheet's data, its engine's
    /// declared supply; a sheet whose engine declares `Held` reverts the fork.
    #[test]
    fn the_shipped_engine_supplies_whole_until_knocked_out() {
        let engines: Vec<_> = crate::bake::selected_spec()
            .volumes
            .values()
            .filter_map(crate::spec::VolumeSpec::component)
            .filter_map(|component| component.function)
            .filter(|function| function.role == FunctionRole::Engine)
            .collect();
        assert!(!engines.is_empty(), "the shipped sheet has an engine");
        assert!(
            engines
                .iter()
                .all(|function| function.supply == ModuleSupply::Whole),
            "the shipped engine supplies whole: {engines:?}"
        );
    }

    /// The game's damage model: a station supplies competence while in action, independent
    /// of its remaining HP. Task and staffing consumers rely on the same availability boundary.
    #[test]
    fn station_quality_is_competence_until_knocked_out() {
        use bevy::ecs::system::RunSystemOnce;
        // Fixture: a foreign occupant at full, reduced and emptied ten-point pools.
        for held in [10, 5, 1, 0] {
            let mut world = World::new();
            let tank = world.spawn_empty().id();
            let mut health = ComponentHealth::full(10);
            health.set_held(held * crate::ballistics::POOL_UNITS_PER_HP);
            let station = world
                .spawn((
                    CrewStation::Driver,
                    Crewman {
                        home: CrewStation::Loader,
                    },
                    health,
                    VolumeOf(tank),
                ))
                .id();
            if held == 0 {
                world.entity_mut(station).insert(Dead);
            }
            world
                .run_system_once(
                    move |tanks: Query<&TankVolumes>, facets: Query<VolumeFacets>| {
                        let quality = part_facts(tanks.get(tank).unwrap(), &facets).quality;
                        let expected = if held == 0 {
                            0.0
                        } else {
                            competence(CrewStation::Loader, CrewStation::Driver)
                        };
                        assert_eq!(live_quality(&quality, Part::Driver), expected);
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
        assert!(ControlStaffing::NotRequired.permits(&[].into()));
    }

    /// The game's damage model: unavailable complete alternatives stay unavailable regardless of
    /// ordering. Task consumers cannot combine the available half of each path into one operator.
    #[test]
    fn unavailable_backup_members_stay_unavailable_under_permutation() {
        // Fixture: complementary unavailable dependencies on complete paths.
        let mut members = vec![
            (1.0, vec![Part::Driver, Part::Engine]),
            (1.0, vec![Part::Commander, Part::Transmission]),
        ];
        let quality = q(&[(Part::Driver, 1.0), (Part::Transmission, 1.0)]);
        for _ in 0..2 {
            let req = vec![Group::Graded(GradedGroup::Backup(members.clone()))];
            assert_eq!(evaluate(&req, &quality), 0.0);
            members.reverse();
        }
    }

    /// The game's damage model: each complete path supplies its coefficient times its parts;
    /// task consumers receive the best alternative or cooperative sum, bounded by other groups.
    #[test]
    fn graded_task_rates_combine_complete_member_work() {
        // Fixture coefficients and independent part qualities.
        let members = vec![
            (0.8, vec![Part::Driver, Part::Engine]),
            (0.5, vec![Part::Commander, Part::Transmission]),
        ];
        let quality = q(&[
            (Part::Driver, 0.6),
            (Part::Engine, 1.0),
            (Part::Commander, 1.0),
            (Part::Transmission, 0.5),
        ]);
        let first = members[0].0 * quality[&Part::Driver] * quality[&Part::Engine];
        let second = members[1].0 * quality[&Part::Commander] * quality[&Part::Transmission];
        let backup = Group::Graded(GradedGroup::Backup(members.clone()));
        let pool = Group::Graded(GradedGroup::Pool(members));
        assert_eq!(evaluate(&vec![backup.clone()], &quality), first.max(second));
        assert_eq!(
            evaluate(&vec![pool.clone()], &quality),
            (first + second).min(1.0)
        );
        assert_eq!(
            evaluate(&vec![backup, pool], &quality),
            first.max(second).min(first + second)
        );
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
        assert_eq!(evaluate(&req, &quality), 0.0);
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

    /// The game's damage model: a swap start from a player's command is refused by type when its
    /// ends name one station or a station the tank has no seat at, and otherwise names the two
    /// seats it exchanges. Fixture: a tank seating a Driver and a Gunner.
    #[test]
    fn a_swap_start_names_two_seats_or_its_refusal() {
        let mut world = World::new();
        let (driver, gunner) = (world.spawn_empty().id(), world.spawn_empty().id());
        let seat_of = |station| match station {
            CrewStation::Driver => Some(driver),
            CrewStation::Gunner => Some(gunner),
            _ => None,
        };
        use CrewStation::{Driver, Gunner, Loader};
        assert_eq!(swap_seats(Driver, Gunner, seat_of), Ok((driver, gunner)));
        assert_eq!(
            swap_seats(Gunner, Gunner, seat_of),
            Err(SwapRefusal::OneStation(Gunner))
        );
        assert_eq!(
            swap_seats(Driver, Loader, seat_of),
            Err(SwapRefusal::NoSeat(Loader))
        );
        assert_eq!(
            swap_seats(Loader, Driver, seat_of),
            Err(SwapRefusal::NoSeat(Loader))
        );
    }

    /// The game's damage model: commands resolve only the requesting tank's stations. Backfill
    /// relies on starts preserving an in-flight swap and cancellation removing only its owner.
    /// Sharing seats across owners, dropping a cancel, or replacing an in-flight swap fails here.
    #[test]
    fn swap_commands_keep_seats_and_pending_state_with_their_owner() {
        let mut app = App::new();
        app.add_systems(Update, apply_crew_swap_commands);
        let mut owners = Vec::new();
        // Fixture: two complete station pairs and one tank missing its requested second seat.
        for complete in [true, true, false] {
            let tank = app
                .world_mut()
                .spawn((
                    Tank,
                    TankCommand {
                        crew_swap: Some(CrewSwap::Start(CrewStation::Driver, CrewStation::Gunner)),
                        ..default()
                    },
                ))
                .id();
            let driver = app
                .world_mut()
                .spawn((CrewStation::Driver, VolumeOf(tank)))
                .id();
            let gunner = complete.then(|| {
                app.world_mut()
                    .spawn((CrewStation::Gunner, VolumeOf(tank)))
                    .id()
            });
            owners.push((tank, driver, gunner));
        }
        app.update();
        for &(tank, driver, gunner) in &owners {
            match gunner {
                Some(gunner) => {
                    let pending = app.world().get::<PendingSwap>(tank).unwrap();
                    assert_eq!((pending.a, pending.b), (driver, gunner));
                    assert_eq!(pending.remaining, crew_data().swap_secs.0);
                }
                None => assert!(app.world().get::<PendingSwap>(tank).is_none()),
            }
        }
        app.world_mut()
            .get_mut::<TankCommand>(owners[0].0)
            .unwrap()
            .crew_swap = Some(CrewSwap::Cancel);
        app.world_mut()
            .get_mut::<TankCommand>(owners[1].0)
            .unwrap()
            .crew_swap = Some(CrewSwap::Start(CrewStation::Gunner, CrewStation::Driver));
        app.world_mut()
            .get_mut::<TankCommand>(owners[2].0)
            .unwrap()
            .crew_swap = None;
        app.update();
        assert!(app.world().get::<PendingSwap>(owners[0].0).is_none());
        let (tank, driver, gunner) = owners[1];
        let pending = app.world().get::<PendingSwap>(tank).unwrap();
        assert_eq!((pending.a, Some(pending.b)), (driver, gunner));
        assert!(app.world().get::<PendingSwap>(owners[2].0).is_none());
    }

    fn physics_app() -> App {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            crate::offline_physics(),
        ))
        .init_asset::<Mesh>()
        // Fixture simulation clock; no wall-clock measurement.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )));
        crate::headless_test::finish_plugins(&mut app);
        app
    }

    /// The game's damage model: wreck physics must neither create mass nor lose carried momentum.
    /// The only addition is the authored launch's linear impulse and angular impulse about the
    /// fixed world origin. Keeping any full-vehicle mass property, omitting the new-centre velocity,
    /// or mixing propagation and physics poses breaks this.
    #[test]
    fn detachment_conserves_mass_and_momentum_plus_the_stated_launch_impulse() {
        use avian3d::prelude::{Collider, ColliderDensity, Physics, PhysicsTime, RigidBody};
        use bevy::ecs::system::RunSystemOnce;

        let mut app = physics_app();
        app.world_mut().resource_mut::<Time<Physics>>().pause();
        // Fixture: distinct mass ratios, non-central assembly, full 3D poses and motion.
        let mut cases = Vec::new();
        for (i, scale) in [0.7, 1.3, 2.1].into_iter().enumerate() {
            let mass = 20_000.0 * scale;
            let inertia = AngularInertia::from_shape(&Cuboid::new(5.0, 3.0, 7.0), mass);
            let turret_mass = 2300.0 * scale * scale;
            let old = Transform::from_xyz(i as f32 * 20.0, 2.0, -3.0)
                .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, 0.2, -0.1));
            let current = Transform::from_translation(old.translation + Vec3::new(0.4, -0.1, 0.3))
                .with_rotation(old.rotation * Quat::from_rotation_z(0.15));
            let ring = Transform::from_xyz(0.5, 1.3, -0.4).with_rotation(Quat::from_euler(
                EulerRot::YXZ,
                -0.7,
                0.1,
                0.2,
            ));
            let centre = Vec3::new(-0.1, 0.4, 0.2);
            let linear = Vec3::new(4.0, -0.5, 7.0);
            let spin = Vec3::new(0.2, 0.9, -0.3);
            let body = TurretBody {
                mass: turret_mass,
                angular_inertia: AngularInertia::from_shape(
                    &Cuboid::new(1.7, 1.2, 2.3),
                    turret_mass,
                ),
                centre_of_mass: Vec3::new(0.2, 0.1, -0.3),
            };
            let tank = app
                .world_mut()
                .spawn((
                    current,
                    Position(current.translation),
                    Rotation(current.rotation),
                    explicit_mass(mass, centre, inertia),
                    LinearVelocity(linear),
                    AngularVelocity(spin),
                    RigidBody::Dynamic,
                ))
                .id();
            // RigidBody's add hook seeds GlobalTransform from Transform. Recreate the
            // post-Writeback window only after that hook, so both propagated frames are old.
            app.world_mut()
                .entity_mut(tank)
                .insert(GlobalTransform::from(old));
            let turret = app
                .world_mut()
                .spawn((
                    Turret,
                    body,
                    ring,
                    GlobalTransform::from(old) * ring,
                    ChildOf(tank),
                ))
                .id();
            // Fixture colliders have their own density; NoAuto* must keep their contributions
            // out of both explicit bodies when collider membership and mass are recomputed.
            for owner in [tank, turret] {
                app.world_mut().spawn((
                    Collider::cuboid(1.0, 1.0, 1.0),
                    ColliderDensity(900.0),
                    Transform::IDENTITY,
                    ChildOf(owner),
                ));
            }
            link_rig(app.world_mut(), tank, turret);
            // Exercise the owner collection with simultaneous emptied pools.
            for _ in 0..2 {
                app.world_mut().spawn((VolumeOf(tank), CookedOff));
            }
            let pose = current * ring;
            let turret_centre = pose.transform_point(body.centre_of_mass);
            let ((lift, throw), (pitch, yaw)) = (
                cookoff_launch().launch_velocity.0,
                cookoff_launch().launch_spin.0,
            );
            let impulse = body.mass * (*pose.up() * lift + *pose.forward() * throw);
            let angular_impulse = body
                .angular_inertia
                .tensor()
                .rotated(pose.rotation)
                .to_mat3()
                * (*pose.right() * pitch + *pose.up() * yaw)
                + turret_centre.cross(impulse);
            let p = mass * linear;
            let l = inertia.tensor().rotated(current.rotation).to_mat3() * spin
                + current.transform_point(centre).cross(p);
            cases.push((tank, turret, mass, p + impulse, l + angular_impulse));
        }
        app.world_mut()
            .run_system_once(launch_turrets_on_cookoff)
            .unwrap();
        for pass in 0..2 {
            for &(tank, turret, total_mass, expected_p, expected_l) in &cases {
                let world = app.world();
                let body = world.get::<TurretBody>(turret).unwrap();
                assert_eq!(world.get::<Mass>(turret).unwrap().0, body.mass);
                assert_eq!(
                    world.get::<CenterOfMass>(turret).unwrap().0,
                    body.centre_of_mass
                );
                assert_eq!(
                    *world.get::<AngularInertia>(turret).unwrap(),
                    body.angular_inertia
                );
                assert!(world.get::<LaunchedTurret>(turret).is_some());
                let mut mass = 0.0;
                let (mut p, mut l) = (Vec3::ZERO, Vec3::ZERO);
                for entity in [tank, turret] {
                    // Read the solver-facing properties: updating only the authored components
                    // would pass a bookkeeping test while Avian still steps the wrong body.
                    let m = world.get::<ComputedMass>(entity).unwrap().value();
                    let c = world.get::<ComputedCenterOfMass>(entity).unwrap().0;
                    let inertia = *world.get::<ComputedAngularInertia>(entity).unwrap();
                    let rotation = world.get::<Rotation>(entity).unwrap().0;
                    let centre = world.get::<Position>(entity).unwrap().0 + rotation * c;
                    let momentum = m * world.get::<LinearVelocity>(entity).unwrap().0;
                    mass += m;
                    p += momentum;
                    l += inertia.rotated(rotation).tensor().to_mat3()
                        * world.get::<AngularVelocity>(entity).unwrap().0
                        + centre.cross(momentum);
                    assert!((m - world.get::<Mass>(entity).unwrap().0).abs() < total_mass * 1e-6);
                    assert_eq!(c, world.get::<CenterOfMass>(entity).unwrap().0);
                    let authored = world
                        .get::<AngularInertia>(entity)
                        .unwrap()
                        .tensor()
                        .to_mat3();
                    let error = (authored - inertia.tensor().to_mat3())
                        .to_cols_array()
                        .into_iter()
                        .map(f32::abs)
                        .fold(0.0, f32::max);
                    assert!(error < authored.x_axis.x * 1e-5);
                }
                // Resolution: relative f32 allowance for subtraction, principal-axis
                // diagonalization, inverse-tensor storage and summing the two bodies.
                assert!(
                    (mass - total_mass).abs() < total_mass * 1e-5,
                    "mass pass {pass}"
                );
                assert!(
                    (p - expected_p).length() < expected_p.length() * 1e-5,
                    "linear pass {pass}: {p:?} vs {expected_p:?}"
                );
                assert!(
                    (l - expected_l).length() < expected_l.length() * 1e-5,
                    "angular pass {pass}: {l:?} vs {expected_l:?}"
                );
            }
            if pass == 0 {
                // The fork's real Prepare pass recomputes changed mass properties; paused physics
                // leaves poses/motion fixed so this checks that it preserves the same split.
                app.world_mut().run_schedule(FixedPostUpdate);
            }
        }
        // A different pool empties after the assembly left. It cannot subtract that body again.
        let before: Vec<_> = cases
            .iter()
            .map(|(tank, ..)| app.world().get::<Mass>(*tank).unwrap().0)
            .collect();
        for despawn in [false, true] {
            for &(tank, turret, ..) in &cases {
                if despawn {
                    app.world_mut().despawn(turret);
                }
                app.world_mut().spawn((VolumeOf(tank), CookedOff));
            }
            app.world_mut()
                .run_system_once(launch_turrets_on_cookoff)
                .unwrap();
            for ((tank, ..), before) in cases.iter().zip(&before) {
                assert_eq!(app.world().get::<Mass>(*tank).unwrap().0, *before);
            }
        }
    }

    /// The game's damage model: fixture full-vehicle properties and motion. Position has advanced
    /// past propagation, as after physics Writeback; detachment must rebase the relative assembly
    /// pose onto it rather than leave the assembly behind at the old world pose.
    fn hull(pose: Transform, linear: Vec3, angular: Vec3) -> impl Bundle {
        (
            GlobalTransform::from(pose),
            Position(pose.translation + Vec3::new(0.3, 0.0, -0.2)),
            Rotation(pose.rotation),
            explicit_mass(
                10_000.0,
                Vec3::new(0.0, 0.5, 0.3),
                AngularInertia::new(Vec3::new(40_000.0, 50_000.0, 60_000.0)),
            ),
            LinearVelocity(linear),
            AngularVelocity(angular),
        )
    }

    /// The game's damage model: a turret on `tank` at world `pose`, carrying fixture body data.
    fn turret_on(world: &mut World, tank: Entity, pose: GlobalTransform) -> Entity {
        let turret = world
            .spawn((
                Turret,
                TurretBody {
                    mass: 1234.0,
                    angular_inertia: avian3d::prelude::AngularInertia::new(Vec3::new(
                        700.0, 900.0, 1100.0,
                    )),
                    centre_of_mass: Vec3::new(0.1, 0.4, -0.2),
                },
                ChildOf(tank),
                pose,
            ))
            .id();
        link_rig(world, tank, turret);
        turret
    }

    fn link_rig(world: &mut World, tank: Entity, turret: Entity) {
        world.entity_mut(tank).insert(Rig {
            hull: tank,
            turret,
            gunner_yaw: turret,
            gun: turret,
            muzzle: turret,
        });
    }

    /// The game's damage model: a mounted assembly cannot lose required construction data.
    /// This is a composition bug, not the named state of an assembly that has already left.
    #[test]
    #[should_panic(expected = "a mounted assembly has Turret, TurretBody and GlobalTransform")]
    fn a_mounted_assembly_missing_body_data_panics() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = physics_app();
        let world = app.world_mut();
        let tank = world
            .spawn(hull(Transform::IDENTITY, Vec3::ZERO, Vec3::ZERO))
            .id();
        let turret = turret_on(world, tank, GlobalTransform::IDENTITY);
        world.entity_mut(turret).remove::<TurretBody>();
        world.spawn((VolumeOf(tank), CookedOff));
        world.run_system_once(launch_turrets_on_cookoff).unwrap();
    }

    /// The game's damage model: a missing mounted pose must fail just like missing body data.
    #[test]
    #[should_panic(expected = "a mounted assembly has Turret, TurretBody and GlobalTransform")]
    fn a_mounted_assembly_missing_its_pose_panics() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = physics_app();
        let world = app.world_mut();
        let tank = world
            .spawn(hull(Transform::IDENTITY, Vec3::ZERO, Vec3::ZERO))
            .id();
        let turret = turret_on(world, tank, GlobalTransform::IDENTITY);
        world.entity_mut(turret).remove::<GlobalTransform>();
        world.spawn((VolumeOf(tank), CookedOff));
        world.run_system_once(launch_turrets_on_cookoff).unwrap();
    }

    /// The game's damage model: velocity continuity at a cook-off. The detached assembly leaves
    /// with the motion the hull point at its mass centre had, v + ω × r with both ends of r from
    /// one moment's poses, and the hull's spin. On top, the release adds `cookoff_launch()`'s
    /// increment along the assembly's own axes, whatever the hull was doing: from rest it leaves
    /// with that increment alone. Fixture poses and motion.
    #[test]
    fn a_launched_turret_keeps_the_hull_velocity_it_left_with() {
        let mut app = App::new();
        app.add_systems(Update, launch_turrets_on_cookoff);
        let root = Transform::from_xyz(1.0, 0.2, -3.0).with_rotation(Quat::from_rotation_y(0.6));
        // Fixture ring, off the hull's mass centre and turned on it.
        let ring = Transform::from_xyz(0.5, 1.8, 0.4).with_rotation(Quat::from_rotation_y(0.5));
        let pose = GlobalTransform::from(root) * ring;
        let (linear, angular) = (Vec3::new(4.0, -0.5, 7.0), Vec3::new(0.2, 0.9, -0.3));
        let launch = [(linear, angular), (Vec3::ZERO, Vec3::ZERO)].map(|(linear, angular)| {
            let tank = app.world_mut().spawn(hull(root, linear, angular)).id();
            app.world_mut().spawn((VolumeOf(tank), CookedOff));
            (tank, turret_on(app.world_mut(), tank, pose))
        });
        let hull_centre = GlobalTransform::from(root)
            .transform_point(app.world().get::<CenterOfMass>(launch[0].0).unwrap().0);
        app.update();
        let world = app.world();
        let [moving, resting] = launch.map(|(_, turret)| {
            (
                world.get::<LinearVelocity>(turret).unwrap().0,
                world.get::<AngularVelocity>(turret).unwrap().0,
            )
        });
        let (_, turret) = launch[0];
        let centre = pose.transform_point(world.get::<TurretBody>(turret).unwrap().centre_of_mass);
        let carried = linear + angular.cross(centre - hull_centre);
        let data = cookoff_launch();
        let ((lift, throw), (pitch, yaw)) = (data.launch_velocity.0, data.launch_spin.0);
        let (up, right, forward) = (
            Vec3::from(pose.up()),
            Vec3::from(pose.right()),
            Vec3::from(pose.forward()),
        );
        for (got, expected) in [
            (resting.0, up * lift + forward * throw),
            (resting.1, right * pitch + up * yaw),
            (moving.0 - resting.0, carried),
            (moving.1 - resting.1, angular),
        ] {
            assert!(
                (got - expected).length() <= 1e-5 * expected.length(),
                "{got:?}, expected {expected:?}"
            );
        }
    }

    /// The game's damage model: detachment preserves each collider's contact law. Wreck physics
    /// relies on frictional assemblies resting where a frictionless control keeps sliding.
    /// This is a class fixture, not a check of shipped materials or proxy construction.
    /// Fixture: box bodies, a twenty-degree slope, friction above tan(slope), and a fixed settling
    /// horizon and launch motion. Removing friction at detachment fails the resting body; disabling motion fails
    /// the control. Rest is the solver's Sleeping state, not a speed pinned to an earlier run.
    #[test]
    fn a_launched_turret_comes_to_rest_on_a_slope() {
        use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody, Sleeping};
        let mut app = physics_app();
        app.add_systems(Update, launch_turrets_on_cookoff);

        let slope = Quat::from_rotation_x(20f32.to_radians());
        let ground = crate::substances::contact_friction(0.8);
        let gripping = crate::substances::contact_friction(0.8);
        assert!(gripping.combine(ground).dynamic_coefficient > 20f32.to_radians().tan());
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(2000.0, 1.0, 2000.0),
            Transform::from_rotation(slope).with_translation(slope * Vec3::new(0.0, -0.5, 0.0)),
            ground,
        ));
        let mut turrets = Vec::new();
        let frictionless = crate::substances::contact_friction(0.0);
        for (i, friction) in [gripping, frictionless].into_iter().enumerate() {
            let tank = app
                .world_mut()
                .spawn((
                    Transform::from_rotation(slope)
                        .with_translation(slope * Vec3::new(100.0 * i as f32, 1.0, 0.0)),
                    RigidBody::Static,
                    explicit_mass(
                        10_000.0,
                        Vec3::ZERO,
                        AngularInertia::new(Vec3::splat(50_000.0)),
                    ),
                ))
                .id();
            let turret = app
                .world_mut()
                .spawn((
                    Turret,
                    TurretBody {
                        mass: 1000.0,
                        angular_inertia: avian3d::prelude::AngularInertia::from_shape(
                            &Cuboid::new(2.0, 1.0, 2.0),
                            1000.0,
                        ),
                        centre_of_mass: Vec3::ZERO,
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
            link_rig(app.world_mut(), tank, turret);
            turrets.push((tank, turret));
        }
        for _ in 0..4 {
            app.update();
        }
        for &(tank, ..) in &turrets {
            app.world_mut().spawn((VolumeOf(tank), CookedOff));
        }
        app.update();
        // Fixture: a gentle release, 2 m/s along the slope normal, with no spin. The contact
        // comparison owns this motion; shipped cook-off increments may change independently.
        for &(_, turret) in &turrets {
            assert!(app.world().get::<LaunchedTurret>(turret).is_some());
            *app.world_mut().get_mut::<LinearVelocity>(turret).unwrap() =
                LinearVelocity(slope * Vec3::Y * 2.0);
            *app.world_mut().get_mut::<AngularVelocity>(turret).unwrap() = AngularVelocity::ZERO;
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
        println!(
            "game wreck fixture on slope: gripping {launched} m/s, frictionless {control} m/s"
        );
        assert!(
            app.world().get::<Sleeping>(turrets[0].1).is_some(),
            "the gripping assembly must rest, got {launched} m/s"
        );
        assert!(
            app.world().get::<Sleeping>(turrets[1].1).is_none(),
            "the frictionless control must keep sliding, got {control} m/s"
        );
    }

    /// The game's damage model: launched armour reaches the world at bit-exact unit scale, whatever pose the
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
        use avian3d::prelude::{Collider, RigidBody};
        let mut app = physics_app();
        app.add_systems(Update, launch_turrets_on_cookoff);

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
            let tank = app
                .world_mut()
                .spawn((
                    hull,
                    RigidBody::Static,
                    explicit_mass(
                        10_000.0,
                        Vec3::ZERO,
                        AngularInertia::new(Vec3::splat(50_000.0)),
                    ),
                ))
                .id();
            let turret = app
                .world_mut()
                .spawn((
                    Turret,
                    TurretBody {
                        mass: 1000.0,
                        angular_inertia: avian3d::prelude::AngularInertia::from_shape(
                            &Cuboid::from_length(1.0),
                            1000.0,
                        ),
                        centre_of_mass: Vec3::ZERO,
                    },
                    ring,
                    ChildOf(tank),
                ))
                .id();
            link_rig(app.world_mut(), tank, turret);
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
