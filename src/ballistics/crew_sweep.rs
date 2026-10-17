//! The crew-outcome sweep (ADR-0039): random main-gun hits on the bound tank under the witness law
//! and under the law it replaced, counted per crewman as dead, wounded and untouched, and per hit
//! as a tank killed (living crew < 2); and, over the same hits, the crew outcomes that change with
//! the spall cone's direction count. A measurement, run on demand:
//!
//! `cargo test --release --lib crew_sweep -- --ignored --nocapture`
//!
//! The hits are [`PER_BEARING`] per bearing of [`BEARINGS`], each a horizontal shot aimed
//! uniformly at random over the rectangle the tank's colliders span across the line of fire and
//! kept when its line meets a hard volume of the tank, so the kept hits are uniform over the struck
//! silhouette. The tank stands at its spawn pose, turret and gun at rest; the round is the weapon
//! on the primary trigger's, at its muzzle speed, with its own wound rate. Every hit meets a whole
//! crew: hit points are restored between hits.
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

/// The resistance factor (reference-mm per metre) crew wore as a hard substance: the `Flesh`
/// entry of `materials.ron` before ADR-0039 (TUNED there, a low ballistic-gelatin equivalence).
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

/// One hit as the crew came out of it.
#[derive(Clone)]
struct Hit {
    /// Each crewman's outcome, in station order.
    crew: Vec<(CrewStation, Outcome)>,
    /// The shot's verdict on the tank was a penetration.
    penetrated: bool,
}

impl Hit {
    /// Fewer than two crew living.
    fn killed(&self) -> bool {
        self.crew
            .iter()
            .filter(|(_, o)| *o != Outcome::Dead)
            .count()
            < 2
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

/// The hits that left the tank killed.
fn killed(outcomes: &[Hit]) -> usize {
    outcomes.iter().filter(|hit| hit.killed()).count()
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

/// The kept hits, `(bearing, origin, direction)`, drawn in `app` (a witness-law world).
fn draw_hits(app: &mut App) -> Vec<(f32, Vec3, Vec3)> {
    let world = app.world_mut();
    let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for aabb in world
        .query_filtered::<&ColliderAabb, With<BallisticSurfaces>>()
        .iter(world)
    {
        lo = lo.min(aabb.min);
        hi = hi.max(aabb.max);
    }
    let centre = (lo + hi) / 2.0;
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            ) - centre
        })
        .collect();
    let standoff = (hi - lo).length() + 5.0;
    let mut state = SEED;
    let mut hits = Vec::new();
    for bearing in BEARINGS {
        let b = bearing.to_radians();
        // The tank's front is -Z: a shot from bearing 0 travels +Z.
        let direction = Vec3::new(-b.sin(), 0.0, b.cos());
        let across = Vec3::Y.cross(direction).normalize();
        let span = |axis: Vec3| {
            corners
                .iter()
                .map(|corner| corner.dot(axis))
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), x| {
                    (a.min(x), b.max(x))
                })
        };
        let ((u0, u1), (v0, v1)) = (span(across), span(Vec3::Y));
        let dir = Dir3::new(direction).expect("a unit bearing");
        let mut kept = 0;
        while kept < PER_BEARING {
            let u = u0 + (u1 - u0) * unit(&mut state);
            let v = v0 + (v1 - v0) * unit(&mut state);
            let origin = centre + across * u + Vec3::Y * v - direction * standoff;
            let struck = app
                .world_mut()
                .run_system_once(move |spatial: SpatialQuery| {
                    spatial
                        .cast_ray(
                            origin,
                            dir,
                            2.0 * standoff,
                            true,
                            &SpatialQueryFilter::from_mask(crate::Layer::Armor),
                        )
                        .is_some()
                })
                .expect("the ray query runs");
            if struck {
                hits.push((bearing, origin, direction));
                kept += 1;
            }
        }
    }
    hits
}

/// Fire the main gun along each hit in `app`, each at a whole crew, and read what the crew came
/// out as.
fn fire_hits(app: &mut App, hits: &[(f32, Vec3, Vec3)]) -> Outcomes {
    let spec = app
        .world()
        .resource::<crate::bake::TankBlueprint>()
        .spec
        .clone();
    let weapon = spec
        .weapons
        .values()
        .find(|weapon| weapon.trigger == Trigger::Primary)
        .expect("the tank has a weapon on the primary trigger");
    let round = weapon
        .resolved_round()
        .expect("the main gun's round resolves");
    let tank = app.world().resource::<super::fuzz::ProbeTank>().0;
    let mut out = Vec::with_capacity(hits.len());
    for &(_, origin, direction) in hits {
        let world = app.world_mut();
        for mut health in world.query::<&mut ComponentHealth>().iter_mut(world) {
            health.current = health.max;
        }
        let (mut projectile, readout) =
            march_tests::fixture_round(round.body, direction * round.muzzle_speed);
        projectile.wound = round.wound;
        let shell = world
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
        let verdict = ShotVerdict::of_path(&record::path_steps(world, shell, Some(tank)));
        world.despawn(shell);
        out.push(Hit {
            crew,
            penetrated: verdict == Some(ShotVerdict::Penetration),
        });
    }
    out
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
    let k = killed(outcomes);
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
        "  tanks killed (crew < 2): {k}/{hits} = {:.3} per hit; crew dead per hit {:.3}",
        k as f64 / hits as f64,
        total[0] as f64 / hits as f64
    );
    println!(
        "  penetrations {}/{hits}; per penetration: crew dead {:.3}, a crewman touched {:.3}, \
         tank killed {:.3}",
        penetrating.len(),
        dead_pen as f64 / penetrating.len().max(1) as f64,
        touched_pen as f64 / penetrating.len().max(1) as f64,
        killed(&penetrating) as f64 / penetrating.len().max(1) as f64
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
        tank += usize::from(x.killed() != y.killed());
    }
    let mut total = 0;
    for (station, row) in &flips {
        let sum: usize = row.values().sum();
        total += sum;
        println!("  {:<12} {sum:>4}  {row:?}", station.label());
    }
    println!("  all crew     {total:>4}; tank-killed verdict flipped on {tank} hits");
}

/// The sweep, printed as a table per law and the flips between direction counts.
#[test]
#[ignore = "a measurement, run on demand"]
fn crew_outcome_sweep() {
    let shipped = spall_cone::ConeShape::shipped().directions_count();
    let mut witness = sweep_world(None);
    let hits = draw_hits(&mut witness);
    println!(
        "{} hits: {PER_BEARING} per bearing at {BEARINGS:?} degrees, seed {SEED:#x}, tank at \
         {PROBE_TANK_AT}",
        hits.len()
    );
    let new = fire_hits(&mut witness, &hits);
    let mut replaced_at = |directions: usize| {
        let mut app = sweep_world(Some(directions));
        replace_witnesses(&mut app);
        fire_hits(&mut app, &hits)
    };
    let old = replaced_at(REPLACED_DIRECTIONS);
    // The rebuild checked against the record the replaced law wrote: the first scenario's doses.
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
    for directions in RESOLUTIONS {
        let finer = fire_hits(&mut sweep_world(Some(directions)), &hits);
        print_law(&format!("witness law, N = {directions}"), &finer);
        print_flips(
            &format!("witness law, N = {shipped} vs N = {directions}"),
            &new,
            &finer,
        );
    }
    println!("\nper bearing (witness law, N = {shipped}): crew dead per hit, tanks killed");
    for (bearing, chunk) in BEARINGS.iter().zip(new.chunks(PER_BEARING)) {
        let dead: usize = per_crewman(chunk).values().map(|row| row[0]).sum();
        println!(
            "  {bearing:>5}°  {:.2}  {}  penetrations {}",
            dead as f64 / chunk.len() as f64,
            killed(chunk),
            chunk.iter().filter(|hit| hit.penetrated).count()
        );
    }
}
