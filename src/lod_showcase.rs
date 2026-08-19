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
//! A belt's rung is SELECTED by its distance, and the ladder tiles `[0, ∞)` — which is exactly what
//! makes "show me L2 and L3 at the same distance" impossible on the production path, and rightly
//! so. [`clamp_showcase_shoes`] — which does not exist in a process without the variable — pins one
//! tank's belts to a rung through [`crate::track::link_view::ShoeBelt::pin`], the one override the
//! selector honours. No production code branches on any of it, and the certificate still owns every
//! distance the selector derives: a pin is not a point on the ladder, so it asks the chain for
//! nothing.

use bevy::prelude::*;

use crate::geometry_lod::TankCertificate;
use crate::track::link_view::{ShoeBelt, shoe_chain_key};
use crate::view::ViewProfile;

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
/// therefore Z, and the player's LEFT (with +Y up and +X forward) is −Z. The map's own square is
/// what `no_showcase_tank_stands_off_the_map` holds this layout to.
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
/// The alternating lanes are what keep the staged pairs clear of each other — see [`LANE_OFFSET_M`]
/// and `no_pair_stands_in_front_of_another_from_the_player_spawn`.
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
/// A pure function of the chain, so the layout is a thing a test can check without a window, a
/// world or an asset load.
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
fn aim_camera_down_range(mut done: Local<bool>, mut camera: Query<&mut Transform, With<Camera3d>>) {
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
    use crate::view::ViewFacts;

    /// The shipped shoe chain's switch distances at the view the corpus was quoted in — the gunner
    /// optic at 4K native, one pixel of budget.
    fn shipped_switches() -> Vec<f32> {
        let certificate =
            TankCertificate(std::sync::Arc::new(crate::geometry_lod::certificate::load(
                &crate::assets::asset_root(),
                crate::geometry_lod::TIGER_ID,
            )));
        shoe_switches(
            &certificate,
            ViewProfile::of(
                ViewFacts::new(crate::camera::GUNNER_FOV_FALLBACK, 2160.0),
                1.0,
            ),
        )
    }

    /// The shipped shoe chain's certified deviations, nearest first.
    fn shipped_deviations() -> Vec<f32> {
        shoe_deviations(&TankCertificate(std::sync::Arc::new(
            crate::geometry_lod::certificate::load(
                &crate::assets::asset_root(),
                crate::geometry_lod::TIGER_ID,
            ),
        )))
    }

    /// The layout is the CHAIN's, not a table: one pair per JUDGEABLE switch, standing at the
    /// distance that switch happens, with the finer level on the left.
    ///
    /// Driven off the CERTIFICATE's own switch distances so a re-cut ladder re-lays itself out —
    /// which is the property that makes this harness worth keeping rather than re-writing each time
    /// the meshes change.
    #[test]
    fn every_judgeable_switch_gets_a_pair_at_its_own_distance() {
        let switches = shipped_switches();
        let tanks = layout(&switches);
        let staged = staged_pairs(&switches);
        assert_eq!(
            tanks.len(),
            1 + 2 * staged.len(),
            "the player plus two per staged switch"
        );

        assert!(tanks[0].controlled, "the player is the controlled tank");
        assert_eq!(tanks[0].clamp, None, "and its shoes select normally");
        assert!(
            tanks[1..].iter().all(|t| !t.controlled),
            "the probes are scenery — a controlled probe would drive out of its own pair",
        );

        for (slot, &pair) in staged.iter().enumerate() {
            let (left, right) = (&tanks[1 + 2 * slot], &tanks[2 + 2 * slot]);
            assert_eq!(left.clamp, Some(pair), "left is the FINER level");
            assert_eq!(right.clamp, Some(pair + 1), "right is the COARSER level");

            // Both at the switch's own distance, straddling their lane. Compared with a millimetre
            // of slack because the spawn point is an ABSOLUTE world coordinate: 55.9124 added to
            // −480 and taken back off is 55.912415, and a harness whose pairs stand a hundredth of a
            // millimetre out is not a defect worth a red suite.
            let switch = switches[pair];
            let expected = switch.min(MAX_RANGE_M);
            assert!((left.xz.x - START_XZ.x - expected).abs() < 1e-3);
            assert!((right.xz.x - START_XZ.x - expected).abs() < 1e-3);
            let lane = START_XZ.y + pair_lane_z(slot, staged.len());
            assert_eq!(left.xz.y, lane - LATERAL_HALF_M, "left is −Z");
            assert_eq!(right.xz.y, lane + LATERAL_HALF_M);
        }
    }

    /// The staging RULE, stated against the CHAIN and the MAP rather than against a count — so this
    /// keeps meaning the same thing when the ladder is next re-cut.
    ///
    /// A pair is staged iff its switch fits inside this map's reach. Nothing in the tree decides a
    /// switch is too SMALL to look at any more: the rendered-difference gate that used to abstain
    /// below a 20-pixel footprint is deleted (ADR 0036 §3), and inventing a replacement threshold
    /// here would be a taste call nobody made. What survives is a geometric fact about the map.
    #[test]
    fn the_harness_stages_exactly_the_switches_the_map_can_reach() {
        let switches = shipped_switches();
        let staged = staged_pairs(&switches);
        for (pair, &switch) in switches.iter().enumerate() {
            assert_eq!(
                staged.contains(&pair),
                switch <= MAX_RANGE_M,
                "pair {pair} (the switch into L{}) is staged iff it happens within {MAX_RANGE_M} m",
                pair + 1,
            );
        }
        assert_eq!(
            staged.len() + unstageable_pairs(&switches).len(),
            switches.len(),
            "every switch is either staged or named in the legend as out of reach",
        );
        assert!(
            !staged.is_empty(),
            "a showcase that stages nothing is a showcase nobody can use — if a re-cut ladder ever \
             put every switch past the map edge, the harness needs a bigger map, not a silent \
             empty scene",
        );
    }

    /// NO PAIR HIDES BEHIND ANOTHER, from the one place the harness is meant to be used from.
    ///
    /// The whole scene is laid out along one line and viewed from one end of it, which is a layout
    /// whose default failure is a near tank standing in front of a far one — and the far ones are
    /// the interesting ones. It is invisible in a test that only checks coordinates (every tank is
    /// exactly where it was asked to be) and obvious the moment anyone looks, which is precisely the
    /// kind of thing to assert instead of eyeball twice.
    ///
    /// Asserted as ANGULAR span from the spawn: each broadside tank covers `atan(z / x)` over its
    /// own length, and the terrain is flat and every tank the same height, so a horizontal overlap
    /// with anything nearer IS an occlusion. The lanes ([`LANE_OFFSET_M`]) are what separate them;
    /// this says by how much, and fails if a regenerated ladder ever puts two pairs close enough in
    /// range that alternating lanes stop being enough.
    #[test]
    fn no_pair_stands_in_front_of_another_from_the_player_spawn() {
        /// A Tiger over the tracks, metres — MEASURED, and the extent that matters here because the
        /// tanks stand BROADSIDE (their long axis across the sight line).
        const HULL_LENGTH_M: f32 = 8.45;
        /// How much clear sky a pair must have on either side of it, radians. ~0.006 rad is a
        /// twentieth of the gunner optic's 0.12 rad frame — small in absolute terms and still an
        /// unambiguous gap at every range in the layout.
        const CLEARANCE_RAD: f32 = 0.006;

        // Every tank's angular interval about the sight line, paired with its range, ordered near to
        // far — so "nearer" is "earlier".
        let mut spans: Vec<(f32, f32, f32, String)> = layout(&shipped_switches())[1..]
            .iter()
            .map(|tank| {
                let range = tank.xz.x - START_XZ.x;
                let z = tank.xz.y - START_XZ.y;
                (
                    range,
                    ((z - HULL_LENGTH_M / 2.0) / range).atan(),
                    ((z + HULL_LENGTH_M / 2.0) / range).atan(),
                    tank.name.clone(),
                )
            })
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));

        for (i, (range, lo, hi, name)) in spans.iter().enumerate() {
            for (near_range, near_lo, near_hi, near) in &spans[..i] {
                // Same range = the two halves of one pair, which are supposed to be side by side.
                if (near_range - range).abs() < 1.0 {
                    continue;
                }
                assert!(
                    near_lo - CLEARANCE_RAD > *hi || near_hi + CLEARANCE_RAD < *lo,
                    "`{near}` at {near_range:.0} m covers {near_lo:.4}..{near_hi:.4} rad and `{name}` \
                     at {range:.0} m covers {lo:.4}..{hi:.4} rad — the near tank stands in front of \
                     the far one, so the far pair cannot be judged from the spawn at all",
                );
            }
        }
    }

    /// Every tank the showcase spawns is ON THE MAP, with its footprint clearance inside the edge.
    ///
    /// The chain's last switch is past the map (1 049.9 m from a −480 start would land at +570 on a
    /// world that stops at +500), which is precisely the case a hardcoded layout gets wrong and
    /// discovers as a tank falling forever. [`MAX_RANGE_M`] is the clamp; this is the assertion that
    /// the clamp is enough, for whatever the ladder turns out to be.
    #[test]
    fn no_showcase_tank_stands_off_the_map() {
        use crate::map::tests::shipped_manifest;
        use crate::terrain_grid::SPAWN_FOOTPRINT_HALF_M;

        // The SHIPPED map's square, read off its manifest: the showcase stands on whatever world
        // ships, so a re-scaled map re-asks this question instead of leaving a stale answer.
        let half = shipped_manifest().extent.half_extent();
        for tank in layout(&shipped_switches()) {
            let edge = half - SPAWN_FOOTPRINT_HALF_M;
            assert!(
                tank.xz.x.abs() <= edge && tank.xz.y.abs() <= edge,
                "{} stands at {:?}, and its spawn footprint reaches past the {edge} m usable edge",
                tank.name,
                tank.xz,
            );
        }
    }

    /// The lever is OFF by default and REACHES only four places, enforced by scanning the source.
    ///
    /// The whole promise of a debug harness is that a build which does not ask for it does not pay
    /// for it and cannot be surprised by it. Two halves to that, and only one of them is checked by
    /// the suite passing: every other test in this crate runs with the variable unset and would go
    /// red if the showcase engaged, which covers BEHAVIOUR. What it cannot cover is REACH — a fourth
    /// site reading `enabled()` next year is one more production path with a debug branch in it, and
    /// nothing about it would fail. So the sites are enumerated here, and adding one is a deliberate
    /// edit to this list rather than a quiet spread.
    ///
    /// (The variable's NAME is likewise allowed in exactly one file. A second `env_flag` on the same
    /// string would be a second, undiscoverable definition of "enabled".)
    #[test]
    fn the_showcase_is_off_by_default_and_reaches_exactly_four_places() {
        assert!(
            !enabled(),
            "the suite runs with the lever unset — a test process that has it set is not testing \
             the shipping configuration",
        );

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut named = Vec::new();
        let mut callers = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src is readable") {
                let path = entry.expect("a readable entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("a readable source file");
                let name = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .display()
                    .to_string();
                if text.contains("OVERMATCH_LOD_SHOWCASE\"") {
                    named.push(name.clone());
                }
                if text.contains("lod_showcase::enabled()") {
                    callers.push(name);
                }
            }
        }
        named.sort();
        callers.sort();
        assert_eq!(
            named,
            ["lod_showcase.rs"],
            "the variable is read in ONE place; everything else asks `enabled()`",
        );
        assert_eq!(
            callers,
            // The spawn (pairs instead of the duel), the terrain decode (a flat grid instead of the
            // shipped map), the world's scatter call (none of the map's objects instead of 709 of
            // them), and this file — whose hit is [`plugin`]'s own mount guard AND the literal this
            // scan is written with, so it can never be absent.
            [
                "lod_showcase.rs",
                "tank/scenario.rs",
                "terrain_grid.rs",
                "world.rs"
            ],
            "the showcase reaches the scenario spawn, the terrain decode and the scatter spawn, \
             and nowhere else",
        );
    }

    /// The legend describes the scene the layout builds — same count, same distances, same levels.
    ///
    /// Two renderings of one fact drift; this is what stops the log from labelling the tanks
    /// something other than what was spawned.
    #[test]
    fn the_legend_describes_the_tanks_that_are_spawned() {
        let switches = shipped_switches();
        let deviations = shipped_deviations();
        let lines = legend(&switches, &deviations);
        let tanks = layout(&switches);
        let staged = staged_pairs(&switches).len();
        assert_eq!(lines.len(), staged + unstageable_pairs(&switches).len());

        for (slot, line) in lines.iter().take(staged).enumerate() {
            let (left, right) = (&tanks[1 + 2 * slot], &tanks[2 + 2 * slot]);
            let range = left.xz.x - START_XZ.x;
            assert!(
                line.contains(&format!("{range:.1} m")),
                "the legend must quote the range the pair actually stands at: {line}",
            );
            for level in [left.clamp, right.clamp] {
                let level = level.expect("a pair member is clamped");
                assert!(
                    line.contains(&format!("L{level}")),
                    "the legend must name both levels of the pair: {line}",
                );
            }
            assert!(
                line.contains(&format!("{:.3} mm", deviations[slot])),
                "the legend must quote the coarser level's CERTIFIED deviation — the certificate \
                 carries no triangle count: {line}",
            );
        }
        // The lines that have to explain themselves: the switches the map cannot stage at all.
        // A ladder whose deepest level opens past the map's reach is the ordinary case, and a
        // legend that simply omitted it would show a person three pairs and let them believe that
        // is the whole chain.
        let out_of_reach = lines.iter().filter(|l| l.contains("NOT staged")).count();
        assert_eq!(
            out_of_reach,
            unstageable_pairs(&switches).len(),
            "every switch past the map's reach is named, and no staged one claims to be",
        );
    }
}
