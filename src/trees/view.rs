//! The tree's view: the glb a tree's root carries as a child, bound to the body by the
//! generator's node extras, and the fall presented on it from the state and its transition tick
//! (ADR-0014: the sim never waits on this; ADR-0016: the presentation derives from the state).

use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};

use super::{FallClock, Fell, Resting, TreeBody, TreeKind, TreeState, fall_rotation, seat_in_root};
use crate::{ClientReplica, ShotClock};

/// Marks the glb view's root, a child of the tree's root.
#[derive(Component)]
pub(crate) struct TreeView;

/// Attach the glb view of `kind` under the tree `root` spawned by [`super::spawn`], scaled by the
/// instance's uniform `scale` and loaded through `assets`; bound to the body once its scene is
/// ready. Called by a composition that draws, never by the sim.
pub(crate) fn attach(
    commands: &mut Commands,
    root: Entity,
    kind: &TreeKind,
    scale: f32,
    assets: &AssetServer,
) {
    let id = &kind.id;
    commands
        .spawn((
            ChildOf(root),
            Transform::from_scale(Vec3::splat(scale)),
            WorldAssetRoot(
                assets.load(GltfAssetLabel::Scene(0).from_asset(format!("trees/{id}/{id}.glb"))),
            ),
            TreeView,
            crate::render_cost::CasterClass::Tree,
        ))
        .observe(bind_tree_view);
}

/// Which glb nodes present which part of the trunk, by segment: found once on attach from each
/// node's extras (`tree_part`, `tree_segment`), the generator's contract.
#[derive(Component, Debug, Default)]
pub(crate) struct TreeViewNodes {
    trunks: Vec<Option<Entity>>,
    /// Each trunk node's translation in its parent's frame as the glb binds it, asset metres:
    /// where the node stands, which the cut's node leaves during a fall and every node returns
    /// to when the tree stands again. Captured once at bind, never read back off the node.
    standing: Vec<Vec3>,
    caps_top: Vec<Option<Entity>>,
    caps_bottom: Vec<Option<Entity>>,
}

/// The per-node extras the generator writes.
#[derive(serde::Deserialize)]
struct PartExtras {
    tree_part: String,
    tree_segment: usize,
}

/// How far a tree's primitives draw. DERIVED: a 0.4 m trunk at 600 m subtends about 1 px at the
/// gunner's 1× field; the frame-cost measurement (stage 5) decides whether this stays.
const TREE_VIEW_RANGE_M: f32 = 600.0;

/// Join an attached glb view to its tree: every cap hidden until a cut reveals it, every rendered
/// primitive given its visibility range, and the nodes indexed by segment for the fall. The state
/// is presented right after (`present_fall` reads the index the same frame it lands), so a view
/// attaching to a tree already down shows it down.
fn bind_tree_view(
    ready: On<WorldInstanceReady>,
    views: Query<&ChildOf, With<TreeView>>,
    bodies: Query<&TreeBody>,
    children: Query<&Children>,
    parts: Query<(&GltfExtras, &Transform)>,
    meshes: Query<(), With<Mesh3d>>,
    mut commands: Commands,
) {
    let Ok(parent) = views.get(ready.entity) else {
        return;
    };
    let Ok(body) = bodies.get(parent.parent()) else {
        return;
    };
    let n = body.kind.segments.len();
    let mut nodes = TreeViewNodes {
        trunks: vec![None; n],
        standing: vec![Vec3::ZERO; n],
        caps_top: vec![None; n],
        caps_bottom: vec![None; n],
    };
    for entity in children.iter_descendants(ready.entity) {
        if meshes.contains(entity) {
            commands
                .entity(entity)
                .insert(VisibilityRange::abrupt(0.0, TREE_VIEW_RANGE_M));
        }
        let Ok((extras, local)) = parts.get(entity) else {
            continue;
        };
        let Ok(part) = serde_json::from_str::<PartExtras>(&extras.value) else {
            continue;
        };
        let k = part.tree_segment;
        if k >= n {
            continue;
        }
        match part.tree_part.as_str() {
            "trunk" => {
                nodes.trunks[k] = Some(entity);
                nodes.standing[k] = local.translation;
            }
            "cap_top" => {
                nodes.caps_top[k] = Some(entity);
                commands.entity(entity).insert(Visibility::Hidden);
            }
            "cap_bottom" => {
                nodes.caps_bottom[k] = Some(entity);
                commands.entity(entity).insert(Visibility::Hidden);
            }
            _ => {}
        }
    }
    commands.entity(ready.entity).insert(nodes);
}

/// The fall's duration, seconds: longer for a longer piece, within a watchable window.
/// DERIVED (contract §6): 0.08 s per metre of fallen length, clamped to [1, 3] s.
fn fall_duration_s(fallen_length_m: f32) -> f32 {
    (0.08 * fallen_length_m).clamp(1.0, 3.0)
}

/// The butt's slide off the stump to its seat, seconds: the free fall over its drop,
/// `sqrt(2 × drop / g)` at g = 9.81 m/s² (DERIVED), within a watchable window of [0.2, 1] s.
/// A seat at or above the stump's top (an uphill fall) drops nothing and takes the floor.
fn slide_duration_s(drop_m: f32) -> f32 {
    (2.0 * drop_m / 9.81).sqrt().clamp(0.2, 1.0)
}

/// The cut's trunk node `seconds` into its fall: the angle of its turn and its translation in
/// its parent's frame, from its bound `standing` translation (asset metres). Stage A, over
/// [`fall_duration_s`] of the fallen length: the node turns about its own origin from upright to
/// the hinge with a `phase²` easing (a fall accelerates), translated as it stood. Stage B, for a
/// cut above the base, over [`slide_duration_s`] of the butt's drop from the stump's top to the
/// seat: the origin moves to its rest with `phase²` (a drop accelerates) while the turn runs
/// from the hinge to the resting tilt linearly; at its end the translation is exactly the rest
/// and the angle exactly the tilt. For a cut at the base the hinge is the tilt and the seat is
/// zero, so the fall ends with A. The rest translation is the standing one moved by the seat's
/// offset from the cut's bottom in the root's frame ([`seat_in_root`]), in asset metres: the
/// chain is under the view root's uniform scale and carries no rotation (A5), so a parent-frame
/// offset is the root-frame offset over that scale.
fn fall_pose(
    body: &TreeBody,
    resting: &Resting,
    root: Quat,
    cut: usize,
    standing: Vec3,
    seconds: f32,
) -> (f32, Vec3) {
    let tip = body.prefix.len() - 1;
    let fall = fall_duration_s(body.prefix[tip] - body.prefix[cut]);
    let phase = (seconds / fall).clamp(0.0, 1.0);
    if cut == 0 || phase < 1.0 {
        return (resting.hinge * phase * phase, standing);
    }
    let drop = (body.prefix[cut] - resting.seat.y).max(0.0);
    let slide = ((seconds - fall) / slide_duration_s(drop)).clamp(0.0, 1.0);
    let rest = standing + (seat_in_root(resting, root) - Vec3::Y * body.prefix[cut]) / body.scale;
    if slide >= 1.0 {
        return (resting.tilt, rest);
    }
    (
        resting.hinge + (resting.tilt - resting.hinge) * slide,
        standing + (rest - standing) * (slide * slide),
    )
}

/// Present a tree's state on its glb, from the state alone (ADR-0016): standing, every trunk node
/// upright at its bound translation and every cap hidden; down, the cut's trunk node — the
/// chain's translations carry the piece above — falls in two stages ([`fall_pose`]): it turns
/// about its own origin by [`fall_rotation`] from upright to the hinge, then its origin drops to
/// the seat while it turns on to the resting tilt, where its rotation is the sim's lying
/// segments' own and its origin the seat. The time is read off [`FallClock`] against the
/// transition tick (ADR-0037: the fall STARTS when the cursor crosses its tick, however late the
/// entry arrived, and completes [`fall_duration_s`] and then, above the base,
/// [`slide_duration_s`] later), and of the caps exactly the pair the cut reveals (A5: `cap_top`
/// below the cut and `cap_bottom` of the cut) shows. Written when the state, the body's resting
/// pose or the bound view is new, and on every frame the cut's trunk is not at the turn and the
/// translation the clock derives — through the fall and on the frame it completes; a view that
/// binds to a tree in either state presents it on its first frame, a tree long down with both
/// stages complete. The sim never waits on this.
fn present_fall(
    clock: Res<FallClock>,
    step: Res<Time<Fixed>>,
    trees: Query<(
        Ref<TreeState>,
        &TreeBody,
        Option<&Fell>,
        Option<Ref<Resting>>,
        &Transform,
        &Children,
    )>,
    views: Query<Ref<TreeViewNodes>>,
    mut nodes: Query<(&mut Transform, &mut Visibility), Without<TreeBody>>,
) {
    let dt = step.timestep().as_secs_f32();
    for (state, body, fell, resting, transform, children) in &trees {
        let Some(view) = children.iter().find_map(|child| views.get(child).ok()) else {
            continue;
        };
        // The cut and its trunk node's pose this frame: none standing, none until the body's
        // pose is derived.
        let turned = match (*state, fell, resting.as_deref()) {
            (TreeState::Down { cut, .. }, Some(fell), Some(resting)) => {
                let cut = usize::from(cut);
                let seconds = clock.since(fell.tick) as f32 * dt;
                let (angle, translation) = fall_pose(
                    body,
                    resting,
                    transform.rotation,
                    cut,
                    view.standing[cut],
                    seconds,
                );
                Some((
                    cut,
                    fall_rotation(resting, transform.rotation, angle),
                    translation,
                ))
            }
            _ => None,
        };
        let fresh = state.is_changed()
            || view.is_added()
            || resting.as_ref().is_some_and(|resting| resting.is_changed());
        // An unchanged tree is redrawn while its cut's trunk is not at the pose this frame
        // derives: every frame of the fall, and the frame the last stage completes, which writes
        // the resting pose; at rest the two agree and the tree is skipped.
        let behind = turned.is_some_and(|(cut, turn, translation)| {
            view.trunks[cut]
                .and_then(|trunk| nodes.get(trunk).ok())
                .is_some_and(|(local, _)| {
                    local.rotation != turn || local.translation != translation
                })
        });
        if !fresh && !behind {
            continue;
        }
        for (k, trunk) in view.trunks.iter().enumerate() {
            let (rotation, translation) = match turned {
                Some((cut, turn, translation)) if k == cut => (turn, translation),
                _ => (Quat::IDENTITY, view.standing[k]),
            };
            if let Some(trunk) = trunk
                && let Ok((mut local, _)) = nodes.get_mut(*trunk)
            {
                if local.rotation != rotation {
                    local.rotation = rotation;
                }
                if local.translation != translation {
                    local.translation = translation;
                }
            }
        }
        let cut = turned.map(|(cut, _, _)| cut);
        let tops = view
            .caps_top
            .iter()
            .enumerate()
            .map(|(k, cap)| (cap, cut.is_some_and(|cut| k + 1 == cut)));
        let bottoms = view
            .caps_bottom
            .iter()
            .enumerate()
            .map(|(k, cap)| (cap, cut == Some(k)));
        for (cap, shown) in tops.chain(bottoms) {
            if let Some(cap) = cap
                && let Ok((_, mut visibility)) = nodes.get_mut(*cap)
            {
                let visibility_target = if shown {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
                visibility.set_if_neq(visibility_target);
            }
        }
    }
}

/// The authority's (and single-player's) presentation clock: the sim tick plus the fixed step's
/// overstep. A replica's is the interpolation cursor, written by the net client instead.
fn publish_sim_fall_clock(
    tick: Option<Res<ShotClock>>,
    step: Res<Time<Fixed>>,
    mut clock: ResMut<FallClock>,
) {
    // A composition with no clock (the sandboxes, the headless sim) keeps its falls unkeyed, as
    // its shells are.
    let Some(tick) = tick else {
        return;
    };
    clock.tick = tick.0;
    clock.overstep = step.overstep_fraction_f64();
}

/// The view half, mounted by the windowed roots: the presentation clock and the fall on the glb.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<FallClock>()
        .add_systems(
            Update,
            publish_sim_fall_clock.run_if(not(resource_exists::<ClientReplica>)),
        )
        .add_systems(PostUpdate, present_fall.before(TransformSystems::Propagate));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trees::tests::{derive_bed, fixture_grid, fixture_kind, segments_of};
    use std::f32::consts::FRAC_PI_2;

    /// The fixture's view under a tree root at `pose` (the root's own transform, unit scale) in
    /// `state` with `resting`, presented at clock tick `tick` after a transition at tick 0: the
    /// view's root under the tree at the instance's `scale`, the trunk nodes nested under it
    /// bottom-up, each at its segment's base in its parent's frame in asset metres (fixture
    /// data, the glb's nesting); the trunk nodes' world rotations and positions composed down
    /// the chain, bottom first.
    fn presented(
        pose: Transform,
        scale: f32,
        state: TreeState,
        resting: Resting,
        tick: u32,
    ) -> Vec<(Quat, Vec3)> {
        let mut app = App::new();
        app.insert_resource(FallClock {
            tick,
            overstep: 0.0,
        })
        .insert_resource(Time::<Fixed>::from_hz(64.0))
        .add_systems(Update, present_fall);
        let body = TreeBody::new(fixture_kind(), scale);
        let world = app.world_mut();
        let root = world
            .spawn((state, Fell { tick: 0 }, resting, body.clone(), pose))
            .id();
        let view = world
            .spawn((ChildOf(root), Transform::from_scale(Vec3::splat(scale))))
            .id();
        let mut parent = view;
        let mut trunks = Vec::new();
        let mut standing = Vec::new();
        for k in 0..body.kind.segments.len() {
            let lift = if k == 0 {
                0.0
            } else {
                body.kind.segments[k - 1].height_m
            };
            let translation = Vec3::new(0.0, lift, 0.0);
            parent = world
                .spawn((
                    ChildOf(parent),
                    Transform::from_translation(translation),
                    Visibility::Inherited,
                ))
                .id();
            trunks.push(parent);
            standing.push(translation);
        }
        let n = trunks.len();
        world.entity_mut(view).insert(TreeViewNodes {
            trunks: trunks.iter().copied().map(Some).collect(),
            standing,
            caps_top: vec![None; n],
            caps_bottom: vec![None; n],
        });
        app.update();
        let world = app.world();
        let mut chain = pose.mul_transform(*world.get::<Transform>(view).unwrap());
        trunks
            .iter()
            .map(|trunk| {
                chain = chain.mul_transform(*world.get::<Transform>(*trunk).unwrap());
                (chain.rotation, chain.translation)
            })
            .collect()
    }

    /// THE SIM AND THE VIEW LAY THE PIECE DOWN TO THE SAME BITS: for every row of the lying-pose
    /// pins (cuts 0 and 1 on flat ground, on the slope and at the grid's edge, in three bins,
    /// under a yaw), for the yawed fixture down at cut 2 and for one at scale 1.5, each lying
    /// segment's world rotation composed from the sim's body — the root's over the segment's —
    /// equals, bit for bit, its trunk node's composed down the view's chain at rest, and each
    /// lying segment's world position is within 1e-4 m of its node's (DERIVED: the chain's sum
    /// and the scale division round differently from the sim's one addition; positions are
    /// root-relative, tens of metres, where an f32 ulp is under 1e-5). Both take
    /// [`fall_rotation`] and [`seat_in_root`] under the same root; a sim or a view that builds
    /// its turn by another route — the world tilt conjugated by the root's inverse, say —
    /// differs in the last bits, a view that leaves the butt on the stump is `2r` off, and a
    /// rest translation not divided by the scale is off by the seat's offset times the scale's
    /// excess; each reds this.
    #[test]
    fn the_sim_and_the_view_lay_the_piece_down_to_the_same_bits() {
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        let flat: fn(f32, f32) -> f32 = |_, _| 0.0;
        let slope: fn(f32, f32) -> f32 = |x, _| x / 10.0;
        let mut cases = Vec::new();
        for (height, base) in [
            (flat, Vec3::ZERO),
            (slope, Vec3::new(100.0, 10.0, 0.0)),
            (slope, Vec3::new(499.0, 49.9, 0.0)),
        ] {
            for cut in [0_u8, 1] {
                for dir in [0_u8, 64, 200] {
                    cases.push((height, base, 1.0_f32, TreeState::Down { cut, dir }));
                }
            }
        }
        cases.push((
            flat,
            Vec3::new(5.0, 0.0, 5.0),
            1.0,
            TreeState::Down { cut: 2, dir: 0 },
        ));
        cases.push((
            flat,
            Vec3::new(5.0, 0.0, 5.0),
            1.5,
            TreeState::Down { cut: 1, dir: 64 },
        ));
        for (height, base, scale, state) in cases {
            let spawn_pose = Transform {
                translation: base,
                rotation: yaw,
                scale: Vec3::splat(scale),
            };
            let (mut app, root) = derive_bed(fixture_grid(height), spawn_pose);
            *app.world_mut().get_mut::<TreeState>(root).unwrap() = state;
            app.update();
            let resting = *app.world().get::<Resting>(root).expect("the pose");
            let root_pose = Transform {
                scale: Vec3::ONE,
                ..spawn_pose
            };
            let sim: Vec<(Quat, Vec3)> = segments_of(&app, root)
                .iter()
                .map(|(local, _)| {
                    let world = root_pose.mul_transform(*local);
                    (world.rotation, world.translation)
                })
                .collect();
            let view = presented(root_pose, scale, state, resting, 1000);
            let TreeState::Down { cut, .. } = state else {
                unreachable!()
            };
            for k in usize::from(cut)..sim.len() {
                assert_eq!(
                    sim[k].0.to_array().map(f32::to_bits),
                    view[k].0.to_array().map(f32::to_bits),
                    "{state:?} at {base}, scale {scale}, segment {k}: rotation"
                );
                assert!(
                    sim[k].1.abs_diff_eq(view[k].1, 1e-4),
                    "{state:?} at {base}, scale {scale}, segment {k}: the sim at {}, the view at {}",
                    sim[k].1,
                    view[k].1
                );
            }
        }
    }

    /// THE FALL ENDS ON THE RESTING POSE, STAGE BY STAGE: an unchanged tree whose clock advances
    /// through its fall has its cut's trunk node, through stage A, at its bound translation with
    /// a partial turn, and at A's end exactly the hinge's turn; through stage B, between the
    /// bound translation and the rest with a turn between the hinge and the tilt; and from B's
    /// end, at exactly the rest translation and exactly the turn of the whole tilt — the bits the
    /// derive computes at rest, which the sim's lying piece shares. A seat no lower than the
    /// stump's top drops nothing, and B takes its 0.2 s floor. A presentation that stops
    /// redrawing once a phase reaches one, before writing that frame's pose, leaves the node
    /// short of the rest; one that moves the origin during A reds this, and one that ends B at
    /// `hinge + (tilt − hinge)` instead of the tilt itself reds the third case, whose pair —
    /// hinge 90°, tilt 0.3 rad, fixture data — is a DERIVED f32 counterexample: that sum reads
    /// one ulp above 0.3. Mid-B the origin drops with `phase²` and the turn runs linearly from
    /// the hinge to the tilt, read off the tick count independently of the presentation's own
    /// arithmetic; a linear drop, a quadratic turn or a reversed trajectory reds that frame.
    #[test]
    fn a_fall_ends_on_the_resting_pose() {
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        let body = TreeBody::new(fixture_kind(), 1.0);
        // The fixture's piece above cut 1 is 4 m, a one-second stage A: 64 ticks at 64 Hz
        // (DERIVED). Its butt drops 1.8 m from the stump's 2 m top to a seat 0.2 m up in
        // 0.606 s, 38.8 ticks (DERIVED: sqrt(2 × 1.8 / 9.81)); a seat 2.5 m up drops nothing,
        // 0.2 s, 12.8 ticks. The third case is the first with the rounding-sensitive tilt.
        for (seat_y, b_ticks, tilt) in [(0.2_f32, 39_u32, 1.6_f32), (2.5, 13, 1.6), (0.2, 39, 0.3)]
        {
            let resting = Resting {
                hinge: FRAC_PI_2,
                tilt,
                seat: Vec3::new(0.4, seat_y, 0.0),
                dx: 1.0,
                dz: 0.0,
            };
            let mut app = App::new();
            app.insert_resource(FallClock {
                tick: 0,
                overstep: 0.0,
            })
            .insert_resource(Time::<Fixed>::from_hz(64.0))
            .add_systems(Update, present_fall);
            let world = app.world_mut();
            let root = world
                .spawn((
                    TreeState::Down { cut: 1, dir: 0 },
                    Fell { tick: 0 },
                    resting,
                    body.clone(),
                    Transform::from_rotation(yaw),
                ))
                .id();
            let view = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
            let standing: Vec<Vec3> = (0..3)
                .map(|k| Vec3::new(0.0, 2.0 * k as f32, 0.0))
                .collect();
            let trunks: Vec<Entity> = standing
                .iter()
                .map(|translation| {
                    world
                        .spawn((
                            ChildOf(view),
                            Transform::from_translation(*translation),
                            Visibility::Inherited,
                        ))
                        .id()
                })
                .collect();
            world.entity_mut(view).insert(TreeViewNodes {
                trunks: trunks.iter().copied().map(Some).collect(),
                standing: standing.clone(),
                caps_top: vec![None; 3],
                caps_bottom: vec![None; 3],
            });
            let node = |app: &App| *app.world().get::<Transform>(trunks[1]).unwrap();
            let at = |app: &mut App, tick: u32| {
                app.world_mut().resource_mut::<FallClock>().tick = tick;
                app.update();
            };
            let bits = |rotation: Quat| rotation.to_array().map(f32::to_bits);
            let hinge_turn = fall_rotation(&resting, yaw, resting.hinge);
            let rest_turn = fall_rotation(&resting, yaw, resting.tilt);
            let rest =
                standing[1] + (seat_in_root(&resting, yaw) - Vec3::Y * body.prefix[1]) / body.scale;
            at(&mut app, 32);
            let mid_a = node(&app);
            assert_eq!(
                mid_a.translation, standing[1],
                "stage A: the node stands where it was bound"
            );
            assert!(
                mid_a.rotation != Quat::IDENTITY && mid_a.rotation != hinge_turn,
                "stage A: a partial turn"
            );
            at(&mut app, 32);
            assert_eq!(
                node(&app),
                mid_a,
                "a frame with no time passed redraws nothing"
            );
            at(&mut app, 64);
            assert_eq!(
                bits(node(&app).rotation),
                bits(hinge_turn),
                "A's end: the hinge"
            );
            assert_eq!(node(&app).translation, standing[1]);
            at(&mut app, 64 + b_ticks / 2);
            let mid_b = node(&app);
            let drop = (body.prefix[1] - seat_y).max(0.0);
            let slide = ((64 + b_ticks / 2) as f32 / 64.0 - 1.0) / slide_duration_s(drop);
            let dropped = standing[1] + (rest - standing[1]) * (slide * slide);
            let turning = fall_rotation(
                &resting,
                yaw,
                resting.hinge + (resting.tilt - resting.hinge) * slide,
            );
            assert!(
                mid_b.translation.abs_diff_eq(dropped, 1e-5),
                "seat {seat_y}: stage B drops the origin with phase²: {} for {dropped}",
                mid_b.translation
            );
            assert!(
                mid_b.rotation.abs_diff_eq(turning, 1e-5),
                "seat {seat_y}: stage B turns linearly from the hinge to the tilt: {} for {turning}",
                mid_b.rotation
            );
            at(&mut app, 64 + b_ticks - 1);
            assert_ne!(
                node(&app).translation,
                rest,
                "seat {seat_y}: a tick short of B's end is short of the rest"
            );
            at(&mut app, 64 + b_ticks);
            let landed = node(&app);
            assert_eq!(
                landed.translation.to_array().map(f32::to_bits),
                rest.to_array().map(f32::to_bits),
                "seat {seat_y}: the frame B completes writes the rest translation"
            );
            assert_eq!(
                bits(landed.rotation),
                bits(rest_turn),
                "seat {seat_y}: and the resting turn"
            );
            at(&mut app, 300);
            assert_eq!(node(&app), landed, "and the node stays there");
        }
    }

    /// THE GLB PRESENTS THE STATE AND NOTHING BEFORE IT: standing, every trunk node upright at
    /// its bound translation and every cap hidden; down at a cut, that trunk node alone turned
    /// to the resting tilt and moved to its seat (the fall long landed) and exactly the cap pair
    /// the cut reveals shown; back to standing, upright at the bound translation and hidden
    /// again; down at another cut, the old trunk upright and in place and the old caps hidden.
    /// A presentation that only reveals, only turns the current cut's node, leaves a node where
    /// the fall put it, or skips `Standing` reds this.
    #[test]
    fn the_glb_presents_every_state_from_the_state() {
        let mut app = App::new();
        app.insert_resource(FallClock {
            tick: 1000,
            overstep: 0.0,
        })
        .insert_resource(Time::<Fixed>::from_hz(64.0))
        .add_systems(Update, present_fall);
        let world = app.world_mut();
        let root = world
            .spawn((
                TreeState::Standing,
                TreeBody::new(fixture_kind(), 1.0),
                Transform::IDENTITY,
            ))
            .id();
        let view = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
        let node = |world: &mut World, translation: Vec3| {
            world
                .spawn((
                    ChildOf(view),
                    Transform::from_translation(translation),
                    Visibility::Inherited,
                ))
                .id()
        };
        let standing: Vec<Vec3> = (0..3)
            .map(|k| Vec3::new(0.0, 2.0 * k as f32, 0.0))
            .collect();
        let trunks: Vec<Entity> = standing.iter().map(|at| node(world, *at)).collect();
        let caps_top: Vec<Entity> = (0..3).map(|_| node(world, Vec3::ZERO)).collect();
        let caps_bottom: Vec<Entity> = (0..3).map(|_| node(world, Vec3::ZERO)).collect();
        world.entity_mut(view).insert(TreeViewNodes {
            trunks: trunks.iter().copied().map(Some).collect(),
            standing: standing.clone(),
            caps_top: caps_top.iter().copied().map(Some).collect(),
            caps_bottom: caps_bottom.iter().copied().map(Some).collect(),
        });
        let shown = |app: &App, caps: &[Entity]| -> Vec<bool> {
            caps.iter()
                .map(|cap| app.world().get::<Visibility>(*cap) == Some(&Visibility::Inherited))
                .collect()
        };
        let turned = |app: &App| -> Vec<bool> {
            trunks
                .iter()
                .map(|trunk| {
                    app.world().get::<Transform>(*trunk).unwrap().rotation != Quat::IDENTITY
                })
                .collect()
        };
        let moved = |app: &App| -> Vec<bool> {
            trunks
                .iter()
                .zip(&standing)
                .map(|(trunk, at)| app.world().get::<Transform>(*trunk).unwrap().translation != *at)
                .collect()
        };
        app.update();
        assert_eq!(shown(&app, &caps_top), [false; 3], "standing: no cap");
        assert_eq!(shown(&app, &caps_bottom), [false; 3]);
        assert_eq!(turned(&app), [false; 3], "standing: upright");
        assert_eq!(moved(&app), [false; 3], "standing: in place");

        // Standing again leaves the last tick and pose on the root: the derive that drops them is
        // not mounted here, and the presentation reads the state first.
        let set = |app: &mut App, state: TreeState, resting: Option<Resting>| {
            let mut tree = app.world_mut().entity_mut(root);
            tree.insert(state);
            if let Some(resting) = resting {
                tree.insert((Fell { tick: 0 }, resting));
            }
            app.update();
        };
        let east = Resting {
            hinge: FRAC_PI_2,
            tilt: 1.2,
            seat: Vec3::new(0.4, 0.2, 0.0),
            dx: 1.0,
            dz: 0.0,
        };
        set(&mut app, TreeState::Down { cut: 1, dir: 0 }, Some(east));
        assert_eq!(turned(&app), [false, true, false], "cut 1: its trunk turns");
        assert_eq!(
            moved(&app),
            [false, true, false],
            "cut 1: its trunk alone leaves the stump"
        );
        let turn = app.world().get::<Transform>(trunks[1]).unwrap().rotation;
        assert!(
            (turn * Vec3::Y).abs_diff_eq(Vec3::new(libm::sinf(1.2), libm::cosf(1.2), 0.0), 1e-5),
            "the trunk's axis leans to +x by the tilt: {}",
            turn * Vec3::Y
        );
        assert_eq!(
            shown(&app, &caps_top),
            [true, false, false],
            "cap_top below the cut"
        );
        assert_eq!(
            shown(&app, &caps_bottom),
            [false, true, false],
            "cap_bottom of the cut"
        );

        set(&mut app, TreeState::Standing, None);
        assert_eq!(turned(&app), [false; 3], "standing again: upright");
        assert_eq!(
            moved(&app),
            [false; 3],
            "standing again: at the bound translation"
        );
        assert_eq!(shown(&app, &caps_top), [false; 3], "standing again: no cap");
        assert_eq!(shown(&app, &caps_bottom), [false; 3]);

        set(
            &mut app,
            TreeState::Down { cut: 0, dir: 64 },
            Some(Resting {
                hinge: 1.0,
                tilt: 1.0,
                seat: Vec3::ZERO,
                dx: 0.0,
                dz: 1.0,
            }),
        );
        assert_eq!(moved(&app), [false; 3], "cut 0: the butt stays at the base");
        assert_eq!(
            turned(&app),
            [true, false, false],
            "cut 0: the first trunk turns"
        );
        assert_eq!(
            shown(&app, &caps_top),
            [false; 3],
            "cut 0 reveals no cap_top"
        );
        assert_eq!(shown(&app, &caps_bottom), [true, false, false]);

        set(&mut app, TreeState::Down { cut: 2, dir: 0 }, Some(east));
        assert_eq!(
            turned(&app),
            [false, false, true],
            "cut 2: the old trunk stands again"
        );
        assert_eq!(shown(&app, &caps_top), [false, true, false]);
        assert_eq!(shown(&app, &caps_bottom), [false, false, true]);
    }
}
