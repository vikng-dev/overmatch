//! Cross-platform reproducibility of the spall-cloud casts.
//!
//! A client may re-cast a spall cloud from a shot record's parameters only if the casts the sim
//! runs for it (physics-engine ray casts against the struck tank's armour and boxes) come out
//! bit-identical on every shipped target: Windows x86_64, Linux x86_64, macOS arm64. This test
//! measures that and decides nothing; the decision and its two outcomes are in
//! `.agents/scratch/damage-cam-2026-10-08/product/in-battle/owner-direction-2026-10-10.md`.
//!
//! # What runs
//!
//! The bound fixture tank from [`probe_world`], `RigidBody::Static`, at two fixed poses: spawn
//! pose, and a tilted, yawed pose whose quaternion is literal components normalised by `sqrt`
//! (no trig in the pose). The ray set is laid out in tank-local metres from the tank's armour
//! bounds and each damageable box's bounds, read at spawn pose:
//!
//! - `cloud`: cones from interior origins through the sim's own [`spall_directions`];
//! - `lattice`: a grid of slightly off-axis rays into each face of the bounds (crosses seams);
//! - `graze`: rays skimming the four sides and the top at shallow inward angles;
//! - `box`: rays aimed at each damageable box's centre from two quadrants.
//!
//! Each ray goes through two paths:
//!
//! 1. the engine walk: the same `cast_ray` (solid) and same-entity `cast_ray_predicate` (hollow)
//!    pair [`cast_spall_fragment`] issues, with no penetration budget, recording every entry and
//!    exit as (struck collider, distance, normal);
//! 2. [`cast_spall_fragment`] itself, depositing into every box after all boxes are reset to
//!    [`BOX_RESET_HP`]; the remaining HP of each touched box and the fragment end are recorded, and
//!    each box's dose (`reset - remaining`) accumulates in f32 in ray order.
//!
//! # Digest
//!
//! Each ray is written as text with every f32 as its raw IEEE-754 bits in hex, and the text is
//! hashed with 64-bit FNV-1a over its UTF-8 bytes ([`fnv1a`]). The INPUT (origin, direction,
//! budget) and the OUTPUT (hits, fragment, HP) are digested separately: cone directions come from
//! `f32::sin`/`cos`, which call the platform's libm, so a red run must say whether the ray
//! generator moved or the casts did. The bounds the layout is built from are their own `frame`
//! record, and each pose's per-box dose is a `dose` record.
//!
//! `cloud_casts.pinned` holds one line per record (`label input-digest output-digest`); the digest
//! of those lines is [`PINNED_TOTAL`]. Both are MEASURED on macOS arm64.
//!
//! # Re-pinning and reading a red run
//!
//! A red run prints the first differing record and its full text on this target. Running the
//! test on macOS arm64 with `OVERMATCH_CLOUD_CASTS_SHOW=<label prefix>` and `--nocapture` prints
//! the reference text of the records it names. `OVERMATCH_CLOUD_CASTS_REPIN=1` rewrites the pinned file and
//! prints the new total for [`PINNED_TOTAL`]; re-pin only on macOS arm64, and only for a change
//! meant to move the casts (geometry, the cast path, an engine bump).

use std::fmt::Write as _;

use avian3d::prelude::{ColliderAabb, Position, Rotation, SpatialQuery, SpatialQueryFilter};
use bevy::ecs::system::RunSystemOnce;

use super::fuzz::{PROBE_TANK_AT, ProbeTank, probe_world};
use super::*;

/// The digest of every line of `cloud_casts.pinned`, MEASURED on macOS arm64 (debug and release
/// agree).
const PINNED_TOTAL: u64 = 0x6fdc_9425_1162_c15d;

const PINNED: &str = include_str!("cloud_casts.pinned");

const PINNED_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ballistics/cloud_casts.pinned"
);

/// HP every box is reset to before each ray. Fixture value above the most one fragment can deposit
/// in one crossing (`FRAG_PEN_MAX · FRAG_DMG_PER_MM`), so a single hit's dose is not clamped away.
const BOX_RESET_HP: f32 = 4.0;

/// Fixture cone half-angle (rad) and reach (m) of the `cloud` family.
const CLOUD_HALF_ANGLE: f32 = 0.6;
const CLOUD_RAYS_PER_CONE: usize = 24;
const CLOUD_RANGE: f32 = 6.0;

/// The nudge and exit-probe reach [`cast_spall_fragment`] uses, mirrored by the engine walk.
const EPS: f32 = 1.0e-3;
const PROBE: f32 = 50.0;
/// Upper bound on entries one engine walk records; a ray reaching it is recorded as truncated.
const MAX_WALK_HITS: usize = 64;

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

/// One world-space ray and the budget its fragment is thrown with.
struct WorldRay {
    label: String,
    origin: Vec3,
    dir: Dir3,
    pen: f32,
    range: f32,
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
    let across = 4.0 * half.length() + 2.0;
    let mut out = Vec::new();

    // cloud: 3 axes × 2 sides × 2 lateral offsets, each a cone pointing inward.
    for a in 0..3 {
        let (b, d) = (axis(a + 1), axis(a + 2));
        for (si, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            for (ki, k) in [-1.0f32, 1.0].into_iter().enumerate() {
                let origin =
                    center + axis(a) * (s * 0.8 * half[a]) + b * (k * 0.35 * half[(a + 1) % 3]);
                let axis_local = -s * axis(a) + 0.2 * k * b + 0.1 * d;
                for (n, (dir, t)) in
                    spall_directions(dir_world(axis_local), CLOUD_HALF_ANGLE, CLOUD_RAYS_PER_CONE)
                        .into_iter()
                        .enumerate()
                {
                    out.push(WorldRay {
                        label: format!("{}/cloud/{a}{si}{ki}/{n:02}", pose.label),
                        origin: to_world(origin),
                        dir,
                        pen: FRAG_PEN_MAX * (1.0 - t),
                        range: CLOUD_RANGE,
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
                        pen: FRAG_PEN_MAX,
                        range: across,
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
                    pen: FRAG_PEN_MAX,
                    range: across,
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
                pen: FRAG_PEN_MAX,
                range: across,
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
             health: Query<&ComponentHealth>,
             names: Query<&Name>,
             parents: Query<&ChildOf>| {
                let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
                let mut boxes: Vec<(String, Vec3, Vec3)> = Vec::new();
                // The query's own order is archetype order; sorting by name below makes the
                // layout independent of it, and min/max is order-free.
                for (entity, aabb) in &aabbs {
                    if hit_ancestor(entity, &volumes, &parents).is_none() {
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
            move |spatial: SpatialQuery,
                  volumes: Query<&BallisticVolume>,
                  parents: Query<&ChildOf>,
                  children: Query<&Children>,
                  names: Query<&Name>,
                  labelled: Query<(Entity, &Name), With<ComponentHealth>>,
                  mut health: Query<&mut ComponentHealth>| {
                let armor = SpatialQueryFilter::from_mask(Layer::Armor);
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
                    bits(&mut input, ray.pen);
                    input.push(' ');
                    bits(&mut input, ray.range);

                    // 1. The engine walk: every entry and exit along the ray.
                    let mut output = String::new();
                    let dir = Vec3::from(ray.dir);
                    let (mut pos, mut range, mut entries) = (ray.origin, ray.range, 0);
                    while range > EPS {
                        if entries == MAX_WALK_HITS {
                            output.push_str("truncated;");
                            break;
                        }
                        let Some(hit) = spatial.cast_ray(pos, ray.dir, range, true, &armor) else {
                            break;
                        };
                        entries += 1;
                        let _ = write!(
                            output,
                            "{}:",
                            stable_id(hit.entity, &names, &parents, &children)
                        );
                        bits(&mut output, hit.distance);
                        output.push(':');
                        bits3(&mut output, hit.normal);
                        let at = pos + dir * hit.distance;
                        let exit = spatial.cast_ray_predicate(
                            at + dir * EPS,
                            ray.dir,
                            PROBE,
                            false,
                            &armor,
                            &|e| e == hit.entity,
                        );
                        let span = match exit {
                            Some(exit) => {
                                output.push('>');
                                bits(&mut output, exit.distance);
                                output.push(':');
                                bits3(&mut output, exit.normal);
                                EPS + exit.distance
                            }
                            None => {
                                output.push_str(">none");
                                0.0
                            }
                        };
                        output.push(';');
                        pos = at + dir * (span + EPS);
                        range -= hit.distance + span + EPS;
                    }

                    // 2. The sim's fragment cast, into freshly reset boxes.
                    for (_, entity) in &boxes {
                        health
                            .get_mut(*entity)
                            .expect("a laid-out box has health")
                            .current = BOX_RESET_HP;
                    }
                    let (fragment, dealt) = cast_spall_fragment(
                        ray.origin,
                        ray.dir,
                        ray.pen,
                        ray.range,
                        &spatial,
                        &volumes,
                        &parents,
                        &mut health,
                        &armor,
                        true,
                    );
                    output.push_str(" end ");
                    bits3(&mut output, fragment.end);
                    let _ = write!(output, " deposited {} dealt ", fragment.deposited);
                    bits(&mut output, dealt);
                    for (i, (name, entity)) in boxes.iter().enumerate() {
                        let left = health
                            .get(*entity)
                            .expect("a laid-out box has health")
                            .current;
                        if left != BOX_RESET_HP {
                            let _ = write!(output, " {name}=");
                            bits(&mut output, left);
                            dose[i] += BOX_RESET_HP - left;
                        }
                    }
                    records.push(Record {
                        label: ray.label.clone(),
                        input,
                        output,
                    });
                }
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
    records
}

/// Parse one pinned line, `label input-digest output-digest`; the label may hold spaces.
fn parse_pinned(line: &str) -> Option<(&str, u64, u64)> {
    let mut parts = line.rsplitn(3, ' ');
    let output = u64::from_str_radix(parts.next()?, 16).ok()?;
    let input = u64::from_str_radix(parts.next()?, 16).ok()?;
    Some((parts.next()?, input, output))
}

/// The spall-cloud casts digest to the pinned macOS arm64 value, bit for bit.
#[test]
fn the_cloud_casts_digest_to_the_pinned_value() {
    let records = measure();
    let lines: Vec<String> = records.iter().map(Record::line).collect();
    let mut joined = lines.join("\n");
    joined.push('\n');
    let total = fnv1a(joined.as_bytes());

    if let Ok(prefix) = std::env::var("OVERMATCH_CLOUD_CASTS_SHOW") {
        for record in records.iter().filter(|r| r.label.starts_with(&prefix)) {
            println!(
                "{}\n  input:  {}\n  output: {}",
                record.label, record.input, record.output
            );
        }
    }
    if std::env::var_os("OVERMATCH_CLOUD_CASTS_REPIN").is_some() {
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
                "the ray INPUT differs: the layout bounds or the cone generator's sin/cos moved"
            }
            Some(_) => "the cast OUTPUT differs: the engine casts or the fragment cast moved",
        };
        panic!(
            "cloud casts on {target} differ from the macOS arm64 pin at record {i} of {}: {what}\n\
             expected: {}\n\
             actual:   {}\n\
             actual input:  {}\n\
             actual output: {}\n\
             reference text: run on macOS arm64 with OVERMATCH_CLOUD_CASTS_SHOW='{}' and --nocapture",
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
        "cloud casts on {target}: the pinned file has {} records, this run {}",
        pinned.len(),
        lines.len()
    );
    assert_eq!(
        total, PINNED_TOTAL,
        "cloud casts on {target}: every record matches the pinned file but the total \
         0x{total:016x} is not PINNED_TOTAL 0x{PINNED_TOTAL:016x}: the file was re-pinned \
         without the constant"
    );
}
