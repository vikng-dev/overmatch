//! `OVERMATCH_LOD_SHOWCASE=1`: the shoe LOD ladder laid out on the ground, PAIRED, so a human can
//! judge the switches the pipeline scored.
//!
//! # Why this exists
//!
//! `scripts/lod/generate.py` gates every level with a RENDERED-DIFFERENCE score — candidate against
//! parent, at the switch distance, under the shipped material and lighting, normalised so 0 is
//! "same image" and 1 is "as wrong as 20°-broken normals". The ladder ships with three of four
//! switches comfortably under the 0.5 the gate allows and one that is not: **L2→L3 at 501 m scores
//! 1.674**. That is a number saying the swap is visibly wrong, and a number saying that is still
//! only a number. Someone has to look.
//!
//! Looking is harder than it sounds, which is the actual reason this file exists rather than a note
//! telling a human to drive around. To see the L2→L3 switch you have to be at 501 m from a tank, on
//! ground flat enough that the tank is not half-buried, holding the optic steady, and — the part no
//! amount of driving gets you — seeing BOTH meshes at once. A switch judged by driving toward a
//! tank is judged from memory: the coarse mesh, then a blink, then the fine one, seconds apart.
//! Every rendered-difference gate in the pipeline compares them SIDE BY SIDE, and so does this.
//!
//! # What it does
//!
//! One environment variable, no knobs (a debug instrument with a settings surface is a feature, and
//! this is not one):
//!
//!   1. The terrain is FLATTENED — at the GRID, before anything reads it, so the oracle, the
//!      collider and the render mesh are all flat by the same construction that keeps them agreeing
//!      on the shipped map (`terrain_grid`'s one-surface doctrine). Flattening only the render mesh
//!      would put the tanks on invisible hills.
//!   2. The player spawns at one edge of the 1 000 m map facing down-range.
//!   3. At every switch distance in `SHOE_LOD_CHAIN`, a PAIR of stationary Tigers stands broadside
//!      to the sight line: the LEFT one clamped to the finer level, the RIGHT one to the coarser.
//!      So at each range the two tanks in frame are exactly the two meshes the gate compared, at
//!      exactly the distance it compared them.
//!   4. A legend goes to the log: one line per pair, with the levels and their triangle counts.
//!
//! Nothing here is mounted, spawned or ticked when the variable is unset — [`plugin`] adds no
//! systems at all, [`crate::tank::scenario`] takes its ordinary path, and the heightmap decodes
//! normally.
//!
//! # The clamp is the whole trick, and it is showcase-only
//!
//! A level is SELECTED by its [`VisibilityRange`], and the ranges tile `[0, ∞)` — which is exactly
//! what makes "show me L2 and L3 at the same distance" impossible on the production path, and
//! rightly so. The clamp overrides the range on one tank's shoes: the chosen level gets `[0, ∞)`
//! and every other level gets an EMPTY range, which `is_visible_at_all` reads as never
//! (`distance >= 0 && distance < 0` is false everywhere). It is written by
//! [`clamp_showcase_shoes`], which does not exist in a process without the variable, onto entities
//! found through [`crate::track::link_view::ShoeLod`], which is a tag and not a knob. No production
//! code branches on any of it, and `link_view` still owns every range it writes — the clamp asks
//! [`shoe_lod_range`] for nothing, it writes the two degenerate ranges directly, because "always"
//! and "never" are not points on the ladder.

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;

use crate::track::link_view::{
    ShoeLod, shoe_lod_levels, shoe_lod_range, shoe_lod_switch_is_judgeable, shoe_lod_tris,
};

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
/// Read by [`plugin`], by `tank::scenario`'s spawn (which lays out the pairs instead of the duel)
/// and by `terrain_grid`'s decode (which flattens the world instead of loading it). Those three are
/// the whole of its reach.
pub(crate) fn enabled() -> bool {
    crate::env_flag("OVERMATCH_LOD_SHOWCASE", false)
}

/// Where the player stands: hard against the map's west edge, on the centre line.
///
/// The map is [`crate::terrain_grid::WORLD_SIZE`] = 1 000 m across, so −480 leaves 950 m of usable
/// down-range with 20 m of shoulder behind the spawn. Down-range is +X and the pairs are laid out
/// along it; LATERAL is therefore Z, and the player's LEFT (with +Y up and +X forward) is −Z.
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
/// 15 m is also small enough that a pair is never a hunt: the furthest is 0.05 rad off the axis,
/// inside the gunner optic's own 0.12 rad frame, so panning to a pair costs a nudge rather than a
/// search.
const LANE_OFFSET_M: f32 = 15.0;

/// The furthest down-range a pair may stand, metres from [`START_XZ`].
///
/// The map runs out at +500 (`WORLD_HALF_EXTENT`) and a tank needs its footprint clearance inside
/// it, so 950 m from −480 puts the last pair at x = +470 with 30 m to spare. Any switch beyond this
/// is a switch that cannot be staged on this map — the legend says so rather than the pair silently
/// standing somewhere it was not asked to.
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
/// On the tank ROOT, read by [`clamp_showcase_shoes`] through the shoe's ancestors — so it survives
/// the rig binding, rebinding, and the belt's pool being rebuilt, none of which the showcase knows
/// about or should.
#[derive(Component, Clone, Copy)]
pub(crate) struct LodClamp(pub(crate) usize);

/// The down-range distance each PAIR stands at, and whether that is the switch's true distance.
///
/// Pair `i` compares level `i` against level `i + 1`, so it belongs at the distance level `i + 1`
/// takes over — read off [`shoe_lod_range`] rather than from any table here, because the ladder is
/// regenerated and this file must not become a second copy of it.
fn pair_range_m(pair: usize) -> (f32, Option<f32>) {
    let switch = shoe_lod_range(pair + 1).start_margin.start;
    if switch <= MAX_RANGE_M {
        (switch, None)
    } else {
        (MAX_RANGE_M, Some(switch))
    }
}

/// The switches this harness stages: those a human can actually judge.
///
/// DERIVED FROM THE MANIFEST, never listed (Yan ruling, 2026-08-07). Pair `p` compares level `p`
/// against level `p + 1`, so it is the switch INTO `p + 1`; it is staged iff the rendered-difference
/// gate had an OPINION about that level ([`shoe_lod_switch_is_judgeable`]). Where the gate abstained
/// — the asset is under the ratified 20 px floor at its own switch distance — there is by
/// construction nothing an eye could resolve, and parking two tanks out there to compare them shows
/// a person two identical specks and asks them to prefer one.
///
/// On today's three-rung ladder that drops exactly the L2|L3 pair (13.1 px at 1 017 m, on a 1 000 m
/// map it could not even be reached) and stages two. A future five-rung ladder stages whatever
/// subset of ITS switches clears the floor, with no edit here: the rule is a property of the gate's
/// verdicts, not a count.
///
/// The two-pair result is also what buys the alternating lanes their clearance — see
/// [`LANE_OFFSET_M`] and `no_pair_stands_in_front_of_another_from_the_player_spawn`.
pub(crate) fn staged_pairs() -> Vec<usize> {
    (0..shoe_lod_levels() - 1)
        .filter(|&pair| shoe_lod_switch_is_judgeable(pair + 1))
        .collect()
}

/// Which lane pair `pair` stands in: the world Z its two tanks straddle. See [`LANE_OFFSET_M`].
fn pair_lane_z(pair: usize) -> f32 {
    if pair.is_multiple_of(2) {
        LANE_OFFSET_M
    } else {
        -LANE_OFFSET_M
    }
}

/// The whole scene: the player first, then two tanks per switch in the chain.
///
/// A pure function of the chain, so the layout is a thing a test can check without a window, a
/// world or an asset load.
pub(crate) fn layout() -> Vec<ShowcaseTank> {
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
    for (slot, pair) in staged_pairs().into_iter().enumerate() {
        let (range, _) = pair_range_m(pair);
        let lane = pair_lane_z(slot);
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
pub(crate) fn legend() -> Vec<String> {
    staged_pairs()
        .into_iter()
        .enumerate()
        .map(|(slot, pair)| {
            let (range, beyond) = pair_range_m(pair);
            let note = beyond.map_or(String::new(), |switch| {
                format!(" (true switch {switch:.1} m is beyond the map edge)")
            });
            format!(
                "lod showcase: L{pair}|L{} pair at {range:.1} m{note}, {} of the sight line — \
                 LEFT L{pair} ({} tris), RIGHT L{} ({} tris)",
                pair + 1,
                if pair_lane_z(slot) < 0.0 {
                    "left"
                } else {
                    "right"
                },
                shoe_lod_tris(pair),
                pair + 1,
                shoe_lod_tris(pair + 1),
            )
        })
        .collect()
}

/// Pin the shoes of every clamped tank to their tank's level, as they appear.
///
/// `Added<ShoeLod>` rather than a sweep: shoes arrive when the rig binds, and a rig rebind despawns
/// the pool and spawns a new one, so the added filter covers the whole lifetime while costing an
/// empty query every frame after the first.
///
/// The clamp is looked up through the ANCESTORS rather than passed down from the spawn, for the
/// same reason `render_policy` resolves scopes that way: the showcase does not know how many frames
/// deep `track::view` parents its pool, and should not have to.
fn clamp_showcase_shoes(
    mut commands: Commands,
    shoes: Query<(Entity, &ShoeLod), Added<ShoeLod>>,
    parents: Query<&ChildOf>,
    clamps: Query<&LodClamp>,
) {
    for (entity, lod) in &shoes {
        let mut node = Some(entity);
        let mut clamp = None;
        while let Some(current) = node {
            if let Ok(found) = clamps.get(current) {
                clamp = Some(found.0);
                break;
            }
            node = parents.get(current).ok().map(ChildOf::parent);
        }
        let Some(clamp) = clamp else {
            continue;
        };
        // `[0, ∞)` and `[0, 0)`: bevy reads a range as `distance >= start && distance < end`, so the
        // second is EMPTY at every distance including zero. Abrupt in both cases — a crossfade
        // between "always" and "never" would dither the very silhouette being compared.
        commands.entity(entity).insert(if lod.0 == clamp {
            VisibilityRange::abrupt(0.0, f32::INFINITY)
        } else {
            VisibilityRange::abrupt(0.0, 0.0)
        });
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

    /// The layout is the CHAIN's, not a table: one pair per JUDGEABLE switch, standing at the
    /// distance that switch happens, with the finer level on the left.
    ///
    /// Driven off `shoe_lod_range`/`staged_pairs` so a regenerated ladder re-lays itself out —
    /// which is the property that makes this harness worth keeping rather than re-writing each time
    /// the meshes change.
    #[test]
    fn every_judgeable_switch_gets_a_pair_at_its_own_distance() {
        let tanks = layout();
        let staged = staged_pairs();
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
            let switch = shoe_lod_range(pair + 1).start_margin.start;
            let expected = switch.min(MAX_RANGE_M);
            assert!((left.xz.x - START_XZ.x - expected).abs() < 1e-3);
            assert!((right.xz.x - START_XZ.x - expected).abs() < 1e-3);
            let lane = START_XZ.y + pair_lane_z(slot);
            assert_eq!(left.xz.y, lane - LATERAL_HALF_M, "left is −Z");
            assert_eq!(right.xz.y, lane + LATERAL_HALF_M);
        }
    }

    /// The staging RULE, stated against the gate's verdicts rather than against a count — so this
    /// keeps meaning the same thing when the ladder is next re-cut.
    ///
    /// Every staged pair is a switch the rendered-difference gate had an opinion about, every
    /// skipped one is a switch it abstained on, and the harness is not empty. On today's ladder
    /// that is pairs 0 and 1 staged and the L2|L3 pair dropped (13.1 px at 1 017 m).
    #[test]
    fn the_harness_stages_exactly_the_switches_the_gate_could_judge() {
        let staged = staged_pairs();
        for pair in 0..shoe_lod_levels() - 1 {
            assert_eq!(
                staged.contains(&pair),
                shoe_lod_switch_is_judgeable(pair + 1),
                "pair {pair} (the switch into L{}) is staged iff the render gate judged it",
                pair + 1,
            );
        }
        assert!(
            !staged.is_empty(),
            "a showcase that stages nothing is a showcase nobody can use — if a re-cut ladder \
             ever abstains on every switch, the harness needs a new idea, not a silent empty scene",
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
        let mut spans: Vec<(f32, f32, f32, String)> = layout()[1..]
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
        use crate::terrain_grid::{SPAWN_FOOTPRINT_HALF_M, WORLD_HALF_EXTENT};

        for tank in layout() {
            let edge = WORLD_HALF_EXTENT - SPAWN_FOOTPRINT_HALF_M;
            assert!(
                tank.xz.x.abs() <= edge && tank.xz.y.abs() <= edge,
                "{} stands at {:?}, and its spawn footprint reaches past the {edge} m usable edge",
                tank.name,
                tank.xz,
            );
        }
    }

    /// The lever is OFF by default and REACHES only three places, enforced by scanning the source.
    ///
    /// The whole promise of a debug harness is that a build which does not ask for it does not pay
    /// for it and cannot be surprised by it. Two halves to that, and only one of them is checked by
    /// the suite passing: every other test in this crate runs with the variable unset and would go
    /// red if the showcase engaged, which covers BEHAVIOUR. What it cannot cover is REACH — a fourth
    /// site reading `enabled()` next year is a fourth production path with a debug branch in it, and
    /// nothing about it would fail. So the sites are enumerated here, and adding one is a deliberate
    /// edit to this list rather than a quiet spread.
    ///
    /// (The variable's NAME is likewise allowed in exactly one file. A second `env_flag` on the same
    /// string would be a second, undiscoverable definition of "enabled".)
    #[test]
    fn the_showcase_is_off_by_default_and_reaches_exactly_three_places() {
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
            // shipped map), and this file — whose hit is [`plugin`]'s own mount guard AND the
            // literal this scan is written with, so it can never be absent.
            ["lod_showcase.rs", "tank/scenario.rs", "terrain_grid.rs"],
            "the showcase reaches the scenario spawn and the terrain decode, and nowhere else",
        );
    }

    /// The legend describes the scene the layout builds — same count, same distances, same levels.
    ///
    /// Two renderings of one fact drift; this is what stops the log from labelling the tanks
    /// something other than what was spawned.
    #[test]
    fn the_legend_describes_the_tanks_that_are_spawned() {
        let lines = legend();
        let tanks = layout();
        assert_eq!(lines.len(), staged_pairs().len());

        for (slot, line) in lines.iter().enumerate() {
            let (left, right) = (&tanks[1 + 2 * slot], &tanks[2 + 2 * slot]);
            let range = left.xz.x - START_XZ.x;
            assert!(
                line.contains(&format!("{range:.1} m")),
                "the legend must quote the range the pair actually stands at: {line}",
            );
            for level in [left.clamp, right.clamp] {
                let level = level.expect("a pair member is clamped");
                assert!(
                    line.contains(&format!("L{level} ({} tris)", shoe_lod_tris(level))),
                    "the legend must quote each level's own triangle count: {line}",
                );
            }
        }
        // The one line that has to explain itself: the switch the map cannot stage.
        let beyond: Vec<_> = lines
            .iter()
            .filter(|l| l.contains("beyond the map edge"))
            .collect();
        let expected = staged_pairs()
            .into_iter()
            .filter(|&p| shoe_lod_range(p + 1).start_margin.start > MAX_RANGE_M)
            .count();
        assert_eq!(
            beyond.len(),
            expected,
            "a pair standing short of its switch must say so, and one standing at it must not",
        );
    }
}
