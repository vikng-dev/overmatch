//! Placement clearance, in the game's environment destruction model: no two placed instances'
//! solids interpenetrate. Game state only.
//!
//! Two constructions placed inside each other stand, but when they break their pieces are born
//! inside each other and the server's rubble never comes to rest. So such a placement is refused
//! before play ([`check`]) at the first point where the terrain height and the solids meet: the
//! scatter's spawn ([`super::spawn`]; a reconstruction re-places the same bytes on the same grid),
//! and CI on the shipped map (`map::tests::shipped_map_ships_a_valid_manifest`).
//!
//! The solids are the ones the scatter spawns: each construction element's hull at its instance's
//! ground pose, and each graybox block's box. Instance bounds first, then solid bounds, then the
//! physics engine's contact query on the pairs left.

use std::collections::BTreeMap;

use avian3d::collision::collider::contact_query::contact;
use avian3d::prelude::{Collider, ColliderAabb, SimpleCollider};
use bevy::prelude::*;

use super::{Placement, place, resolve};
use crate::map::{MapManifest, Prototype};
use crate::terrain_grid::HeightGrid;

/// Resolution: the deepest overlap admitted as touching. Solids authored face to face meet at
/// zero depth, which f32 placement at map scale (ulp 6.1e-5 m at 600 m) and the [`NUDGE`] lift
/// above zero.
const TOUCH_M: f32 = 1.0e-3;

/// Resolution: headings tried for a refusal's clearing move, evenly spaced; the repair hint's
/// angular step.
const HEADINGS: u16 = 72;

/// Resolution: the bisection step of a clearing move's length, under half the printed millimetre.
const BISECT_M: f32 = 1.0e-4;

/// Resolution: the second pose a depth is measured at. The contact query's depth search (parry
/// 0.27's EPA) can only underestimate, and from a degenerate start, the origin on its first
/// simplex, it reports too little: MEASURED zero for an exact duplicate hull, and 0.275 m for
/// hulls offset 0.4 m deep along a shared axis. A nudge off every axis breaks that symmetry, so the
/// deeper of the two measures stands, off by no more than the nudge's length (under half
/// [`TOUCH_M`]); each component is at least 2 f32 ulps at map scale.
const NUDGE: Vec3 = Vec3::new(2.0e-4, 3.0e-4, 1.5e-4);

/// One solid, posed where the scatter spawns it.
struct Solid {
    collider: Collider,
    position: Vec3,
    rotation: Quat,
    bounds: ColliderAabb,
}

impl Solid {
    fn new(collider: Collider, position: Vec3, rotation: Quat) -> Self {
        let bounds = collider.aabb(position, rotation);
        Self {
            collider,
            position,
            rotation,
            bounds,
        }
    }
}

/// One placed instance's solids and their joint bounds.
struct Solids<'a> {
    id: &'a str,
    solids: Vec<Solid>,
    bounds: ColliderAabb,
}

/// Two instances whose solids interpenetrate, `a` before `b` in id order.
#[derive(Debug)]
struct Interpenetration {
    a: String,
    b: String,
    /// The deepest overlap of a solid of one in a solid of the other.
    depth_m: f32,
    /// The shortest horizontal move of `b`, at its present height, after which the two instances'
    /// solids at most touch; XZ metres.
    clear: Vec2,
}

/// Refuse a placement in which two instances' solids interpenetrate (module doc), naming the map
/// file, both instances, the depth and the shortest horizontal move that clears each pair.
pub(crate) fn check(manifest: &MapManifest, grid: &HeightGrid) {
    let found = interpenetrations(manifest, grid);
    assert!(
        found.is_empty(),
        "map: {} field instances places solids inside each other: {} — move each pair apart until \
         their solids at most touch ({TOUCH_M} m), then check again: a moved instance stands at the \
         terrain height under its new XZ",
        manifest.level_path().display(),
        found
            .iter()
            .map(|pair| format!(
                "{a} and {b} by {depth:.3} m (move {b} by at least ({x:.3}, {z:.3}) m in XZ, or {a} \
                 by the opposite)",
                a = pair.a,
                b = pair.b,
                depth = pair.depth_m,
                x = pair.clear.x,
                z = pair.clear.y,
            ))
            .collect::<Vec<_>>()
            .join("; "),
    );
}

/// Every interpenetrating pair of the placement `manifest` stands on `grid`, in id order.
fn interpenetrations(manifest: &MapManifest, grid: &HeightGrid) -> Vec<Interpenetration> {
    let hulls: BTreeMap<&str, Vec<Collider>> = manifest
        .prototypes
        .values()
        .filter_map(|prototype| match prototype {
            Prototype::Construction { asset } => Some(asset.as_str()),
            Prototype::Block { .. } => None,
        })
        .map(|asset| {
            let kind = manifest
                .constructions
                .get(asset)
                .expect("map::load loaded every construction asset the library names");
            let hulls = (0..kind.asset.elements.len())
                .map(|k| crate::construction::element_collider(kind, k))
                .collect();
            (asset, hulls)
        })
        .collect();
    let placements = place(&resolve(&manifest.instances, &manifest.prototypes), grid);
    let mut placed: Vec<Solids> = placements
        .iter()
        .map(|placement| solids(placement, &hulls))
        .collect();
    placed.sort_by(|a, b| a.bounds.min.x.total_cmp(&b.bounds.min.x));
    let mut found = Vec::new();
    for (k, first) in placed.iter().enumerate() {
        for second in placed[k + 1..]
            .iter()
            .take_while(|second| second.bounds.min.x <= first.bounds.max.x)
        {
            if !first.bounds.intersects(&second.bounds) {
                continue;
            }
            let (a, b) = if first.id < second.id {
                (first, second)
            } else {
                (second, first)
            };
            let depth_m = pairs(a, b, Vec3::ZERO)
                .map(|(x, y)| depth(x, y, Vec3::ZERO))
                .fold(0.0, f32::max);
            if depth_m > TOUCH_M {
                found.push(Interpenetration {
                    a: a.id.to_owned(),
                    b: b.id.to_owned(),
                    depth_m,
                    clear: clearing_move(a, b),
                });
            }
        }
    }
    found.sort_by(|x, y| (&x.a, &x.b).cmp(&(&y.a, &y.b)));
    found
}

/// `placement`'s solids: a construction's element hulls at its ground pose, a block's box.
fn solids<'a>(placement: &'a Placement<'_>, hulls: &BTreeMap<&str, Vec<Collider>>) -> Solids<'a> {
    let solids: Vec<Solid> = match placement.shape {
        Prototype::Construction { asset } => hulls[asset.as_str()]
            .iter()
            .map(|hull| {
                Solid::new(
                    hull.clone(),
                    placement.pose.translation.into(),
                    placement.pose.rotation,
                )
            })
            .collect(),
        Prototype::Block { .. } => {
            let block = placement
                .shape
                .block(placement.pose)
                .expect("a block prototype supplies its unit-cube transform");
            vec![Solid::new(
                Collider::cuboid(block.scale.x, block.scale.y, block.scale.z),
                block.translation,
                block.rotation,
            )]
        }
    };
    let bounds = solids.iter().fold(ColliderAabb::INVALID, |bounds, solid| {
        bounds.merged(solid.bounds)
    });
    Solids {
        id: &placement.identity.id,
        solids,
        bounds,
    }
}

/// The pairs of a solid of `a` and a solid of `b` moved by `offset` whose bounds meet.
fn pairs<'s>(
    a: &'s Solids,
    b: &'s Solids,
    offset: Vec3,
) -> impl Iterator<Item = (&'s Solid, &'s Solid)> {
    a.solids.iter().flat_map(move |x| {
        b.solids
            .iter()
            .filter(move |y| {
                x.bounds.intersects(&ColliderAabb::from_min_max(
                    y.bounds.min + offset,
                    y.bounds.max + offset,
                ))
            })
            .map(move |y| (x, y))
    })
}

/// How deep `b`, moved by `offset`, overlaps `a`: the deeper of the measures at two poses
/// [`NUDGE`] apart, each zero when the contact query finds the solids apart.
fn depth(a: &Solid, b: &Solid, offset: Vec3) -> f32 {
    let at = |position: Vec3| {
        contact(
            &a.collider,
            a.position,
            a.rotation,
            &b.collider,
            position,
            b.rotation,
            0.0,
        )
        .expect("the contact query supports convex hulls and boxes")
        .map_or(0.0, |contact| contact.penetration)
    };
    let position = b.position + offset;
    at(position).max(at(position + NUDGE))
}

/// [`Interpenetration::clear`], over [`HEADINGS`] headings.
fn clearing_move(a: &Solids, b: &Solids) -> Vec2 {
    let mut best: Option<(f32, Vec2)> = None;
    for k in 0..HEADINGS {
        let heading = Vec2::from_angle(f32::from(k) * std::f32::consts::TAU / f32::from(HEADINGS));
        let shortest = best.map_or(f32::INFINITY, |(distance, _)| distance);
        if let Some(distance) = clearing_distance(a, b, heading, shortest) {
            best = Some((distance, heading * distance));
        }
    }
    best.expect("a move past both instances' bounds clears them on any heading")
        .1
}

/// How far `b` moves along `heading` before no pair of solids overlaps; `None` when that is no
/// shorter than `shortest`.
///
/// Along a line, the moves at which two convex solids overlap form one interval: the overlap is
/// the signed distance of the move to their Minkowski difference, a convex function of it. So
/// passing a pair's interval end retires that pair, and the walk ends within one pass per pair.
fn clearing_distance(a: &Solids, b: &Solids, heading: Vec2, shortest: f32) -> Option<f32> {
    let along = Vec3::new(heading.x, 0.0, heading.y);
    let mut distance = 0.0;
    loop {
        let mut beyond = None;
        for (x, y) in pairs(a, b, along * distance) {
            if depth(x, y, along * distance) > 0.0 {
                let end = interval_end(x, y, along, distance, shortest)?;
                beyond = Some(beyond.map_or(end, |far: f32| far.max(end)));
            }
        }
        match beyond {
            None => return (distance < shortest).then_some(distance),
            Some(end) => distance = end,
        }
    }
}

/// The end of the interval of moves along `along` over which `b` overlaps `a`, from `from` inside
/// it; `None` when that end is at or past `limit`.
fn interval_end(a: &Solid, b: &Solid, along: Vec3, from: f32, limit: f32) -> Option<f32> {
    // Past this, the solids' bounding spheres are apart.
    let reach = (b.bounds.center() + along * from - a.bounds.center()).length()
        + 0.5 * (a.bounds.size().length() + b.bounds.size().length());
    let mut outside = from + reach;
    if outside >= limit {
        if depth(a, b, along * limit) > 0.0 {
            return None;
        }
        outside = limit;
    }
    let mut inside = from;
    while outside - inside > BISECT_M {
        let middle = 0.5 * (inside + outside);
        if depth(a, b, along * middle) > 0.0 {
            inside = middle;
        } else {
            outside = middle;
        }
    }
    Some(outside)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construction::tests::{column_construction, library_of};
    use crate::map::tests::{fixture_box_surface, fixture_manifest, fixture_records};
    use crate::terrain_grid::{FIXTURE_EXTENT, TerrainExtent};
    use serde_json::json;

    /// Fixture: a map 1000 m across placing `instances` of a 1 m square, 3 m tall construction
    /// column (`construction::tests::column_construction`) and a graybox box of the same size.
    fn manifest(instances: serde_json::Value) -> MapManifest {
        let mut manifest = fixture_manifest(FIXTURE_EXTENT.half_extent(), 0.0, 100.0);
        manifest.prototypes = BTreeMap::from([
            (
                "column".to_owned(),
                Prototype::Construction {
                    asset: "column_fixture".to_owned(),
                },
            ),
            (
                "box".to_owned(),
                Prototype::Block {
                    min: [-0.5, 0.0, -0.5],
                    max: [0.5, 3.0, 0.5],
                    substance: "Brick".to_owned(),
                    surface: fixture_box_surface(),
                },
            ),
        ]);
        manifest.constructions = library_of([column_construction()]);
        manifest.instances = fixture_records(instances);
        manifest
    }

    /// One instance record at XZ `at` and `yaw` radians, with the file's (stale) Y `stale_y`.
    fn instance(prototype: &str, id: &str, at: Vec2, yaw: f32, stale_y: f32) -> serde_json::Value {
        let rotation = Quat::from_rotation_y(yaw);
        json!({
            "prototype": prototype, "id": id, "translation": [at.x, stale_y, at.y],
            "rotation": [rotation.x, rotation.y, rotation.z, rotation.w],
        })
    }

    fn flat(height: f32) -> HeightGrid {
        HeightGrid::new(vec![height; 4].into(), 2, FIXTURE_EXTENT)
    }

    /// The server's rubble rests only if no two pieces are born inside each other, so two
    /// overlapping instances are refused: two constructions, a construction and a graybox block,
    /// and an exact duplicate, each named with its depth and a move that clears it. A gate that
    /// read bounds alone, ignored blocks, or trusted one contact measure (which reads the duplicate
    /// as touching and the first pair short, [`NUDGE`]) fails here. The 1 m squares overlap 0.4 m
    /// and 0.3 m face on, so each pair's
    /// shortest clearing move is that depth, straight apart, and the duplicate's is the 1 m width,
    /// along any of four tied headings (DERIVED from the fixture).
    #[test]
    fn interpenetrating_instances_are_refused_with_their_depth_and_clearing_move() {
        let manifest = manifest(json!([
            instance("column", "a", Vec2::new(0.0, 0.0), 0.0, 0.0),
            instance("column", "b", Vec2::new(0.6, 0.0), 0.0, 0.0),
            instance("box", "c", Vec2::new(0.0, 50.0), 0.0, 0.0),
            instance("column", "d", Vec2::new(0.7, 50.0), 0.0, 0.0),
            instance("column", "e", Vec2::new(0.0, 100.0), 0.0, 0.0),
            instance("column", "f", Vec2::new(0.0, 100.0), 0.0, 0.0),
        ]));
        let grid = flat(12.0);
        let found = interpenetrations(&manifest, &grid);
        let named: Vec<(&str, &str)> = found
            .iter()
            .map(|pair| (pair.a.as_str(), pair.b.as_str()))
            .collect();
        assert_eq!(named, [("a", "b"), ("c", "d"), ("e", "f")]);
        // Within the half millimetre the refusal prints.
        let tolerance = 0.5 * TOUCH_M;
        for (pair, depth) in found.iter().zip([0.4, 0.3, 1.0]) {
            assert!((pair.depth_m - depth).abs() < tolerance, "{pair:?}");
            assert!((pair.clear.length() - depth).abs() < tolerance, "{pair:?}");
        }
        for (pair, depth) in found.iter().zip([0.4, 0.3]) {
            assert!(
                (pair.clear - Vec2::X * depth).length() < tolerance,
                "{pair:?}"
            );
        }
        let refusal =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&manifest, &grid)))
                .expect_err("an interpenetrating placement must be refused before play");
        let reason = refusal.downcast_ref::<String>().unwrap();
        for part in [
            "fixture/level.json",
            "a and b by 0.400 m (move b by at least (0.400, 0.000) m",
            "c and d by 0.300 m",
            "e and f by 1.000 m",
        ] {
            assert!(reason.contains(part), "{reason}");
        }
    }

    /// Touching is not interpenetrating: authored contact must reach play. Two columns face to
    /// face at a yaw, at map-scale coordinates where f32 placement rounds their shared face, are
    /// admitted, as is a third a hand's breadth (0.1 m) from one of them. Fixture coordinates.
    #[test]
    fn touching_and_separated_instances_are_admitted() {
        let (at, yaw) = (Vec2::new(437.3, -412.9), 0.52);
        let side = (Quat::from_rotation_y(yaw) * Vec3::X).xz();
        let manifest = manifest(json!([
            instance("column", "a", at, yaw, 0.0),
            instance("column", "b", at + side, yaw, 0.0),
            instance("column", "c", at - side * 1.1, yaw, 0.0),
        ]));
        let grid = flat(37.0);
        assert!(interpenetrations(&manifest, &grid).is_empty());
        check(&manifest, &grid);
    }

    /// The solids stand where the scatter stands them: at the terrain height under each instance,
    /// never the file's stale Y. Two columns 0.6 m apart overlap on flat ground though the file
    /// lifts one far clear; on a slope rising 10 m per metre (fixture) the same two stand 6 m apart
    /// in height, clear of each other's 3 m, though the file sets them level.
    #[test]
    fn the_terrain_height_under_each_instance_decides_overlap() {
        let placing = |stale_y: f32| {
            manifest(json!([
                instance("column", "a", Vec2::ZERO, 0.0, 0.0),
                instance("column", "b", Vec2::new(0.6, 0.0), 0.0, stale_y),
            ]))
        };
        let overlapping = interpenetrations(&placing(100.0), &flat(0.0));
        assert_eq!(overlapping.len(), 1, "{overlapping:?}");
        let slope = HeightGrid::new(
            vec![-50.0, 50.0, -50.0, 50.0].into(),
            2,
            TerrainExtent {
                world_size_m: 10.0,
                height_offset_m: -50.0,
                height_span_m: 100.0,
            },
        );
        assert!(interpenetrations(&placing(0.0), &slope).is_empty());
    }
}
