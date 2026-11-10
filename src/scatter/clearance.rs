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
//! ground pose, and each graybox block's box. Instance bounds first, then solid bounds, then
//! parry's GJK on the pairs left ([`deep`]).
//!
//! An authoring gate: hulls and queries round in native float math, so two platforms may decide a
//! pair within rounding of [`TOUCH_M`] differently (unmeasured). CI holds the shipped map
//! [`AUTHORING_MARGIN_M`] inside that tolerance.

use std::collections::BTreeMap;

use avian3d::collision::collider::contact_query::contact;
use avian3d::parry::math::Pose3;
use avian3d::parry::query::details::intersection_test_support_map_support_map_with_params;
use avian3d::parry::query::gjk::VoronoiSimplex;
use avian3d::parry::shape::SupportMap;
use avian3d::prelude::{Collider, ColliderAabb, SimpleCollider};
use bevy::prelude::*;

use super::{Placement, place, resolve};
use crate::map::{MapManifest, Prototype};
use crate::terrain_grid::HeightGrid;

/// Resolution: the deepest overlap admitted as touching ([`deep`] states what that means). Solids
/// authored face to face meet at zero depth, which f32 placement at map scale (ulp 6.1e-5 m at
/// 600 m) moves either way.
const TOUCH_M: f32 = 1.0e-3;

/// Resolution: how far inside [`TOUCH_M`] CI holds the shipped map (module doc): eight f32 ulps of
/// a map-scale coordinate, far above the rounding that could split two platforms.
#[cfg(test)]
const AUTHORING_MARGIN_M: f32 = 0.5e-3;

/// Resolution: headings sampled for a refusal's clearing move, evenly spaced; the repair hint's
/// angular step.
const HEADINGS: u16 = 72;

/// Resolution: the bisection step of a parting move's length, under half the printed millimetre.
const BISECT_M: f32 = 1.0e-4;

const SUPPORTED: &str = "convex hulls and boxes are support maps the contact queries take";

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

    fn support(&self) -> &dyn SupportMap {
        self.collider
            .shape_scaled()
            .as_support_map()
            .expect(SUPPORTED)
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
    /// An estimate of the deepest overlap: per [`deep`] pair of solids, the shortest parting move
    /// along its candidate directions (an upper bound on its depth); the deepest of those.
    depth_m: f32,
    /// An approximate shortest horizontal move of `b`, at its present height, after which no solid
    /// of either intersects the other: the shortest over [`HEADINGS`] sampled headings.
    clear: Vec2,
}

/// Refuse a placement in which two instances' solids interpenetrate deeper than [`TOUCH_M`]
/// (module doc), naming the map file, both instances, the depth and a horizontal move that clears
/// each pair.
pub(crate) fn check(manifest: &MapManifest, grid: &HeightGrid) {
    refuse(manifest, grid, TOUCH_M);
}

/// [`check`] at [`AUTHORING_MARGIN_M`] inside its tolerance: CI's form for the shipped map.
#[cfg(test)]
pub(crate) fn check_with_authoring_margin(manifest: &MapManifest, grid: &HeightGrid) {
    refuse(manifest, grid, TOUCH_M - AUTHORING_MARGIN_M);
}

fn refuse(manifest: &MapManifest, grid: &HeightGrid, tolerance: f32) {
    let found = interpenetrations(manifest, grid, tolerance);
    assert!(
        found.is_empty(),
        "map: {} field instances places solids inside each other, deeper than {tolerance} m: {} — \
         move each pair apart until their solids at most touch, then check again: a moved instance \
         stands at the terrain height under its new XZ",
        manifest.level_path().display(),
        found
            .iter()
            .map(|pair| format!(
                "{a} and {b}, about {depth:.3} m deep (move {b} by about ({x:.3}, {z:.3}) m in XZ, \
                 or {a} by the opposite)",
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

/// Every pair of instances of the placement `manifest` stands on `grid` with solids [`deep`] at
/// `tolerance`, in id order.
fn interpenetrations(
    manifest: &MapManifest,
    grid: &HeightGrid,
    tolerance: f32,
) -> Vec<Interpenetration> {
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
            let depths: Vec<f32> = pairs(a, b, Vec3::ZERO)
                .filter(|(x, y)| deep(x, y, tolerance))
                .map(|(x, y)| depth_estimate(x, y))
                .collect();
            if !depths.is_empty() {
                found.push(Interpenetration {
                    a: a.id.to_owned(),
                    b: b.id.to_owned(),
                    depth_m: depths.into_iter().fold(0.0, f32::max),
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

/// Whether `b`, moved by `offset`, is apart from `a`, on a certificate: GJK's intersection test
/// (parry's, never its depth search) answers with a direction, and both solids' support points
/// along it show the gap. Touching is not apart. GJK also answers "apart" from three failures
/// (parry3d 0.27.0 `gjk::closest_points`: a bound that stops shrinking, a simplex that rejects
/// its new point, the 100-iteration cap); their direction shows no gap, and they read as
/// intersecting.
fn apart(a: &Solid, b: &Solid, offset: Vec3) -> bool {
    let pos12 = Pose3::from_parts(a.position, a.rotation)
        .inv_mul(&Pose3::from_parts(b.position + offset, b.rotation));
    let (a_shape, b_shape) = (a.support(), b.support());
    let (intersecting, dir) = intersection_test_support_map_support_map_with_params(
        &pos12,
        a_shape,
        b_shape,
        &mut VoronoiSimplex::new(),
        None,
    );
    !intersecting
        && a_shape.local_support_point(dir).dot(dir) < b_shape.support_point(&pos12, -dir).dot(dir)
}

/// Whether `a` and `b` overlap deeper than `tolerance`: they are not [`apart`], and no move of `b`
/// by `tolerance` along a candidate direction ([`directions`]) sets them apart. A pair is admitted
/// only on that certificate, which bounds its depth by `tolerance`; a pair the library cannot show
/// apart is refused, whatever its depth search reads. Touching solids part: one axis lies within
/// 54.7° of any contact normal, so its move opens a gap of at least 0.58 `tolerance`.
fn deep(a: &Solid, b: &Solid, tolerance: f32) -> bool {
    !apart(a, b, Vec3::ZERO)
        && directions(a, b)
            .into_iter()
            .all(|along| !apart(a, b, along * tolerance))
}

/// Candidate directions to part `b` from `a`, each both ways, so swapping the pair leaves the set
/// unchanged: the axes, the line between their bounds' centres, and the contact query's normal
/// where its depth search (parry's EPA) gives one. A wrong normal only fails to part them.
fn directions(a: &Solid, b: &Solid) -> Vec<Vec3> {
    let normal = contact(
        &a.collider,
        a.position,
        a.rotation,
        &b.collider,
        b.position,
        b.rotation,
        0.0,
    )
    .expect(SUPPORTED)
    .and_then(|contact| (a.rotation * contact.local_normal1).try_normalize());
    [Vec3::X, Vec3::Y, Vec3::Z]
        .into_iter()
        .chain((b.bounds.center() - a.bounds.center()).try_normalize())
        .chain(normal)
        .flat_map(|along| [along, -along])
        .collect()
}

/// [`Interpenetration::depth_m`] of one [`deep`] pair.
fn depth_estimate(a: &Solid, b: &Solid) -> f32 {
    directions(a, b)
        .into_iter()
        .map(|along| {
            interval_end(a, b, along, 0.0, f32::INFINITY)
                .expect("an unlimited parting move along any direction exists")
        })
        .fold(f32::INFINITY, f32::min)
}

/// [`Interpenetration::clear`].
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

/// How far `b` moves along `heading` before no pair of solids intersects; `None` when that is no
/// shorter than `shortest`.
///
/// Along a line, the moves at which two convex solids intersect form one interval: the line
/// crosses their Minkowski difference, which is convex. So passing a pair's interval end retires
/// that pair, and the walk ends within one pass per pair.
fn clearing_distance(a: &Solids, b: &Solids, heading: Vec2, shortest: f32) -> Option<f32> {
    let along = Vec3::new(heading.x, 0.0, heading.y);
    let mut distance = 0.0;
    loop {
        let mut beyond = None;
        for (x, y) in pairs(a, b, along * distance) {
            if !apart(x, y, along * distance) {
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

/// The end of the interval of moves along `along`, a unit vector, over which `b` intersects `a`,
/// from `from` inside it; `None` when that end is at or past `limit`.
fn interval_end(a: &Solid, b: &Solid, along: Vec3, from: f32, limit: f32) -> Option<f32> {
    // Past this, the solids' bounding spheres are apart.
    let reach = (b.bounds.center() + along * from - a.bounds.center()).length()
        + 0.5 * (a.bounds.size().length() + b.bounds.size().length());
    let mut outside = from + reach;
    if outside >= limit {
        if !apart(a, b, along * limit) {
            return None;
        }
        outside = limit;
    }
    let mut inside = from;
    while outside - inside > BISECT_M {
        let middle = 0.5 * (inside + outside);
        // Past 1024 m, adjacent f32 lengths lie further apart than the step.
        if middle <= inside || middle >= outside {
            break;
        }
        if apart(a, b, along * middle) {
            outside = middle;
        } else {
            inside = middle;
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

    /// The ids of the refused pairs of `instances` on `grid`, at the shipped tolerance.
    fn refused(instances: serde_json::Value, grid: &HeightGrid) -> Vec<(String, String)> {
        interpenetrations(&manifest(instances), grid, TOUCH_M)
            .into_iter()
            .map(|pair| (pair.a, pair.b))
            .collect()
    }

    fn named(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    /// The server's rubble rests only if no two pieces are born inside each other, so two
    /// overlapping instances are refused: two constructions, a construction and a graybox block,
    /// and an exact duplicate (which the contact query's depth search reads as touching), each
    /// named with an estimate of its depth and a move that clears it. A gate that read bounds
    /// alone, ignored blocks or trusted the depth search admits one. The 1 m squares overlap
    /// 0.4 m and 0.3 m face on, so each pair's depth and shortest clearing move is that, straight
    /// apart, and the duplicate's is the 1 m width along any of four tied headings (DERIVED from
    /// the fixture).
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
        let found = interpenetrations(&manifest, &grid, TOUCH_M);
        let ids: Vec<(String, String)> = found
            .iter()
            .map(|pair| (pair.a.clone(), pair.b.clone()))
            .collect();
        assert_eq!(ids, named(&[("a", "b"), ("c", "d"), ("e", "f")]));
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
            "a and b, about 0.400 m deep (move b by about (0.400, 0.000) m",
            "c and d, about 0.300 m deep",
            "e and f, about 1.000 m deep",
        ] {
            assert!(reason.contains(part), "{reason}");
        }
    }

    /// The tolerance's meaning: two columns overlapping 0.9 mm face on are touching and admitted,
    /// 1.1 mm deep are refused, whichever instance's id comes first. A gate at another tolerance,
    /// or one whose decision leans on which solid it moves, splits these.
    #[test]
    fn the_touch_tolerance_admits_0_9_mm_and_refuses_1_1_mm_in_either_id_order() {
        let at = Vec2::new(-312.7, 205.1); // Fixture: map-scale coordinates.
        for (left, right) in [("a", "b"), ("b", "a")] {
            for (depth_m, expected) in [(0.9e-3, Vec::new()), (1.1e-3, named(&[("a", "b")]))] {
                let pair = json!([
                    instance("column", left, at, 0.0, 0.0),
                    instance("column", right, at + Vec2::X * (1.0 - depth_m), 0.0, 0.0),
                ]);
                assert_eq!(
                    refused(pair, &flat(3.0)),
                    expected,
                    "{left} left, {depth_m} m"
                );
            }
        }
    }

    /// Touching is not interpenetrating: authored contact must reach play. Two columns face to
    /// face at a yaw, at map-scale coordinates where f32 placement rounds their shared face, are
    /// admitted, as is a third a hand's breadth (0.1 m) from one of them; the same face 2 mm
    /// deeper is refused, so the admission is the tolerance's and not a gate that admits all.
    /// Fixture coordinates.
    #[test]
    fn touching_and_separated_instances_are_admitted() {
        let (at, yaw) = (Vec2::new(437.3, -412.9), 0.52);
        let side = (Quat::from_rotation_y(yaw) * Vec3::X).xz();
        let placing = |inward: f32| {
            json!([
                instance("column", "a", at, yaw, 0.0),
                instance("column", "b", at + side * (1.0 - inward), yaw, 0.0),
                instance("column", "c", at - side * 1.1, yaw, 0.0),
            ])
        };
        let grid = flat(37.0);
        assert_eq!(refused(placing(0.0), &grid), named(&[]));
        check(&manifest(placing(0.0)), &grid);
        assert_eq!(refused(placing(2.0e-3), &grid), named(&[("a", "b")]));
    }

    /// The solids stand where the scatter stands them: at the terrain height under each instance,
    /// never the file's stale Y, and at each instance's own yaw. Two columns 0.6 m apart overlap
    /// on flat ground though the file lifts one far clear; on a slope rising 10 m per metre
    /// (fixture) the same two stand 6 m apart in height, clear of each other's 3 m, though the
    /// file sets them level. Two columns 1.1 m apart clear each other square on, and overlap once
    /// one turns 45°, its half-diagonal reaching 0.71 m (DERIVED).
    #[test]
    fn each_instance_stands_at_its_terrain_height_and_yaw() {
        let placing = |stale_y: f32| {
            json!([
                instance("column", "a", Vec2::ZERO, 0.0, 0.0),
                instance("column", "b", Vec2::new(0.6, 0.0), 0.0, stale_y),
            ])
        };
        assert_eq!(refused(placing(100.0), &flat(0.0)), named(&[("a", "b")]));
        let slope = HeightGrid::new(
            vec![-50.0, 50.0, -50.0, 50.0].into(),
            2,
            TerrainExtent {
                world_size_m: 10.0,
                height_offset_m: -50.0,
                height_span_m: 100.0,
            },
        );
        assert_eq!(refused(placing(0.0), &slope), named(&[]));
        let turned = |yaw: f32| {
            json!([
                instance("column", "a", Vec2::ZERO, yaw, 0.0),
                instance("column", "b", Vec2::new(1.1, 0.0), 0.0, 0.0),
            ])
        };
        assert_eq!(refused(turned(0.0), &flat(0.0)), named(&[]));
        assert_eq!(
            refused(turned(std::f32::consts::FRAC_PI_4), &flat(0.0)),
            named(&[("a", "b")])
        );
    }
}
