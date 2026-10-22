//! `OVERMATCH_LOD_SHOWCASE=1`: the shoe LOD ladder laid out on the ground, PAIRED, so a human can
//! judge every switch the pipeline cut.
//!
//! # Why this exists
//!
//! `scripts/lod/generate.py` certifies every level with a PROVEN BOUND on its worst-case surface
//! deviation at the distance it takes over. That bound is the whole guarantee, and there is exactly
//! one question it cannot answer: whether the swap LOOKS like a swap. The eye is the only audit of
//! appearance, and re-arming a rendered gate has a named trigger: a switch seen to pop, here
//! (ADR 0036 §3).
//!
//! Looking is harder than it sounds, which is the actual reason this file exists rather than a note
//! telling a human to drive around. To see a switch you have to be at its exact distance from a
//! tank, on ground flat enough that the tank is not half-buried, holding the optic steady, and —
//! the part no amount of driving gets you — seeing BOTH meshes at once. A switch judged by driving
//! toward a tank is judged from memory: the coarse mesh, then a blink, then the fine one, seconds
//! apart. This stands them SIDE BY SIDE instead.
//!
//! # What it does
//!
//! One environment variable, no knobs (a debug instrument with a settings surface is a feature, and
//! this is not one):
//!
//!   1. The terrain is FLATTENED — at the GRID, before anything reads it, so the oracle, the
//!      collider and the render mesh are all flat by the same construction that keeps them agreeing
//!      on the shipped map (`terrain_grid`'s one-surface doctrine). Flattening only the render mesh
//!      would put the tanks on invisible hills. The map's object scatter is skipped with it: 709
//!      houses and firs are scenery standing in front of the thing being looked at.
//!   2. The player spawns at one edge of the 1 000 m map facing down-range.
//!   3. At every switch distance the shoe's certified chain derives, a PAIR of stationary Tigers
//!      stands broadside
//!      to the sight line: the LEFT one clamped to the finer level, the RIGHT one to the coarser.
//!      So at each range the two tanks in frame are exactly the two meshes that switch is between,
//!      at exactly the distance the runtime swaps them.
//!   4. A legend goes to the log: one line per pair, with the levels and the coarser one's
//!      certified deviation.
//!
//! Nothing here is mounted, spawned or ticked when the variable is unset — [`plugin`] adds no
//! systems at all, [`crate::tank::scenario`] takes its ordinary path, and the heightmap decodes
//! normally.
//!
//! # The clamp is the whole trick, and it is showcase-only
//!
//! A belt's rung is SELECTED by its distance and the ladder tiles `[0, ∞)`, so "L2 and L3 at one
//! range" cannot be reached on the production path at all.
//! [`clamp_showcase_shoes`] — which does not exist in a process without the variable — pins one
//! tank's belts to a rung through [`crate::track::link_view::ShoeBelt::pin`], the one override the
//! selector honours. No production code branches on any of it, and the certificate still owns every
//! distance the selector derives: a pin is not a point on the ladder, so it asks the chain for
//! nothing.

use bevy::prelude::*;

use crate::geometry_lod::TankCertificate;
use crate::track::link_view::{ShoeBelt, shoe_chain_key};
use crate::view::{PlayerView, ViewProfile};

/// Mount the showcase's runtime half — the shoe clamp and the one-shot camera aim.
///
/// Adds NOTHING when the variable is unset, which is the cheapest possible "zero cost when off":
/// not a disabled system, not a `run_if` evaluated every frame, no system at all.
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
    crate::env_flag("OVERMATCH_LOD_SHOWCASE", false)
}

/// Where the player stands: hard against the map's west edge, on the centre line.
///
/// −480 leaves 950 m of usable down-range on a 1 000 m map with 20 m of shoulder behind the spawn,
/// and clears a wider one by more. Down-range is +X and the pairs are laid out along it; LATERAL is
/// therefore Z, and the player's LEFT (with +Y up and +X forward) is −Z. The map's own square bounds
/// this layout: `terrain_grid::spawn_pos` panics on a tank placed off the span.
const START_XZ: Vec2 = Vec2::new(-480.0, 0.0);

/// Half the lateral gap between a pair's two tanks, metres — so the pair straddles the sight line
/// at ±6 m.
///
/// 6 rather than 4 because the tanks stand BROADSIDE: a Tiger is MEASURED 8.45 m long over the
/// tracks, and a pair presenting its full track run needs more than a hull length between centres
/// or the two silhouettes touch at range. 12 m centre-to-centre leaves ~3.5 m of daylight, which
/// reads as two tanks rather than one long one, and is still narrow enough that both fit the
/// gunner optic's 0.12 rad frame at the nearest pair.
const LATERAL_HALF_M: f32 = 6.0;

/// How far off the sight line a pair's CENTRE sits, metres — alternating side by side, so
/// consecutive pairs stand in opposite lanes.
///
/// MEASURED against the thing it fixes, not chosen for looks. With every pair centred on the axis
/// the pairs line up behind one another, and a broadside tank at 127 m subtends ±0.080 rad about the
/// line while the 501 m pair behind it subtends only ±0.020 — so the near tank covers the outer
/// third of the far one, and the L2→L3 switch (the whole reason this harness exists) is the pair
/// that gets eaten. Alternating lanes at ±15 m puts consecutive pairs on opposite sides: the 501 m
/// pair occupies 0.010..0.050 rad while everything nearer on its side is beyond 0.086, and the 950 m
/// pair sits at −0.026..−0.004 against a nearest neighbour on its side at −0.198..−0.038.
///
/// It is the FARTHEST pair's offset; nearer pairs stand proportionally further out (see
/// [`pair_lane_z`]), which is what keeps a deep chain's pairs angularly disjoint rather than merely
/// alternating.
const LANE_OFFSET_M: f32 = 15.0;

/// The furthest down-range a pair may stand, metres from [`START_XZ`].
///
/// The narrowest map this layout targets runs out at +500 and a tank needs its footprint clearance
/// inside it, so 950 m from −480 puts the last pair at x = +470 with 30 m to spare. Any switch
/// beyond this is a switch that cannot be staged on this map — the legend says so rather than the
/// pair silently standing somewhere it was not asked to.
const MAX_RANGE_M: f32 = 950.0;

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

/// The switches this harness stages: the ones that FIT ON THE MAP, at their own distance.
///
/// It used to stage the switches the rendered-difference gate had an opinion about, dropping the
/// ones where the asset fell under a 20-pixel footprint floor. ADR 0036 §3 deleted that gate, and
/// inventing a replacement threshold here would be a taste call nobody made wearing the retired
/// gate's numbers. The rule that replaces it is not about how small a thing is, it is about whether
/// this harness can show the comparison AT ALL: a pair belongs at the distance its switch happens,
/// and a switch past [`MAX_RANGE_M`] cannot be stood at on this map. Parking those two tanks at the
/// map edge instead would ask a person to judge a swap at a distance the runtime never performs it,
/// which is a different comparison wearing this one's label.
///
/// On today's four-rung ladder that stages three and drops the L3|L4 pair, whose switch is 1 499.6 m
/// against a 950 m reach. A wider map stages it with no edit here, and a ladder of any other depth
/// stages whatever subset of ITS switches the map can reach.
///
/// The alternating lanes are what keep the staged pairs clear of each other — see
/// [`LANE_OFFSET_M`].
pub(crate) fn staged_pairs(switches: &[f32]) -> Vec<usize> {
    (0..switches.len())
        .filter(|&pair| switches[pair] <= MAX_RANGE_M)
        .collect()
}

/// The switches this map cannot reach, with the distance they happen at — so the legend says what
/// is missing rather than leaving a person to notice a gap in the ladder they were shown.
fn unstageable_pairs(switches: &[f32]) -> Vec<(usize, f32)> {
    (0..switches.len())
        .map(|pair| (pair, switches[pair]))
        .filter(|&(_, switch)| switch > MAX_RANGE_M)
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
pub(crate) fn layout(switches: &[f32]) -> Vec<ShowcaseTank> {
    // Down-range is +X and the camera looks along it, so the hull (whose forward is its local −Z,
    // bevy's convention) has to be yawed a quarter turn. The PAIRS keep the identity rotation on
    // purpose: it leaves their longitudinal axis on Z, broadside to the sight line, which is the
    // pose that shows a whole track run instead of a nose.
    let down_range = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let mut tanks = vec![ShowcaseTank {
        xz: START_XZ,
        yaw: down_range,
        clamp: None,
        name: "Tiger I (player)".to_string(),
        controlled: true,
    }];
    // `slot` is the position in the STAGED sequence, not the pair's index in the chain — the lanes
    // must alternate over the pairs that are actually laid out. Keying the lane off `pair` would
    // put two consecutive staged pairs in the SAME lane the moment a skipped switch fell between
    // them, which is the occlusion the alternation exists to prevent.
    let pairs = staged_pairs(switches);
    let staged = pairs.len();
    for (slot, pair) in pairs.into_iter().enumerate() {
        let range = switches[pair];
        let lane = pair_lane_z(slot, staged);
        // LEFT is the FINER level, and left is −Z: with +X forward and +Y up, `left = up × forward`
        // = Y × X = −Z. Fine on the left every time, so a sweep down the range is read the same way
        // at every pair rather than remembered per pair.
        for (side, level) in [(-1.0, pair), (1.0, pair + 1)] {
            tanks.push(ShowcaseTank {
                xz: START_XZ + Vec2::new(range, lane + side * LATERAL_HALF_M),
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
pub(crate) fn legend(switches: &[f32], deviations: &[f32]) -> Vec<String> {
    let staged = staged_pairs(switches).len();
    let mut lines: Vec<String> = staged_pairs(switches)
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
                deviations.get(pair).copied().unwrap_or(f32::NAN),
            )
        })
        .collect();
    lines.extend(unstageable_pairs(switches).into_iter().map(|(pair, switch)| {
        format!(
            "lod showcase: L{pair}|L{} switches at {switch:.1} m, past this map's {MAX_RANGE_M:.0} m \
             reach — NOT staged, because standing it at the edge would be a different comparison",
            pair + 1,
        )
    }));
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
