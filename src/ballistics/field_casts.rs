//! Cross-platform reproducibility of the spall-cone casts.
//!
//! A client may re-cast a spall cone from a shot record's parameters only if the walks the sim runs
//! for it (the collector and the pairing walk along each direction, against the struck tank's
//! armour and boxes) come out bit-identical on every shipped target: Windows x86_64, Linux x86_64,
//! macOS arm64. This test measures that and decides nothing; the decision and its two outcomes are
//! in `.agents/scratch/damage-cam-2026-10-08/product/in-battle/owner-direction-2026-10-10.md`.
//!
//! # What runs
//!
//! The bound fixture tank from [`probe_world`], `RigidBody::Static`, at two fixed poses: spawn
//! pose, and a tilted, yawed pose whose quaternion is literal components normalised by `sqrt`
//! (no trig in the pose). The ray set is laid out in tank-local metres from the tank's armour
//! bounds and each damageable box's bounds, read at spawn pose:
//!
//! - `cone`: spall cones of the shipped shape spread by the ball at a fixture ratio
//!   ([`spall_cone::ConeShape::spread`]) from interior origins, each direction carrying its weight
//!   times a fixture strength;
//! - `lattice`: a grid of slightly off-axis rays into each face of the bounds (crosses seams);
//! - `graze`: rays skimming the four sides and the top at shallow inward angles;
//! - `box`: rays aimed at each damageable box's centre from two quadrants.
//!
//! and, at spawn pose in a fresh world each, `shot`: the fixture main-gun round ([`FIXTURE_ROUND`])
//! fired through the real march along fixture lines ([`SHOTS`]), so the pin covers a whole shot end
//! to end — body-exit detection, the mechanism's spends, energies and speeds, each cone's strength
//! and axis, and its walks. A shot record digests every penetration event, every cone (origin,
//! axis, field, half-angle; its source's spend, energy lost, entry and exit speeds and strength
//! `S`, each as raw bits, so a source parameter that moves no direction's end and no HP still
//! moves the record; and each direction's end and whether it dosed), and the pool units every
//! named box holds after the shot.
//!
//! Each ray goes through two paths:
//!
//! 1. the ordered presence a direction is walked over ([`resolve::field_presence`]), with no
//!    strength: the progress of the ray's origin, then every layer as its `[start, end)` and the
//!    volumes present over it (stable identity and factor);
//! 2. [`resolve::field_direction`] itself, through the probe tank's volumes: the ray's end and
//!    every dose it addresses (box and amount, J) are recorded, and each box's dose
//!    accumulates in f32 in ray order;
//! 3. the witness pass's line ([`super::wound::line`]) along the ray, on a fixture disc at a
//!    fixture wound rate: every witness's chord and raw dose, before any rounding to HP.
//!
//! A shot record also digests each witness's chord and raw dose of every
//! [`ArmourMark::Wound`] the march logged.
//!
//! # Digest
//!
//! Each ray is written as text with every f32 and f64 as its raw IEEE-754 bits in hex, and the text is
//! hashed with 64-bit FNV-1a over its UTF-8 bytes ([`fnv1a`]). The INPUT (origin, direction,
//! strength) and the OUTPUT (hits, ray end, HP) are digested separately, so a red run says whether
//! the direction generator moved or the casts did. The cone directions are built with `libm` and
//! IEEE arithmetic alone ([`spall_cone`]), so their input digests are meant to match everywhere.
//! The bounds the layout is built from are their own `frame` record, and each pose's per-box dose
//! is a `dose` record.
//!
//! `field_casts.pinned` holds one line per record (`label input-digest output-digest`); the digest
//! of those lines is [`PINNED_TOTAL`]. Both are MEASURED on macOS arm64.
//!
//! # Re-pinning and reading a red run
//!
//! A red run prints the first differing record and its full text on this target. Running the
//! test on macOS arm64 with `OVERMATCH_FIELD_CASTS_SHOW=<label prefix>` and `--nocapture` prints
//! the reference text of the records it names. `OVERMATCH_FIELD_CASTS_REPIN=1` rewrites the pinned
//! file and prints the new total for [`PINNED_TOTAL`]; re-pin only on macOS arm64, and only for a
//! change meant to move the casts (geometry, the cast path, the cone shape, an engine bump).

use std::fmt::Write as _;
use std::time::Duration;

use avian3d::prelude::{Collider, ColliderAabb, Position, Rotation};
use bevy::ecs::system::RunSystemOnce;
use bevy::time::TimeUpdateStrategy;

use super::fuzz::{PROBE_TANK_AT, ProbeTank, probe_world};
use super::spall_cone::{ConeShape, FieldDescriptor};
use super::*;

/// The line pass's disc calibre (m), wound rate (reference-mm per metre) and reach (m): fixture
/// data.
const LINE_CALIBER: f32 = 0.088;
const LINE_WOUND: f32 = 30.0;
const LINE_REACH: f32 = 20.0;

/// The digest of every line of `field_casts.pinned`, MEASURED on macOS arm64 (the test profile).
const PINNED_TOTAL: u64 = 0x3a01_b227_5b02_a41f;

const PINNED: &str = include_str!("field_casts.pinned");

const PINNED_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ballistics/field_casts.pinned"
);

/// Fixture shots, `(label, tank-local origin, tank-local direction)`: lines into the driver's plate,
/// the rear engine, the hull's left side at the front crew and amidships, the turret's left side and
/// the hull's right side at the rear. Fixture data.
const SHOTS: [(&str, Vec3, Vec3); 6] = [
    ("driver", Vec3::new(-0.57, 1.45, -6.5), Vec3::Z),
    ("rear", Vec3::new(0.0, 1.2, 8.0), Vec3::NEG_Z),
    ("left-front", Vec3::new(-6.0, 1.3, -1.6), Vec3::X),
    ("left-mid", Vec3::new(-6.0, 1.3, 0.0), Vec3::X),
    ("turret-left", Vec3::new(-6.0, 2.2, 0.3), Vec3::X),
    ("right-rear", Vec3::new(6.0, 1.3, 1.0), Vec3::NEG_X),
];

/// Fixture strength `S` (J): a lattice, graze or box ray carries it whole, a cone
/// direction `S·f_k`. Fixture data.
const STRENGTH: f32 = 30.0;

/// The `cone` family's field ratio `λ` (a 26.1° cone, DERIVED): fixture data.
const CONE_RATIO: f32 = 2.27;

/// 64-bit FNV-1a over bytes. Stable by definition: offset basis `0xcbf29ce484222325`, prime
/// `0x100000001b3`.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn bits(text: &mut String, value: f32) {
    let _ = write!(text, "{:08x}", value.to_bits());
}

fn bits64(text: &mut String, value: f64) {
    let _ = write!(text, "{:016x}", value.to_bits());
}

fn bits3(text: &mut String, value: Vec3) {
    bits(text, value.x);
    text.push(',');
    bits(text, value.y);
    text.push(',');
    bits(text, value.z);
}

/// One digested record.
struct Record {
    label: String,
    input: String,
    output: String,
}

impl Record {
    fn line(&self) -> String {
        format!(
            "{} {:016x} {:016x}",
            self.label,
            fnv1a(self.input.as_bytes()),
            fnv1a(self.output.as_bytes())
        )
    }
}

/// The tank-local layout, read once at spawn pose.
struct Layout {
    lo: Vec3,
    hi: Vec3,
    /// Damageable boxes by name: (name, local AABB centre).
    boxes: Vec<(String, Vec3)>,
}

/// A rigid pose of the tank root.
#[derive(Clone, Copy)]
struct Pose {
    label: &'static str,
    translation: Vec3,
    rotation: Quat,
}

/// One world-space ray, the strength it is thrown with, and its share of its cone (1 outside the
/// `cone` family).
struct WorldRay {
    label: String,
    origin: Vec3,
    dir: Dir3,
    strength: f32,
    weight: f32,
}

fn axis(index: usize) -> Vec3 {
    Vec3::AXES[index % 3]
}

/// Every ray of one pose. Tank-local points and directions go to world by the pose alone; cone
/// directions are generated in world from the world axis, as the sim generates them.
fn rays(layout: &Layout, pose: Pose) -> Vec<WorldRay> {
    let to_world = |p: Vec3| pose.translation + pose.rotation * p;
    let dir_world = |d: Vec3| Dir3::new(pose.rotation * d).expect("fixture direction is nonzero");
    let center = (layout.lo + layout.hi) * 0.5;
    let half = (layout.hi - layout.lo) * 0.5;
    let spread = ConeShape::shipped()
        .spread(FieldDescriptor::ball(CONE_RATIO))
        .expect("the fixture ratio lies in the ball's domain");
    let weight = 1.0 / spread.directions_count() as f32;
    let mut out = Vec::new();

    // cone: 3 axes × 2 sides × 2 lateral offsets, each a cone pointing inward.
    for a in 0..3 {
        let (b, d) = (axis(a + 1), axis(a + 2));
        for (si, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            for (ki, k) in [-1.0f32, 1.0].into_iter().enumerate() {
                let origin =
                    center + axis(a) * (s * 0.8 * half[a]) + b * (k * 0.35 * half[(a + 1) % 3]);
                let axis_local = -s * axis(a) + 0.2 * k * b + 0.1 * d;
                for (n, (dir, profile)) in spread.directions(dir_world(axis_local)).enumerate() {
                    out.push(WorldRay {
                        label: format!("{}/cone/{a}{si}{ki}/{n:02}", pose.label),
                        origin: to_world(origin),
                        dir,
                        strength: STRENGTH * profile,
                        weight,
                    });
                }
            }
        }
    }

    // lattice: 5 × 5 per face, entering from 1 m outside the bounds.
    for a in 0..3 {
        let (b, d) = ((a + 1) % 3, (a + 2) % 3);
        for (si, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            for i in 0..5 {
                for j in 0..5 {
                    let u = (i as f32 + 0.5) / 5.0 * 2.0 - 1.0;
                    let v = (j as f32 + 0.5) / 5.0 * 2.0 - 1.0;
                    let origin = center
                        + axis(a) * (s * (half[a] + 1.0))
                        + axis(b) * (u * 0.9 * half[b])
                        + axis(d) * (v * 0.9 * half[d]);
                    let dir = -s * axis(a) + 0.07 * axis(b) + 0.04 * axis(d);
                    out.push(WorldRay {
                        label: format!("{}/lattice/{a}{si}/{i}{j}", pose.label),
                        origin: to_world(origin),
                        dir: dir_world(dir),
                        strength: STRENGTH,
                        weight: 1.0,
                    });
                }
            }
        }
    }

    // graze: along each side (x and z faces) and the top, tilted 0.04 into the face.
    for (fi, (a, s)) in [(0usize, -1.0f32), (0, 1.0), (2, -1.0), (2, 1.0), (1, 1.0)]
        .into_iter()
        .enumerate()
    {
        let (b, d) = if a == 1 { (2, 0) } else { (2 - a, 1) };
        for (ri, f) in [0.97f32, 0.93, 0.88, 0.8].into_iter().enumerate() {
            for (wi, w) in [-0.5f32, 0.5].into_iter().enumerate() {
                let origin = center - axis(b) * (half[b] + 1.0)
                    + axis(a) * (s * f * half[a])
                    + axis(d) * (w * half[d]);
                let dir = axis(b) - s * 0.04 * axis(a);
                out.push(WorldRay {
                    label: format!("{}/graze/{fi}/{ri}{wi}", pose.label),
                    origin: to_world(origin),
                    dir: dir_world(dir),
                    strength: STRENGTH,
                    weight: 1.0,
                });
            }
        }
    }

    // box: at each damageable box's centre from two quadrants.
    let quadrants = [Vec3::new(1.0, 0.6, -0.8), Vec3::new(-0.9, 0.5, 0.7)];
    for (name, at) in &layout.boxes {
        for (qi, q) in quadrants.into_iter().enumerate() {
            let from = q.normalize();
            out.push(WorldRay {
                label: format!("{}/box/{name}/{qi}", pose.label),
                origin: to_world(*at + from * (2.0 * half.length() + 1.0)),
                dir: dir_world(-from),
                strength: STRENGTH,
                weight: 1.0,
            });
        }
    }
    out
}

/// The bounds the layout is built from: armour colliders and damageable boxes, at spawn pose.
fn layout(app: &mut App) -> (Layout, Record) {
    let layout = app
        .world_mut()
        .run_system_once(
            |aabbs: Query<(Entity, &ColliderAabb)>,
             volumes: Query<&BallisticVolume>,
             witnesses: Query<(), With<WitnessSurface>>,
             health: Query<&ComponentHealth>,
             names: Query<&Name>,
             parents: Query<&ChildOf>| {
                let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
                let mut boxes: Vec<(String, Vec3, Vec3)> = Vec::new();
                // The query's own order is archetype order; sorting by name below makes the
                // layout independent of it, and min/max is order-free.
                for (entity, aabb) in &aabbs {
                    if !witnesses.contains(entity)
                        && hit_ancestor(entity, &volumes, &parents).is_none()
                    {
                        continue;
                    }
                    lo = lo.min(aabb.min - PROBE_TANK_AT);
                    hi = hi.max(aabb.max - PROBE_TANK_AT);
                    if let Some((owner, _)) = hit_ancestor(entity, &health, &parents) {
                        let name = names
                            .get(owner)
                            .map(|n| n.as_str().to_owned())
                            .unwrap_or_default();
                        let (min, max) = (aabb.min - PROBE_TANK_AT, aabb.max - PROBE_TANK_AT);
                        match boxes.iter_mut().find(|(n, ..)| *n == name) {
                            Some((_, bmin, bmax)) => {
                                *bmin = bmin.min(min);
                                *bmax = bmax.max(max);
                            }
                            None => boxes.push((name, min, max)),
                        }
                    }
                }
                boxes.sort_by(|a, b| a.0.cmp(&b.0));
                Layout {
                    lo,
                    hi,
                    boxes: boxes
                        .into_iter()
                        .map(|(name, min, max)| (name, (min + max) * 0.5))
                        .collect(),
                }
            },
        )
        .expect("the layout system runs");
    assert!(
        layout.lo.cmple(layout.hi).all() && !layout.boxes.is_empty(),
        "the fixture tank has armour bounds and damageable boxes"
    );
    let mut input = String::new();
    bits3(&mut input, layout.lo);
    input.push(' ');
    bits3(&mut input, layout.hi);
    for (name, at) in &layout.boxes {
        let _ = write!(input, " {name}@");
        bits3(&mut input, *at);
    }
    let record = Record {
        label: "frame".into(),
        input,
        output: String::new(),
    };
    (layout, record)
}

/// Stable identity of a struck collider: the nearest named ancestor, then the child ordinal at each
/// unnamed step down to the collider. Entity ids are not stable across builds; spawn order is.
fn stable_id(
    entity: Entity,
    names: &Query<&Name>,
    parents: &Query<&ChildOf>,
    children: &Query<&Children>,
) -> String {
    let mut ordinals = Vec::new();
    let mut probe = entity;
    let root = loop {
        if let Ok(name) = names.get(probe) {
            break name.as_str().to_owned();
        }
        let Ok(parent) = parents.get(probe) else {
            break "?".to_owned();
        };
        let ordinal = children
            .get(parent.parent())
            .ok()
            .and_then(|c| c.iter().position(|child| child == probe))
            .map_or(-1, |o| o as i64);
        ordinals.push(ordinal);
        probe = parent.parent();
    };
    let mut id = root;
    for ordinal in ordinals.iter().rev() {
        let _ = write!(id, "/{ordinal}");
    }
    id
}

/// Cast every ray of one pose through both paths and return its records, the pose's dose last.
fn cast_pose(app: &mut App, layout: &Layout, pose: Pose) -> Vec<Record> {
    let tank = app.world().resource::<ProbeTank>().0;
    app.world_mut().entity_mut(tank).insert((
        Position(pose.translation),
        Rotation(pose.rotation),
        Transform::from_translation(pose.translation).with_rotation(pose.rotation),
    ));
    // Physics has to step for the colliders to follow the moved root.
    for _ in 0..4 {
        app.update();
    }
    let rays = rays(layout, pose);
    let box_names: Vec<String> = layout.boxes.iter().map(|(n, _)| n.clone()).collect();
    let label = pose.label;
    app.world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>,
                  children: Query<&Children>,
                  names: Query<&Name>,
                  labelled: Query<(Entity, &Name), With<ComponentHealth>>,
                  health: Query<&ComponentHealth>| {
                let context = resolve::ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: true,
                    laws: walk::WalkLaws::default(),
                };
                let parents = &world.parents;
                let within = |collider: Entity| {
                    world.volume_of(collider).is_some_and(|node| {
                        world.owners.get(node).ok().map(VolumeOf::tank) == Some(tank)
                    })
                };
                let boxes: Vec<(String, Entity)> = box_names
                    .iter()
                    .map(|name| {
                        let entity = labelled
                            .iter()
                            .find(|(_, n)| n.as_str() == name)
                            .map(|(e, _)| e)
                            .expect("every laid-out box still exists");
                        (name.clone(), entity)
                    })
                    .collect();
                let mut dose = vec![0.0f32; boxes.len()];
                let mut records = Vec::with_capacity(rays.len() + 1);
                for ray in &rays {
                    let mut input = String::new();
                    bits3(&mut input, ray.origin);
                    input.push(' ');
                    bits3(&mut input, Vec3::from(ray.dir));
                    input.push(' ');
                    bits(&mut input, ray.strength);
                    input.push(' ');
                    bits(&mut input, ray.weight);

                    // 1. The ordered presence the direction is walked over.
                    let mut output = String::new();
                    match resolve::field_presence(&context, &within, ray.origin, ray.dir) {
                        Ok(None) => output.push_str("clear"),
                        Ok(Some((start, walked))) => {
                            bits64(&mut output, start);
                            for layer in &walked.layers {
                                output.push(';');
                                bits64(&mut output, layer.start);
                                output.push(',');
                                bits64(&mut output, layer.end);
                                for presence in &layer.present {
                                    let _ = write!(
                                        output,
                                        " {}:",
                                        stable_id(presence.volume, &names, parents, &children)
                                    );
                                    bits(&mut output, presence.material.factor);
                                }
                            }
                        }
                        Err(error) => {
                            let _ = write!(output, "error {error:?}");
                        }
                    }

                    // 2. The sim's field walk and the doses it addresses.
                    let cast = resolve::field_direction(
                        &context,
                        &within,
                        &|volume| health.contains(volume),
                        &[],
                        ray.origin,
                        ray.dir,
                        ray.strength,
                        ray.weight,
                    )
                    .unwrap_or_else(|error| panic!("{}: the walk failed: {error:?}", ray.label));
                    output.push_str(" end ");
                    bits3(&mut output, cast.end);
                    let _ = write!(output, " deposited {}", !cast.doses.is_empty());
                    for hit in &cast.doses {
                        let Some(i) = boxes.iter().position(|(_, entity)| *entity == hit.address)
                        else {
                            continue;
                        };
                        let _ = write!(output, " {}=", boxes[i].0);
                        bits(&mut output, hit.amount);
                        dose[i] += hit.amount;
                    }

                    // 3. The line through every witness along the ray.
                    let frame = walk::DiscFrame::anchored(Vec3::from(ray.dir))
                        .expect("a ray has a unit direction");
                    let offsets = resolve::layout(&frame, resolve::disc_radius(LINE_CALIBER));
                    let leg = resolve::Leg::new(ray.origin, ray.dir, LINE_REACH, &offsets);
                    match super::wound::line(&context, &within, &[leg], LINE_WOUND) {
                        Ok((crossed, _)) => {
                            for crossing in &crossed {
                                let _ = write!(
                                    output,
                                    " line {}=",
                                    stable_id(crossing.volume, &names, parents, &children)
                                );
                                bits(&mut output, crossing.chord);
                                output.push(',');
                                bits(&mut output, crossing.cost);
                            }
                        }
                        Err(error) => {
                            let _ = write!(output, " line error {error:?}");
                        }
                    }
                    records.push(Record {
                        label: ray.label.clone(),
                        input,
                        output,
                    });
                }
                assert!(
                    dose.iter().any(|dose| *dose > 0.0),
                    "pose {label}: no ray dosed a box — the struck-tank filter excluded the tank"
                );
                let mut output = String::new();
                for ((name, _), dose) in boxes.iter().zip(&dose) {
                    let _ = write!(output, "{name}=");
                    bits(&mut output, *dose);
                    output.push(' ');
                }
                records.push(Record {
                    label: format!("{label}/dose"),
                    input: String::new(),
                    output,
                });
                records
            },
        )
        .expect("the cast system runs")
}

/// The probe world plus the REAL march, stepped at 16 ms. Spent shells are retained, so a round
/// that stops inside the tick it entered still leaves its marks to read.
fn shot_world() -> App {
    let mut app = probe_world().expect("the bound tank builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )))
    .insert_resource(RetainSpentShells(true))
    .insert_resource(MgShortCircuit(false))
    .add_systems(Update, integrate_projectiles);
    app
}

/// Spawn one keyed round of `round` (`(caliber, mass, nose_factor, speed, wound, drag_k)`) at a
/// tank-local origin along a tank-local direction, ready for the march. The tank spawns unrotated at
/// [`PROBE_TANK_AT`], so tank-local to world is that translation alone.
fn spawn_shot_round(
    app: &mut App,
    local_origin: Vec3,
    local_direction: Vec3,
    round: (f32, f32, f32, f32, f32, f32),
) -> Entity {
    let (caliber, mass, nose_factor, speed, wound, drag_k) = round;
    let origin = PROBE_TANK_AT + local_origin;
    let direction = local_direction.normalize();
    let body = RoundBody {
        mass,
        caliber,
        nose_factor,
    };
    app.world_mut()
        .spawn((
            Projectile {
                velocity: direction * speed,
                round: bound(body),
                caliber,
                mass,
                wound,
                drag_k,
                fuze: burst::Fuze::Inert,
                disc: walk::DiscFrame::anchored(direction).expect("a shot fires along a real axis"),
                continuation: None,
            },
            DamageReport::default(),
            TerminalReport::default(),
            ShellPath {
                points: vec![origin],
                segment_starts: Vec::new(),
            },
            PenetrationMarks::default(),
            SpallMarks::default(),
            ShellReadout {
                speed,
                capability: capability(body, speed),
            },
            Shot(ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick: 100,
            }),
            Transform::from_translation(origin).looking_to(direction, Vec3::Y),
        ))
        .id()
}

/// The round the shots fire: a key in the round registry, fixture data. The shot pin digests one
/// solid shot's cones; a round off the selector stays here as the fixture.
const FIXTURE_ROUND: &str = "88mm_AP";

/// `(caliber, mass, nose_factor, speed, wound, drag_k)` of [`FIXTURE_ROUND`]. A full-calibre inert round:
/// the casts' input digest carries one body and no burst, so a sub-calibre or burst round is
/// refused here rather than pinned as its whole body.
fn main_gun_round() -> (f32, f32, f32, f32, f32, f32) {
    let round = crate::guns::GunRegistry::shipped()
        .round_by_key(FIXTURE_ROUND)
        .unwrap_or_else(|err| panic!("the field casts' round: {err}"));
    assert_eq!(
        round.penetrator, round.body,
        "the field casts pin a full-calibre round"
    );
    assert_eq!(round.burst, None, "the field casts pin an inert round");
    (
        round.body.caliber,
        round.body.mass,
        round.body.nose_factor,
        round.muzzle_speed,
        round.wound,
        round.drag_k,
    )
}

/// One fixture shot through the real march, in a fresh world at spawn pose ([`SHOTS`]).
fn shot(label: &str, local_origin: Vec3, local_direction: Vec3) -> Record {
    let round = main_gun_round();
    let mut app = shot_world();
    let shell = spawn_shot_round(&mut app, local_origin, local_direction, round);
    // Eight ticks at 16 ms: the whole tank and the run-out past it, for a main-gun round.
    for _ in 0..8 {
        app.update();
    }
    let mut input = String::new();
    bits3(&mut input, local_origin);
    input.push(' ');
    bits3(&mut input, local_direction);
    for value in [round.0, round.1, round.2, round.3, round.4, round.5] {
        input.push(' ');
        bits(&mut input, value);
    }
    let mut output = String::new();
    let world = app.world();
    let marks = world
        .get::<PenetrationMarks>(shell)
        .expect("the shot's shell is retained");
    for event in &marks.events {
        output.push_str("event ");
        bits3(&mut output, event.entry);
        output.push(' ');
        bits3(&mut output, event.exit);
        let _ = write!(output, " {};", event.overmatched);
    }
    for mark in &marks.log {
        if let ArmourMark::Wound { volumes, .. } = mark {
            for crossing in volumes {
                let name = world
                    .get::<Name>(crossing.volume)
                    .map_or("?", |name| name.as_str());
                let _ = write!(output, "wound {name}=");
                bits(&mut output, crossing.chord);
                output.push(',');
                bits(&mut output, crossing.cost);
                output.push(';');
            }
        }
    }
    for cone in &world
        .get::<SpallMarks>(shell)
        .expect("the shot's shell is retained")
        .cones
    {
        output.push_str("cone ");
        bits3(&mut output, cone.origin);
        output.push(' ');
        bits3(&mut output, Vec3::from(cone.axis));
        output.push(' ');
        let _ = write!(output, "{:?} ", cone.field.law);
        bits(&mut output, cone.field.ratio);
        output.push(' ');
        bits(&mut output, cone.field.half_angle());
        for value in [
            cone.spend,
            cone.energy,
            cone.entry_speed,
            cone.exit_speed,
            cone.strength,
        ] {
            output.push(' ');
            bits(&mut output, value);
        }
        for ray in &cone.rays {
            output.push(' ');
            bits3(&mut output, ray.end);
            output.push(if ray.doses.is_empty() { '-' } else { '+' });
        }
        output.push(';');
    }
    let mut health: Vec<(String, u32)> = app
        .world_mut()
        .query::<(&Name, &ComponentHealth)>()
        .iter(app.world())
        .map(|(name, hp)| (name.as_str().to_owned(), hp.held()))
        .collect();
    health.sort();
    for (name, held) in health {
        let _ = write!(output, " {name}={held}");
    }
    Record {
        label: format!("shot/{label}"),
        input,
        output,
    }
}

/// Every record, in a fixed order.
fn measure() -> Vec<Record> {
    let mut app = probe_world().expect("the bound tank builds");
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_millis(16),
    ));
    let (layout, frame) = layout(&mut app);
    let tilted = Pose {
        label: "B",
        translation: PROBE_TANK_AT + Vec3::new(-3.25, 0.5, 2.75),
        rotation: Quat::from_xyzw(0.02, 0.31, -0.015, 0.95).normalize(),
    };
    let spawn = Pose {
        label: "A",
        translation: PROBE_TANK_AT,
        rotation: Quat::IDENTITY,
    };
    let mut records = vec![frame];
    for pose in [spawn, tilted] {
        records.extend(cast_pose(&mut app, &layout, pose));
    }
    for (label, origin, direction) in SHOTS {
        records.push(shot(label, origin, direction));
    }
    records
}

/// Parse one pinned line, `label input-digest output-digest`; the label may hold spaces.
fn parse_pinned(line: &str) -> Option<(&str, u64, u64)> {
    let mut parts = line.rsplitn(3, ' ');
    let output = u64::from_str_radix(parts.next()?, 16).ok()?;
    let input = u64::from_str_radix(parts.next()?, 16).ok()?;
    Some((parts.next()?, input, output))
}

/// The spall-cone casts digest to the pinned macOS arm64 value, bit for bit.
#[test]
fn the_field_casts_digest_to_the_pinned_value() {
    let records = measure();
    let lines: Vec<String> = records.iter().map(Record::line).collect();
    let mut joined = lines.join("\n");
    joined.push('\n');
    let total = fnv1a(joined.as_bytes());

    if let Ok(prefix) = std::env::var("OVERMATCH_FIELD_CASTS_SHOW") {
        for record in records.iter().filter(|r| r.label.starts_with(&prefix)) {
            println!(
                "{}\n  input:  {}\n  output: {}",
                record.label, record.input, record.output
            );
        }
    }
    if std::env::var_os("OVERMATCH_FIELD_CASTS_REPIN").is_some() {
        std::fs::write(PINNED_PATH, &joined).expect("the pinned file is writable");
        println!(
            "re-pinned {} records; PINNED_TOTAL = 0x{total:016x}",
            lines.len()
        );
        return;
    }

    let target = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let pinned: Vec<&str> = PINNED.lines().collect();
    let first_difference = lines.iter().enumerate().find_map(|(i, line)| {
        let Some(expected) = pinned.get(i) else {
            return Some((i, None));
        };
        (expected.trim_end() != line).then_some((i, Some(*expected)))
    });
    if let Some((i, expected)) = first_difference {
        let record = &records[i];
        let what = match expected.and_then(parse_pinned) {
            None => "the pinned file has no such record",
            Some((label, ..)) if label != record.label => {
                "the record LABEL differs: the ray layout changed"
            }
            Some((_, input, _)) if input != fnv1a(record.input.as_bytes()) => {
                "the ray INPUT differs: the layout bounds or the cone generator moved"
            }
            Some(_) => "the cast OUTPUT differs: the engine casts or the field cast moved",
        };
        panic!(
            "field casts on {target} differ from the macOS arm64 pin at record {i} of {}: {what}\n\
             expected: {}\n\
             actual:   {}\n\
             actual input:  {}\n\
             actual output: {}\n\
             reference text: run on macOS arm64 with OVERMATCH_FIELD_CASTS_SHOW='{}' and --nocapture",
            lines.len(),
            expected.unwrap_or("<none>"),
            lines[i],
            record.input,
            record.output,
            record.label,
        );
    }
    assert_eq!(
        pinned.len(),
        lines.len(),
        "field casts on {target}: the pinned file has {} records, this run {}",
        pinned.len(),
        lines.len()
    );
    assert_eq!(
        total, PINNED_TOTAL,
        "field casts on {target}: every record matches the pinned file but the total \
         0x{total:016x} is not PINNED_TOTAL 0x{PINNED_TOTAL:016x}: the file was re-pinned \
         without the constant"
    );
}
