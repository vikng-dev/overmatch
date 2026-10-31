//! Capture-plan `LodShowcase`: certified shoe LOD pairs at their actual switch distances.
//!
//! The scene flattens the shared terrain grid and omits scatter, then stages a controlled tank and
//! paired fine/coarse levels. The certificate and live view profile own the switch distances;
//! the map extent owns the available range. Unreachable switches are reported rather than moved.
//! [`LodClamp`] pins only this instrument's pairs. ADR-0036 owns the re-arm rationale.

use bevy::prelude::*;

use crate::geometry_lod::TankCertificate;
use crate::track::link_view::{ShoeBelt, shoe_chain_key};
use crate::view::{PlayerView, ViewProfile};

/// Mount the showcase's runtime half — the shoe clamp and the one-shot camera aim.
///
/// The systems are mounted only when the capture plan selects the showcase.
pub fn plugin(app: &mut App) {
    if !enabled() {
        return;
    }
    app.add_systems(Update, (clamp_showcase_shoes, aim_camera_down_range));
}

/// Is this process running the LOD showcase?
///
/// Read by [`plugin`], by `tank::scenario`'s spawn (which lays out the pairs instead of the duel),
/// by `terrain_grid`'s decode (which flattens the world instead of loading it) and by `world`'s
/// scatter call (which skips the map's objects). Those four are the whole of its reach.
pub(crate) fn enabled() -> bool {
    matches!(
        crate::dev_capture::plan().and_then(|plan| plan.scene.demo),
        Some(crate::dev_capture::plan::Demo::LodShowcase)
    )
}

/// ASSUMED presentation spacing, metres: two broadside silhouettes straddle the sight line.
const LATERAL_HALF_M: f32 = 6.0;
/// ASSUMED presentation lane offset, metres: alternating lanes reduce mutual occlusion.
const LANE_OFFSET_M: f32 = 15.0;

/// The map's usable staging interval, with DERIVED 5% margins at both X edges.
#[derive(Clone, Copy)]
pub(crate) struct Stage {
    start: Vec2,
    reach: f32,
}

impl Stage {
    pub(crate) fn new(extent: crate::terrain_grid::TerrainExtent) -> Self {
        let half = extent.half_extent();
        assert!(
            half.is_finite() && half > 0.0,
            "the map load gate supplies a finite positive extent"
        );
        Self {
            start: Vec2::new(-0.95 * half, 0.0),
            reach: 1.9 * half,
        }
    }
}

/// One tank the showcase spawns.
pub(crate) struct ShowcaseTank {
    /// Its spawn point; Y comes from the live surface like every other spawn.
    pub(crate) xz: Vec2,
    /// Rotation about world up. The player faces down-range; the pairs stand broadside, which is
    /// [`Quat::IDENTITY`] here because the hull's longitudinal axis is its local Z and the sight
    /// line is X.
    pub(crate) yaw: Quat,
    /// Which shoe level to pin this tank's whole belt to, or `None` for the player (whose shoes
    /// select normally — the player is the control, and its track is what the ladder does when
    /// nobody is interfering).
    pub(crate) clamp: Option<usize>,
    pub(crate) name: String,
    pub(crate) controlled: bool,
}

/// Pin every shoe under this tank to one level of the chain, whatever distance it is at.
///
/// On the tank ROOT, read by [`clamp_showcase_shoes`] through the belt's ancestors — so it survives
/// the rig binding, rebinding, and the belt's pool being rebuilt, none of which the showcase knows
/// about or should.
#[derive(Component, Clone, Copy)]
pub(crate) struct LodClamp(pub(crate) usize);

/// The shoe chain's switch distances under `view`, nearest first — `switches[i]` is where level
/// `i + 1` takes over from level `i`.
///
/// Derived from the certificate through `geometry_lod`'s own projection, so this file is not a
/// second copy of a ladder: a re-cut asset or a moved view profile stages a different scene with no
/// edit here. An absent chain stages nothing.
pub(crate) fn shoe_switches(certificate: &TankCertificate, view: ViewProfile) -> Vec<f32> {
    certificate
        .chain(&shoe_chain_key())
        .map(|chain| chain.switches(view))
        .unwrap_or_default()
}

/// The certified deviation each rung carries, millimetres, nearest first — what the legend quotes
/// in place of a triangle count (the certificate carries deviations; triangles are an output of
/// generation and are never certified).
pub(crate) fn shoe_deviations(certificate: &TankCertificate) -> Vec<f32> {
    certificate
        .chain(&shoe_chain_key())
        .map(|chain| chain.rungs.iter().map(|rung| rung.deviation_mm).collect())
        .unwrap_or_default()
}

/// Stage only switches inside the declared map's interval, at their exact certified distance.
fn staged_pairs(switches: &[f32], stage: Stage) -> Vec<usize> {
    (0..switches.len())
        .filter(|&pair| switches[pair] <= stage.reach)
        .collect()
}

/// Report every switch beyond this map's reach.
fn unstageable_pairs(switches: &[f32], stage: Stage) -> Vec<(usize, f32)> {
    switches
        .iter()
        .copied()
        .enumerate()
        .filter(|&(_, range)| range > stage.reach)
        .collect()
}

/// Which lane the `slot`-th staged pair stands in, out of `staged`: the world Z its two tanks
/// straddle. See [`LANE_OFFSET_M`].
///
/// The sign ALTERNATES and the magnitude FALLS OFF with the slot, and both halves are load-bearing.
/// Occlusion is angular: a pair at range R covers `atan((z ± h) / R)` about the sight line, and that
/// span shrinks as R grows — so two pairs at the same lateral offset on the same side always overlap,
/// the far one hiding inside the near one's span. Standing the NEAREST pair furthest off the line and
/// each following one nearer to it is what keeps the intervals disjoint for a chain of any depth.
fn pair_lane_z(slot: usize, staged: usize) -> f32 {
    let magnitude = 2.0 * LANE_OFFSET_M * (staged - slot) as f32 / staged.max(1) as f32;
    if slot.is_multiple_of(2) {
        magnitude
    } else {
        -magnitude
    }
}

/// The whole scene: the player first, then two tanks per switch in the chain.
///
/// A pure function of the chain: no window, world or asset load.
pub(crate) fn layout(switches: &[f32], stage: Stage) -> Vec<ShowcaseTank> {
    // Down-range is +X and the camera looks along it, so the hull (whose forward is its local −Z,
    // bevy's convention) has to be yawed a quarter turn. The PAIRS keep the identity rotation on
    // purpose: it leaves their longitudinal axis on Z, broadside to the sight line, which is the
    // pose that shows a whole track run instead of a nose.
    let down_range = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let mut tanks = vec![ShowcaseTank {
        xz: stage.start,
        yaw: down_range,
        clamp: None,
        name: "Player".to_string(),
        controlled: true,
    }];
    // `slot` is the position in the STAGED sequence, not the pair's index in the chain — the lanes
    // must alternate over the pairs that are actually laid out. Keying the lane off `pair` would
    // put two consecutive staged pairs in the SAME lane the moment a skipped switch fell between
    // them, which is the occlusion the alternation exists to prevent.
    let pairs = staged_pairs(switches, stage);
    let staged = pairs.len();
    for (slot, pair) in pairs.into_iter().enumerate() {
        let range = switches[pair];
        let lane = pair_lane_z(slot, staged);
        // LEFT is the FINER level, and left is −Z: with +X forward and +Y up, `left = up × forward`
        // = Y × X = −Z. Fine on the left every time, so a sweep down the range is read the same way
        // at every pair rather than remembered per pair.
        for (side, level) in [(-1.0, pair), (1.0, pair + 1)] {
            tanks.push(ShowcaseTank {
                xz: stage.start + Vec2::new(range, lane + side * LATERAL_HALF_M),
                yaw: Quat::IDENTITY,
                clamp: Some(level),
                name: format!("LOD{level} @ {range:.0} m"),
                controlled: false,
            });
        }
    }
    tanks
}

/// The legend, one line per pair: where it stands, and which two meshes are standing there.
///
/// Written as text rather than logged here so the spawn can emit it beside the tanks it describes —
/// a legend in a different part of the log from the scene it labels is a legend nobody reads.
pub(crate) fn legend(switches: &[f32], deviations: &[f32], stage: Stage) -> Vec<String> {
    let staged = staged_pairs(switches, stage).len();
    let mut lines: Vec<String> = staged_pairs(switches, stage)
        .into_iter()
        .enumerate()
        .map(|(slot, pair)| {
            let range = switches[pair];
            format!(
                "lod showcase: L{pair}|L{} pair at {range:.1} m, {} of the sight line — \
                 LEFT L{pair}, RIGHT L{} ({:.3} mm certified deviation)",
                pair + 1,
                if pair_lane_z(slot, staged) < 0.0 {
                    "left"
                } else {
                    "right"
                },
                pair + 1,
                *deviations
                    .get(pair)
                    .expect("switches and certified deviations come from the same LOD chain"),
            )
        })
        .collect();
    lines.extend(
        unstageable_pairs(switches, stage)
            .into_iter()
            .map(|(pair, switch)| {
                format!(
                    "lod showcase: L{pair}|L{} switches at {switch:.1} m, past this map's {:.0} m \
             reach — NOT staged, because standing it at the edge would be a different comparison",
                    pair + 1,
                    stage.reach,
                )
            }),
    );
    lines
}

/// Pin every clamped tank's belts to their tank's level, as they appear.
///
/// `Added<ShoeBelt>` rather than a sweep: belts arrive when the rig binds, and a rig rebind despawns
/// them and spawns fresh ones, so the added filter covers the whole lifetime while costing an empty
/// query every frame after the first.
///
/// The clamp is looked up through the ANCESTORS rather than passed down from the spawn, for the
/// same reason `render_policy` resolves scopes that way: the showcase does not know how many frames
/// deep `track::view` parents its pool, and should not have to.
fn clamp_showcase_shoes(
    mut belts: Query<(Entity, &mut ShoeBelt), Added<ShoeBelt>>,
    parents: Query<&ChildOf>,
    clamps: Query<&LodClamp>,
) {
    for (entity, mut belt) in &mut belts {
        let mut node = Some(entity);
        let mut clamp = None;
        while let Some(current) = node {
            if let Ok(found) = clamps.get(current) {
                clamp = Some(found.0);
                break;
            }
            node = parents.get(current).ok().map(ChildOf::parent);
        }
        if let Some(clamp) = clamp {
            belt.pin(clamp);
        }
    }
}

/// Point the camera down-range once, on the first frame it exists.
///
/// `camera::spawn_camera` aims at the duel's own geometry, which is 500 m away and 90° off here, so
/// without this the showcase opens looking at empty grass and the first thing anyone does is hunt
/// for the tanks. Writing the ROTATION is enough and is not fought over: the orbit camera reads its
/// yaw and pitch back off the transform each frame (there is no stored orientation) and writes only
/// the translation, so a single write survives until the mouse moves.
fn aim_camera_down_range(
    mut done: Local<bool>,
    mut camera: Query<&mut Transform, With<PlayerView>>,
) {
    if *done {
        return;
    }
    let Ok(mut transform) = camera.single_mut() else {
        return;
    };
    // A bevy camera looks down its own −Z, so a −90° yaw puts that on +X.
    transform.rotation = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    *done = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capture owner needs an exact-range comparison; changing map size must stage newly
    /// reachable switches without moving any of the certificate's distances to an old map edge.
    #[test]
    fn staging_follows_the_declared_extent_without_moving_switches() {
        let extent = |world_size_m| crate::terrain_grid::TerrainExtent {
            world_size_m,
            height_offset_m: 0.0,
            height_span_m: 0.0,
        };
        let switches = [400.0, 1100.0]; // FIXTURE DATA.
        let narrow = Stage::new(extent(1000.0));
        let wide = Stage::new(extent(1500.0));
        assert_eq!(staged_pairs(&switches, narrow), [0]);
        assert_eq!(staged_pairs(&switches, wide), [0, 1]);
        let tanks = layout(&switches, wide);
        assert_eq!(tanks.len(), 5);
        for (pair, range) in switches.into_iter().enumerate() {
            assert_eq!(tanks[1 + 2 * pair].xz.x - tanks[0].xz.x, range);
        }
    }
}
