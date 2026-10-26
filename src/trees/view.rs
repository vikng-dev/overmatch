//! The tree's view: the glb a tree's root carries as a child, bound to the body by the
//! generator's node extras, and the fall presented on it from the state and its transition tick
//! (ADR-0014: the sim never waits on this; ADR-0016: the presentation derives from the state).

use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};

use super::{Fell, Resting, TreeBody, TreeKind, TreeState, fall_rotation};
use crate::{ClientReplica, ShotClock};

/// Marks the glb view's root, a child of the tree's root.
#[derive(Component)]
pub(crate) struct TreeView;

/// Presentation time in authority ticks: the interpolation cursor on a replica (the net client
/// writes it), the sim tick plus the fixed step's overstep elsewhere. The authority's tick is a
/// wrapping 32-bit count (`ShotClock`), so a phase is a wrapped signed difference.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub(crate) struct FallClock {
    pub(crate) tick: u32,
    pub(crate) overstep: f64,
}

impl FallClock {
    /// Fractional ticks since `since`, modulo the 32-bit timeline (negative before it).
    pub(crate) fn since(self, since: u32) -> f64 {
        f64::from(self.tick.wrapping_sub(since) as i32) + self.overstep
    }
}

/// Attach the glb view of `kind` under `root`, scaled by the instance's uniform `scale` and loaded
/// through `assets`; bound to the body once its scene is ready.
pub(super) fn attach(
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
        ))
        .observe(bind_tree_view);
}

/// Which glb nodes present which part of the trunk, by segment: found once on attach from each
/// node's extras (`tree_part`, `tree_segment`), the generator's contract.
#[derive(Component, Debug, Default)]
pub(crate) struct TreeViewNodes {
    trunks: Vec<Option<Entity>>,
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
    extras: Query<&GltfExtras>,
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
        caps_top: vec![None; n],
        caps_bottom: vec![None; n],
    };
    for entity in children.iter_descendants(ready.entity) {
        if meshes.contains(entity) {
            commands
                .entity(entity)
                .insert(VisibilityRange::abrupt(0.0, TREE_VIEW_RANGE_M));
        }
        let Ok(extras) = extras.get(entity) else {
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
            "trunk" => nodes.trunks[k] = Some(entity),
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

/// Present a tree's state on its glb, from the state alone (ADR-0016): standing, every trunk node
/// upright and every cap hidden; down, the cut's trunk node turns about its own origin — the
/// pivot, the chain's translations carry the piece above — by [`fall_rotation`] from upright to
/// the resting tilt, where it is the sim's lying segments' own rotation, over the fall's duration, phase read off [`FallClock`] against the transition tick (ADR-0037: the
/// fall lands when the cursor crosses its tick, however late the entry arrived), and of the caps
/// exactly the pair the cut reveals (A5: `cap_top` below the cut and `cap_bottom` of the cut)
/// shows. Written when the state, the body's resting pose or the bound view is new, and on every
/// frame the cut's trunk is not at the turn the clock derives — through the fall and on the frame
/// it completes; a view that binds to a tree in either state presents it on its first frame. The
/// sim never waits on this.
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
        // The cut and its trunk's turn: none standing, none until the body's pose is derived.
        let turned = match (*state, fell, resting.as_deref()) {
            (TreeState::Down { cut, .. }, Some(fell), Some(resting)) => {
                let cut = usize::from(cut);
                let length = body.prefix[body.prefix.len() - 1] - body.prefix[cut];
                let phase =
                    (clock.since(fell.tick) as f32 * dt / fall_duration_s(length)).clamp(0.0, 1.0);
                Some((
                    cut,
                    fall_rotation(resting, transform.rotation, resting.tilt * phase * phase),
                ))
            }
            _ => None,
        };
        let fresh = state.is_changed()
            || view.is_added()
            || resting.as_ref().is_some_and(|resting| resting.is_changed());
        // An unchanged tree is redrawn while its cut's trunk is not at the turn this frame derives:
        // every frame of the fall, and the frame the phase completes, which writes the resting
        // pose; at rest the two agree and the tree is skipped.
        let behind = turned.is_some_and(|(cut, turn)| {
            view.trunks[cut]
                .and_then(|trunk| nodes.get(trunk).ok())
                .is_some_and(|(local, _)| local.rotation != turn)
        });
        if !fresh && !behind {
            continue;
        }
        for (k, trunk) in view.trunks.iter().enumerate() {
            let rotation = match turned {
                Some((cut, turn)) if k == cut => turn,
                _ => Quat::IDENTITY,
            };
            if let Some(trunk) = trunk
                && let Ok((mut local, _)) = nodes.get_mut(*trunk)
                && local.rotation != rotation
            {
                local.rotation = rotation;
            }
        }
        let cut = turned.map(|(cut, _)| cut);
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

    /// The fixture's view at rest under a root at `pose` in `state` with `resting`: the view's
    /// root under the tree, the trunk nodes nested under it bottom-up, each at its segment's base
    /// in its parent's frame (fixture data, the glb's nesting), presented long after the fall;
    /// the trunk nodes' world rotations composed down the chain, bottom first.
    fn presented_at_rest(pose: Transform, state: TreeState, resting: Resting) -> Vec<Quat> {
        let mut app = App::new();
        app.insert_resource(FallClock {
            tick: 1000,
            overstep: 0.0,
        })
        .insert_resource(Time::<Fixed>::from_hz(64.0))
        .add_systems(Update, present_fall);
        let body = TreeBody::new(fixture_kind(), 1.0);
        let world = app.world_mut();
        let root = world
            .spawn((state, Fell { tick: 0 }, resting, body.clone(), pose))
            .id();
        let view = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
        let mut parent = view;
        let mut trunks = Vec::new();
        for k in 0..body.kind.segments.len() {
            let lift = if k == 0 {
                0.0
            } else {
                body.prefix[k] - body.prefix[k - 1]
            };
            parent = world
                .spawn((
                    ChildOf(parent),
                    Transform::from_xyz(0.0, lift, 0.0),
                    Visibility::Inherited,
                ))
                .id();
            trunks.push(parent);
        }
        let n = trunks.len();
        world.entity_mut(view).insert(TreeViewNodes {
            trunks: trunks.iter().copied().map(Some).collect(),
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
                chain.rotation
            })
            .collect()
    }

    /// THE SIM AND THE VIEW LAY THE PIECE DOWN TO THE SAME BITS: for every row of the lying-pose
    /// pins (the whole trunk, cut 0, on flat ground, on the slope and at the grid's edge, in
    /// three bins) and for the yawed fixture down at cut 1, each segment's world rotation
    /// composed from the sim's body — the root's over the segment's — equals, bit for bit, its
    /// trunk node's composed down the view's chain at rest. Both take [`fall_rotation`] under the
    /// same root; a sim or a view that builds its turn by another route — the world tilt
    /// conjugated by the root's inverse, say — differs in the last bits and reds this.
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
            for dir in [0_u8, 64, 200] {
                cases.push((height, base, TreeState::Down { cut: 0, dir }));
            }
        }
        cases.push((
            flat,
            Vec3::new(5.0, 0.0, 5.0),
            TreeState::Down { cut: 1, dir: 0 },
        ));
        for (height, base, state) in cases {
            let pose = Transform {
                translation: base,
                rotation: yaw,
                scale: Vec3::ONE,
            };
            let (mut app, root) = derive_bed(fixture_grid(height), pose);
            *app.world_mut().get_mut::<TreeState>(root).unwrap() = state;
            app.update();
            let resting = *app.world().get::<Resting>(root).expect("the pose");
            let sim: Vec<[u32; 4]> = segments_of(&app, root)
                .iter()
                .map(|(local, _)| {
                    pose.mul_transform(*local)
                        .rotation
                        .to_array()
                        .map(f32::to_bits)
                })
                .collect();
            let view: Vec<[u32; 4]> = presented_at_rest(pose, state, resting)
                .iter()
                .map(|rotation| rotation.to_array().map(f32::to_bits))
                .collect();
            assert_eq!(sim, view, "{state:?} at {base}");
        }
    }

    /// THE FALL ENDS ON THE RESTING POSE: an unchanged tree whose clock advances from an
    /// unfinished phase through the fall's end leaves its cut's trunk at exactly the turn of the
    /// whole tilt — the bits the derive computes at rest, which the sim's lying piece shares. A
    /// presentation that stops redrawing once the phase reaches one, before writing that frame's
    /// pose, leaves the trunk short of the ground by `1 − phase²` of the tilt.
    #[test]
    fn a_fall_ends_on_the_resting_pose() {
        let mut app = App::new();
        app.insert_resource(FallClock {
            tick: 0,
            overstep: 0.0,
        })
        .insert_resource(Time::<Fixed>::from_hz(64.0))
        .add_systems(Update, present_fall);
        let resting = Resting {
            tilt: 1.2,
            dx: 1.0,
            dz: 0.0,
        };
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        let world = app.world_mut();
        let root = world
            .spawn((
                TreeState::Down { cut: 1, dir: 0 },
                Fell { tick: 0 },
                resting,
                TreeBody::new(crate::trees::tests::fixture_kind(), 1.0),
                Transform::from_rotation(yaw),
            ))
            .id();
        let view = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
        let trunks: Vec<Entity> = (0..3)
            .map(|_| {
                world
                    .spawn((ChildOf(view), Transform::IDENTITY, Visibility::Inherited))
                    .id()
            })
            .collect();
        world.entity_mut(view).insert(TreeViewNodes {
            trunks: trunks.iter().copied().map(Some).collect(),
            caps_top: vec![None; 3],
            caps_bottom: vec![None; 3],
        });
        let turn = |app: &App| app.world().get::<Transform>(trunks[1]).unwrap().rotation;
        // The fixture's fallen piece is 4 m, a one-second fall: 64 ticks at 64 Hz (DERIVED).
        let at = |app: &mut App, tick: u32| {
            app.world_mut().resource_mut::<FallClock>().tick = tick;
            app.update();
        };
        at(&mut app, 63);
        let unfinished = turn(&app);
        at(&mut app, 63);
        assert_eq!(
            turn(&app),
            unfinished,
            "a frame with no time passed redraws nothing"
        );
        at(&mut app, 64);
        let rest = fall_rotation(&resting, yaw, resting.tilt);
        assert_ne!(unfinished, rest, "63/64 of the fall is short of the rest");
        assert_eq!(
            turn(&app).to_array().map(f32::to_bits),
            rest.to_array().map(f32::to_bits),
            "the frame the phase completes writes the resting pose"
        );
        at(&mut app, 200);
        assert_eq!(
            turn(&app).to_array().map(f32::to_bits),
            rest.to_array().map(f32::to_bits),
            "and the trunk stays there"
        );
    }

    /// THE GLB PRESENTS THE STATE AND NOTHING BEFORE IT: standing, every trunk node upright and
    /// every cap hidden; down at a cut, that trunk node alone turned to the resting tilt (the fall
    /// long landed) and exactly the cap pair the cut reveals shown; back to standing, upright and
    /// hidden again; down at another cut, the old trunk upright and the old caps hidden. A
    /// presentation that only reveals, only turns the current cut's node, or skips `Standing`
    /// reds this.
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
                TreeBody::new(crate::trees::tests::fixture_kind(), 1.0),
                Transform::IDENTITY,
            ))
            .id();
        let view = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
        let node = |world: &mut World| {
            world
                .spawn((ChildOf(view), Transform::IDENTITY, Visibility::Inherited))
                .id()
        };
        let trunks: Vec<Entity> = (0..3).map(|_| node(world)).collect();
        let caps_top: Vec<Entity> = (0..3).map(|_| node(world)).collect();
        let caps_bottom: Vec<Entity> = (0..3).map(|_| node(world)).collect();
        world.entity_mut(view).insert(TreeViewNodes {
            trunks: trunks.iter().copied().map(Some).collect(),
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
        app.update();
        assert_eq!(shown(&app, &caps_top), [false; 3], "standing: no cap");
        assert_eq!(shown(&app, &caps_bottom), [false; 3]);
        assert_eq!(turned(&app), [false; 3], "standing: upright");

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
            tilt: 1.2,
            dx: 1.0,
            dz: 0.0,
        };
        set(&mut app, TreeState::Down { cut: 1, dir: 0 }, Some(east));
        assert_eq!(turned(&app), [false, true, false], "cut 1: its trunk turns");
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
        assert_eq!(shown(&app, &caps_top), [false; 3], "standing again: no cap");
        assert_eq!(shown(&app, &caps_bottom), [false; 3]);

        set(
            &mut app,
            TreeState::Down { cut: 0, dir: 64 },
            Some(Resting {
                tilt: 1.0,
                dx: 0.0,
                dz: 1.0,
            }),
        );
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
