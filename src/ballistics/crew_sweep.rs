//! The crew-outcome sweep (ADR-0040): main-gun hits on the bound tank under the witness law and
//! under the law it replaced, counted per crewman as dead, wounded and untouched, and per hit as a
//! tank knocked out under each [`Rule`]; over the same random hits, the crew outcomes that change
//! with the spall cone's direction count; and aimed shots fired in sequence at one tank until it is
//! knocked out, as shots and pen verdicts to the kill. A measurement, run on demand:
//!
//! `cargo test --release --lib crew_sweep -- --ignored --nocapture`
//!
//! A PEN VERDICT is a shot whose whole-shot verdict is [`ShotVerdict::Penetration`]: the round
//! exited the main armour. A shot stopped in the armour is not one, whatever it damaged inside, so
//! a knockout with zero pen verdicts is possible (a box that reaches into a plate is crossed by a
//! round stopping there).
//!
//! The hits are [`PER_BEARING`] per bearing of [`BEARINGS`], each a horizontal shot aimed
//! uniformly at random over the rectangle an [`Aim`]'s colliders span across the line of fire and
//! kept when its line meets one of them, so the kept hits are uniform over that silhouette:
//! [`Aim::Random`] the whole tank's (its hard volumes), [`Aim::Crew`] the union of the crew boxes'
//! (an aimed shot). The tank stands at its spawn pose, turret and gun at rest; the round is the weapon
//! on the primary trigger's, at its muzzle speed, with its own wound rate. Every single hit meets a
//! whole crew: hit points are restored between hits. A sequence restores them only between trials,
//! each shot's bearing drawn from [`BEARINGS`]. An ammunition volume left at zero hit points has
//! cooked off.
//!
//! The replaced law is rebuilt in the same world: every witness volume becomes a hard volume of
//! [`REPLACED_FLESH_FACTOR`] on the armour layer, walked, charged, thrown from and shadowed by as
//! any other hard body. Every law fires the same hits, drawn from [`SEED`].

use avian3d::prelude::{
    ColliderAabb, CollisionLayers, LayerMask, SpatialQuery, SpatialQueryFilter,
};
use bevy::ecs::system::RunSystemOnce;
use bevy::time::TimeUpdateStrategy;
use std::collections::BTreeMap;

use super::fuzz::{PROBE_TANK_AT, probe_world};
use super::pen_cam::{STEP, STEPS};
use super::*;
use crate::damage::CrewStation;
use crate::damage_cam::{ShotVerdict, record};
use crate::spec::Trigger;

/// The bearings a hit comes from (degrees clockwise from the tank's front, seen from above):
/// fixture data.
const BEARINGS: [f32; 8] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0];

/// Kept hits per bearing: fixture data.
const PER_BEARING: usize = 64;

/// The draw's seed: fixture data.
const SEED: u64 = 0x5eed_c0de_0039;

/// The sequential draw's seed: fixture data.
const SEQUENCE_SEED: u64 = 0x5eed_c0de_5e05;

/// Sequences fired, each at a fresh tank: fixture data.
const TRIALS: usize = 128;

/// Shots a sequence gives up after when the tank is still not knocked out: fixture data.
const SHOT_CAP: usize = 60;

/// The resistance factor (reference-mm per metre) crew wore as a hard substance: the `Flesh`
/// entry of `materials.ron` before ADR-0040 (TUNED there, a low ballistic-gelatin equivalence).
const REPLACED_FLESH_FACTOR: f32 = 10.0;

/// The direction counts the witness law is fired at besides the shipped one, to show the
/// outcomes that move with resolution.
const RESOLUTIONS: [usize; 2] = [128, 256];

/// The direction count the replaced law shipped with, before the count was cut: its law as it
/// stood.
const REPLACED_DIRECTIONS: usize = 256;

/// What a crewman came out of one hit as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    Dead,
    Wounded,
    Untouched,
}

impl Outcome {
    fn column(self) -> usize {
        match self {
            Outcome::Dead => 0,
            Outcome::Wounded => 1,
            Outcome::Untouched => 2,
        }
    }
}

/// When a tank is knocked out (playtest fork F2).
#[derive(Clone, Copy, Debug)]
enum Rule {
    /// The shipped rule, [`crate::damage::knockout_from_counts`]: cook-off, or no crew living.
    ZeroCrew,
    /// The kept alternative: cook-off, or fewer than two crew living.
    UnderTwo,
}

impl Rule {
    const ALL: [Rule; 2] = [Rule::ZeroCrew, Rule::UnderTwo];

    fn label(self) -> &'static str {
        match self {
            Rule::ZeroCrew => "zero crew (F2 A, shipped)",
            Rule::UnderTwo => "crew < 2 (F2 B)",
        }
    }
}

/// One hit as the crew came out of it.
#[derive(Clone)]
struct Hit {
    /// Each crewman's outcome, in station order.
    crew: Vec<(CrewStation, Outcome)>,
    /// A pen verdict (module doc).
    penetrated: bool,
    /// An ammunition volume stands at zero hit points.
    cooked: bool,
}

impl Hit {
    fn living(&self) -> usize {
        self.crew
            .iter()
            .filter(|(_, o)| *o != Outcome::Dead)
            .count()
    }

    /// The tank is knocked out under `rule`.
    fn knocked_out(&self, rule: Rule) -> bool {
        match rule {
            Rule::ZeroCrew => crate::damage::knockout_from_counts(
                self.crew.len() as u32,
                self.living() as u32,
                self.cooked,
            )
            .is_some(),
            Rule::UnderTwo => self.cooked || self.living() < 2,
        }
    }
}

/// Every hit, in hit order.
type Outcomes = Vec<Hit>;

/// Per crewman (by station, in station order) the hits that left him dead, wounded and untouched.
fn per_crewman(outcomes: &[Hit]) -> BTreeMap<CrewStation, [usize; 3]> {
    let mut rows = BTreeMap::<CrewStation, [usize; 3]>::new();
    for hit in outcomes {
        for &(station, outcome) in &hit.crew {
            rows.entry(station).or_default()[outcome.column()] += 1;
        }
    }
    rows
}

/// The hits that left the tank knocked out under `rule`.
fn killed(outcomes: &[Hit], rule: Rule) -> usize {
    outcomes.iter().filter(|hit| hit.knocked_out(rule)).count()
}

/// splitmix64: a deterministic stream, so a rerun draws the same hits.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A uniform draw in `[0, 1)`.
fn unit(state: &mut u64) -> f32 {
    (next(state) >> 40) as f32 / (1u64 << 24) as f32
}

/// The probe world with the march driven on [`STEP`], throwing the shipped shape at `directions`
/// (its own count when `None`).
fn sweep_world(directions: Option<usize>) -> App {
    let mut app = probe_world().expect("the bound tank builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
        .insert_resource(RetainSpentShells(true))
        .insert_resource(MgShortCircuit(false))
        .add_systems(Update, integrate_projectiles);
    if let Some(directions) = directions {
        app.insert_resource(spall_cone::SpallLaw(spall_cone::ConeShape::shipped_at(
            directions,
        )));
    }
    app
}

/// Rebuild `app`'s witnesses as hard volumes of [`REPLACED_FLESH_FACTOR`] on the armour layer.
fn replace_witnesses(app: &mut App) {
    let world = app.world_mut();
    let nodes: Vec<(Entity, String)> = world
        .query::<(Entity, &WitnessVolume)>()
        .iter(world)
        .map(|(entity, volume)| (entity, volume.substance.clone()))
        .collect();
    assert!(
        !nodes.is_empty(),
        "the bound tank has no witness to replace"
    );
    for (node, substance) in nodes {
        world
            .entity_mut(node)
            .remove::<WitnessVolume>()
            .insert(BallisticVolume {
                material: walk::VolumeMaterial::fixture(REPLACED_FLESH_FACTOR),
                substance,
            });
    }
    let surfaces: Vec<Entity> = world
        .query_filtered::<Entity, With<WitnessSurface>>()
        .iter(world)
        .collect();
    for surface in surfaces {
        world
            .entity_mut(surface)
            .remove::<WitnessSurface>()
            .insert(CollisionLayers::new([crate::Layer::Armor], LayerMask::NONE));
    }
    for _ in 0..4 {
        app.update();
    }
}

/// What a hit is aimed at: the silhouette it is uniform over.
#[derive(Clone, Copy, Debug)]
enum Aim {
    /// The whole tank: every hard volume.
    Random,
    /// The union of the crew boxes: the witness volumes.
    Crew,
}

/// Draws shots uniform over an [`Aim`]'s silhouette from a bearing.
struct Aimer {
    centre: Vec3,
    /// The aimed bounds' corners, about `centre`.
    corners: Vec<Vec3>,
    /// The distance a shot starts from `centre`, clear of the whole tank.
    standoff: f32,
    layer: crate::Layer,
}

impl Aimer {
    fn new(app: &mut App, aim: Aim) -> Self {
        let ((lo, hi), layer) = match aim {
            Aim::Random => (world_bounds::<BallisticSurfaces>(app), crate::Layer::Armor),
            Aim::Crew => (world_bounds::<WitnessSurface>(app), crate::Layer::Witness),
        };
        let (tank_lo, tank_hi) = world_bounds::<BallisticSurfaces>(app);
        let centre = (lo + hi) / 2.0;
        let corners = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                ) - centre
            })
            .collect();
        Self {
            centre,
            corners,
            standoff: (tank_hi - tank_lo).length() + 5.0,
            layer,
        }
    }

    /// A shot `(origin, direction)` from `bearing` (degrees) whose line meets the aimed colliders.
    fn draw(&self, app: &mut App, bearing: f32, state: &mut u64) -> (Vec3, Vec3) {
        let b = bearing.to_radians();
        // The tank's front is -Z: a shot from bearing 0 travels +Z.
        let direction = Vec3::new(-b.sin(), 0.0, b.cos());
        let across = Vec3::Y.cross(direction).normalize();
        let span = |axis: Vec3| {
            self.corners
                .iter()
                .map(|corner| corner.dot(axis))
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), x| {
                    (a.min(x), b.max(x))
                })
        };
        let ((u0, u1), (v0, v1)) = (span(across), span(Vec3::Y));
        let dir = Dir3::new(direction).expect("a unit bearing");
        let (standoff, layer) = (self.standoff, self.layer);
        loop {
            let u = u0 + (u1 - u0) * unit(state);
            let v = v0 + (v1 - v0) * unit(state);
            let origin = self.centre + across * u + Vec3::Y * v - direction * standoff;
            let struck = app
                .world_mut()
                .run_system_once(move |spatial: SpatialQuery| {
                    spatial
                        .cast_ray(
                            origin,
                            dir,
                            2.0 * standoff,
                            true,
                            &SpatialQueryFilter::from_mask(layer),
                        )
                        .is_some()
                })
                .expect("the ray query runs");
            if struck {
                return (origin, direction);
            }
        }
    }
}

/// The kept hits, `(origin, direction)`, [`PER_BEARING`] per bearing in [`BEARINGS`] order, drawn
/// in `app` (a witness-law world).
fn draw_hits(app: &mut App, aim: Aim) -> Vec<(Vec3, Vec3)> {
    let aimer = Aimer::new(app, aim);
    let mut state = SEED;
    let mut hits = Vec::new();
    for bearing in BEARINGS {
        for _ in 0..PER_BEARING {
            hits.push(aimer.draw(app, bearing, &mut state));
        }
    }
    hits
}

/// The world bounds of every collider carrying `C`: `(min, max)`.
fn world_bounds<C: Component>(app: &mut App) -> (Vec3, Vec3) {
    let world = app.world_mut();
    let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for aabb in world.query_filtered::<&ColliderAabb, With<C>>().iter(world) {
        lo = lo.min(aabb.min);
        hi = hi.max(aabb.max);
    }
    assert!(lo.x <= hi.x, "no collider to aim at");
    (lo, hi)
}

/// The main gun's round, ready to fire.
fn main_round(app: &App) -> crate::rounds::Round {
    let spec = &app.world().resource::<crate::bake::TankBlueprint>().spec;
    spec.weapons
        .values()
        .find(|weapon| weapon.trigger == Trigger::Primary)
        .expect("the tank has a weapon on the primary trigger")
        .resolved_round()
        .expect("the main gun's round resolves")
}

/// Restore every hit point in `app`: a fresh tank.
fn restore(app: &mut App) {
    let world = app.world_mut();
    for mut health in world.query::<&mut ComponentHealth>().iter_mut(world) {
        health.current = health.max;
    }
}

/// Fire `round` from `origin` along `direction` in `app` and read the tank as it is after.
fn fire_one(app: &mut App, round: &crate::rounds::Round, origin: Vec3, direction: Vec3) -> Hit {
    let tank = app.world().resource::<super::fuzz::ProbeTank>().0;
    let (mut projectile, readout) = march_tests::fixture_round_with(
        round.body,
        round.penetrator,
        direction * round.muzzle_speed,
    );
    projectile.wound = round.wound;
    let shell = app
        .world_mut()
        .spawn((
            projectile,
            readout,
            DamageReport::default(),
            TerminalReport::default(),
            ShellPath {
                points: vec![origin],
                segment_starts: Vec::new(),
            },
            PenetrationMarks::default(),
            SpallMarks::default(),
            Transform::from_translation(origin).looking_to(direction, Vec3::Y),
        ))
        .id();
    for _ in 0..STEPS {
        app.update();
    }
    let world = app.world_mut();
    let mut crew: Vec<(CrewStation, Outcome)> = world
        .query::<(&CrewStation, &ComponentHealth)>()
        .iter(world)
        .map(|(station, health)| {
            let outcome = match health.current {
                0 => Outcome::Dead,
                current if current < health.max => Outcome::Wounded,
                _ => Outcome::Untouched,
            };
            (*station, outcome)
        })
        .collect();
    crew.sort_by_key(|(station, _)| *station);
    let cooked = world
        .query_filtered::<&ComponentHealth, With<crate::damage::Ammo>>()
        .iter(world)
        .any(|health| health.current == 0);
    let verdict = ShotVerdict::of_path(&record::path_steps(world, shell, Some(tank)));
    world.despawn(shell);
    Hit {
        crew,
        penetrated: verdict == Some(ShotVerdict::Penetration),
        cooked,
    }
}

/// Fire the main gun along each hit in `app`, each at a fresh tank, and read what the crew came out
/// as.
fn fire_hits(app: &mut App, hits: &[(Vec3, Vec3)]) -> Outcomes {
    let round = main_round(app);
    hits.iter()
        .map(|&(origin, direction)| {
            restore(app);
            fire_one(app, &round, origin, direction)
        })
        .collect()
}

/// One sequence's end under one rule, when the tank was first knocked out.
#[derive(Clone, Copy)]
struct Kill {
    shots: usize,
    /// The pen verdicts among `shots`.
    pen_verdicts: usize,
    /// The knockout was a cook-off.
    cooked: bool,
}

/// A sequence's [`Kill`], `None` past [`SHOT_CAP`].
type ToKill = Option<Kill>;

/// [`TRIALS`] sequences of aimed shots in `app`, each at a fresh tank, damage retained shot to shot
/// until it is knocked out under every [`Rule`]: per sequence, the kill under each rule.
fn sequences(app: &mut App) -> Vec<[ToKill; 2]> {
    let round = main_round(app);
    let aimer = Aimer::new(app, Aim::Crew);
    let mut state = SEQUENCE_SEED;
    let mut out = Vec::with_capacity(TRIALS);
    for _ in 0..TRIALS {
        restore(app);
        let mut kills: [ToKill; 2] = [None; 2];
        let mut pen_verdicts = 0;
        for shot in 1..=SHOT_CAP {
            let bearing = BEARINGS[(next(&mut state) % BEARINGS.len() as u64) as usize];
            let (origin, direction) = aimer.draw(app, bearing, &mut state);
            let hit = fire_one(app, &round, origin, direction);
            pen_verdicts += usize::from(hit.penetrated);
            for (kill, rule) in kills.iter_mut().zip(Rule::ALL) {
                if kill.is_none() && hit.knocked_out(rule) {
                    *kill = Some(Kill {
                        shots: shot,
                        pen_verdicts,
                        cooked: hit.cooked,
                    });
                }
            }
            if kills.iter().all(Option::is_some) {
                break;
            }
        }
        out.push(kills);
    }
    out
}

/// Each rule's shots-to-kill and pen-verdicts-to-kill, as counts per value.
fn print_sequences(label: &str, runs: &[[ToKill; 2]]) {
    println!("\n{label}: {} trials, cap {SHOT_CAP} shots", runs.len());
    for (i, rule) in Rule::ALL.into_iter().enumerate() {
        let ends: Vec<Kill> = runs.iter().filter_map(|run| run[i]).collect();
        let histogram = |pick: fn(&Kill) -> usize| {
            let mut counts = BTreeMap::<usize, usize>::new();
            for end in &ends {
                *counts.entry(pick(end)).or_default() += 1;
            }
            counts
        };
        let mean = |pick: fn(&Kill) -> usize| {
            ends.iter().map(pick).sum::<usize>() as f64 / ends.len().max(1) as f64
        };
        let median = |pick: fn(&Kill) -> usize| {
            let mut values: Vec<usize> = ends.iter().map(pick).collect();
            values.sort_unstable();
            values.get(values.len() / 2).copied().unwrap_or(0)
        };
        println!(
            "  {}: knocked out in {}/{}, {} by cook-off",
            rule.label(),
            ends.len(),
            runs.len(),
            ends.iter().filter(|kill| kill.cooked).count()
        );
        println!(
            "    shots to kill: mean {:.2}, median {}, {:?}",
            mean(|kill| kill.shots),
            median(|kill| kill.shots),
            histogram(|kill| kill.shots)
        );
        println!(
            "    pen verdicts to kill: mean {:.2}, median {}, {:?}",
            mean(|kill| kill.pen_verdicts),
            median(|kill| kill.pen_verdicts),
            histogram(|kill| kill.pen_verdicts)
        );
    }
}

/// One law's row block: per crewman dead / wounded / untouched, all crew, tanks killed per hit.
fn print_law(label: &str, outcomes: &Outcomes) {
    println!("\n{label}");
    println!("  crewman      dead wounded untouched");
    let mut total = [0usize; 3];
    for (station, row) in per_crewman(outcomes) {
        println!(
            "  {:<12} {:>4} {:>7} {:>9}",
            station.label(),
            row[0],
            row[1],
            row[2]
        );
        for i in 0..3 {
            total[i] += row[i];
        }
    }
    println!(
        "  {:<12} {:>4} {:>7} {:>9}",
        "all crew", total[0], total[1], total[2]
    );
    let hits = outcomes.len();
    let penetrating: Vec<Hit> = outcomes
        .iter()
        .filter(|hit| hit.penetrated)
        .cloned()
        .collect();
    let dead_pen: usize = per_crewman(&penetrating).values().map(|row| row[0]).sum();
    let touched_pen = penetrating
        .iter()
        .filter(|hit| hit.crew.iter().any(|(_, o)| *o != Outcome::Untouched))
        .count();
    println!(
        "  crew dead per hit {:.3}; cooked off {}",
        total[0] as f64 / hits as f64,
        outcomes.iter().filter(|hit| hit.cooked).count()
    );
    for rule in Rule::ALL {
        let k = killed(outcomes, rule);
        println!(
            "  tanks killed, {}: {k}/{hits} = {:.3} per hit, {:.3} per pen verdict",
            rule.label(),
            k as f64 / hits as f64,
            killed(&penetrating, rule) as f64 / penetrating.len().max(1) as f64
        );
    }
    println!(
        "  pen verdicts {}/{hits}; per pen verdict: crew dead {:.3}, a crewman touched {:.3}",
        penetrating.len(),
        dead_pen as f64 / penetrating.len().max(1) as f64,
        touched_pen as f64 / penetrating.len().max(1) as f64
    );
    let mut spread = [0usize; 4];
    for hit in &penetrating {
        let dead = hit.crew.iter().filter(|(_, o)| *o == Outcome::Dead).count();
        spread[dead.min(3)] += 1;
    }
    println!(
        "  crew killed per pen verdict: 0: {}, 1: {}, 2: {}, 3+: {}",
        spread[0], spread[1], spread[2], spread[3]
    );
}

/// Per crewman, the hits whose outcome differs between `a` and `b` (same hits), by transition.
fn print_flips(label: &str, a: &Outcomes, b: &Outcomes) {
    println!("\nflips {label} (per crewman: hits whose outcome changed, as from→to)");
    let mut flips = BTreeMap::<CrewStation, BTreeMap<String, usize>>::new();
    let mut tank = 0;
    for (x, y) in a.iter().zip(b) {
        for (&(station, from), &(other, to)) in x.crew.iter().zip(&y.crew) {
            assert_eq!(station, other, "the same crew in the same order");
            if from != to {
                *flips
                    .entry(station)
                    .or_default()
                    .entry(format!("{from:?}→{to:?}"))
                    .or_default() += 1;
            }
        }
        tank += usize::from(
            Rule::ALL
                .iter()
                .any(|&r| x.knocked_out(r) != y.knocked_out(r)),
        );
    }
    let mut total = 0;
    for (station, row) in &flips {
        let sum: usize = row.values().sum();
        total += sum;
        println!("  {:<12} {sum:>4}  {row:?}", station.label());
    }
    println!("  all crew     {total:>4}; a tank-killed verdict flipped on {tank} hits");
}

/// The sweep, random then aimed: a table per law, and for random hits the flips between direction
/// counts; then the aimed sequences under the witness law.
#[test]
#[ignore = "a measurement, run on demand"]
fn crew_outcome_sweep() {
    sweep(Aim::Random);
    sweep(Aim::Crew);
    let shipped = spall_cone::ConeShape::shipped().directions_count();
    print_sequences(
        &format!("aimed sequences, witness law, N = {shipped} (shipped), seed {SEQUENCE_SEED:#x}"),
        &sequences(&mut sweep_world(None)),
    );
}

/// One aim's sweep, printed.
fn sweep(aim: Aim) {
    let shipped = spall_cone::ConeShape::shipped().directions_count();
    let mut witness = sweep_world(None);
    let hits = draw_hits(&mut witness, aim);
    println!(
        "\n===== {aim:?}: {} hits, {PER_BEARING} per bearing at {BEARINGS:?} degrees, seed \
         {SEED:#x}, tank at {PROBE_TANK_AT}",
        hits.len()
    );
    let new = fire_hits(&mut witness, &hits);
    let replaced_at = |directions: usize| {
        let mut app = sweep_world(Some(directions));
        replace_witnesses(&mut app);
        fire_hits(&mut app, &hits)
    };
    let old = replaced_at(REPLACED_DIRECTIONS);
    if matches!(aim, Aim::Random) {
        // The rebuild checked against the record the replaced law wrote: the first scenario's
        // doses.
        let check = super::pen_cam::record_first_edited(
            Some(&spall_cone::ConeShape::shipped_at(REPLACED_DIRECTIONS)),
            replace_witnesses,
        );
        println!("\nreplaced law, N = {REPLACED_DIRECTIONS}: the first pen cam scenario's doses");
        for dose in &check.doses {
            println!(
                "  {:<14} line {:<12} field {:<12} lost {}",
                dose.volume, dose.line, dose.field, dose.lost
            );
        }
    }
    let old_shipped = replaced_at(shipped);
    print_law(
        &format!(
            "replaced law (crew hard, factor {REPLACED_FLESH_FACTOR}), N = {REPLACED_DIRECTIONS}"
        ),
        &old,
    );
    print_law(
        &format!("replaced law (crew hard, factor {REPLACED_FLESH_FACTOR}), N = {shipped}"),
        &old_shipped,
    );
    print_law(&format!("witness law, N = {shipped} (shipped)"), &new);
    if matches!(aim, Aim::Crew) {
        return;
    }
    for directions in RESOLUTIONS {
        let finer = fire_hits(&mut sweep_world(Some(directions)), &hits);
        print_law(&format!("witness law, N = {directions}"), &finer);
        print_flips(
            &format!("witness law, N = {shipped} vs N = {directions}"),
            &new,
            &finer,
        );
    }
    println!(
        "\nper bearing (witness law, N = {shipped}): crew dead per hit, tanks killed (zero crew)"
    );
    for (bearing, chunk) in BEARINGS.iter().zip(new.chunks(PER_BEARING)) {
        let dead: usize = per_crewman(chunk).values().map(|row| row[0]).sum();
        println!(
            "  {bearing:>5}°  {:.2}  {}  pen verdicts {}",
            dead as f64 / chunk.len() as f64,
            killed(chunk, Rule::ZeroCrew),
            chunk.iter().filter(|hit| hit.penetrated).count()
        );
    }
}
