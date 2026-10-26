//! The tree's view: the glb a tree's root carries as a child, bound to the body by the
//! generator's node extras, and the fall presented on it from the state and its transition tick
//! (ADR-0014: the sim never waits on this; ADR-0016: the presentation derives from the state).

use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};

use super::{Fell, Resting, TreeBody, TreeKind, TreeState};
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

/// Present a fallen tree: the cut's trunk node turns about its own origin — the pivot, the
/// chain's translations carry the piece above — from upright to the resting tilt over the fall's
/// duration, phase read off [`FallClock`] against the transition tick (ADR-0037: the fall lands
/// when the cursor crosses its tick, however late the entry arrived); the caps the cut reveals
/// (A5: `cap_top` below the cut and `cap_bottom` of the cut) show. The sim never waits on this.
fn present_fall(
    clock: Res<FallClock>,
    step: Res<Time<Fixed>>,
    trees: Query<(
        &TreeState,
        &TreeBody,
        &Fell,
        &Resting,
        &Transform,
        &Children,
    )>,
    views: Query<&TreeViewNodes>,
    mut nodes: Query<(&mut Transform, &mut Visibility), Without<TreeBody>>,
) {
    let dt = step.timestep().as_secs_f32();
    for (state, body, fell, resting, transform, children) in &trees {
        let TreeState::Down { cut, .. } = *state else {
            continue;
        };
        let Some(view) = children.iter().find_map(|child| views.get(child).ok()) else {
            continue;
        };
        let cut = usize::from(cut);
        let length = body.prefix[body.prefix.len() - 1] - body.prefix[cut];
        let phase = (clock.since(fell.tick) as f32 * dt / fall_duration_s(length)).clamp(0.0, 1.0);
        let axis = transform.rotation.inverse() * Vec3::new(resting.dz, 0.0, -resting.dx);
        let angle = resting.tilt * phase * phase;
        let (s, c) = (libm::sinf(0.5 * angle), libm::cosf(0.5 * angle));
        if let Some(trunk) = view.trunks[cut]
            && let Ok((mut local, _)) = nodes.get_mut(trunk)
        {
            local.rotation = Quat::from_xyzw(axis.x * s, axis.y * s, axis.z * s, c);
        }
        let revealed = [
            cut.checked_sub(1).and_then(|below| view.caps_top[below]),
            view.caps_bottom[cut],
        ];
        for cap in revealed.into_iter().flatten() {
            if let Ok((_, mut visibility)) = nodes.get_mut(cap) {
                visibility.set_if_neq(Visibility::Inherited);
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
