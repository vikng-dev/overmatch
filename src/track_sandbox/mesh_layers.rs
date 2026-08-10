//! Semantic mesh layering for the track sandbox — the multi-state inspection view the ballistics
//! sandbox ([`crate::sandbox`]) pioneered, ported to the driving rig.
//!
//! The raw `WorldAssetRoot` scene spawn instantiates the asset's authored `*_Collider` /
//! `*_Ballistic` volume nodes verbatim (the game's `bind_tank_view` filter never runs here, because
//! the sandbox does not spawn through `SimParts`), so without layering they render as one opaque
//! pile. This module gives each mesh class its own layer with the ballistics-sandbox state model:
//!
//!   * **hull** (the `*_Visual` render meshes) → [`MeshState`]: solid → x-ray → hidden.
//!   * **collider volumes** (`*_Collider` proxy meshes, amber) → [`VolumeState`]: off → on-top →
//!     solid → x-ray.
//!   * **ballistic volumes** (`*_Ballistic` armour/component meshes, steel-blue) → [`VolumeState`].
//!   * **world** (the sandbox's terrain / course / obstacle meshes — everything NOT under the tank
//!     root) → [`MeshState`]: solid → x-ray → hidden.
//!
//! # Tank subtree vs. world
//!
//! The hull fallback is scoped to the TANK by a real ancestry check, not a name heuristic: [`classify`]
//! walks the `ChildOf` chain and a walk that reaches the [`Hull`] body with no role name is a hull
//! visual, while a walk that runs off the top of the tree without passing through [`Hull`] is a WORLD
//! mesh (the course slabs/obstacles [`super::spawn_environment`] spawns at the scene root). The scope
//! is what keeps x-raying the hull from also ghosting the terrain: without it every parent-less
//! course mesh falls through the walk to `Hull`. World meshes are visibility/material only; their
//! static colliders are never touched.
//!
//! The running gear (wheels/sprocket/idler) and the instanced track links are NOT layered here —
//! they are moving simulation views owned by [`super::wheel_view`] / [`super::link_view`] and their
//! own `bool` switches, so this module's tagger classifies them `Skip` and never touches their
//! visibility or material.
//!
//! # Reusing the glb meshes, not rebuilding from proxy data
//!
//! The rendered volumes are the glb node meshes the scene already instantiated, not fresh meshes
//! built from `blueprint.geometry.collision_proxies`. Two reasons: only the two `*_Collider` nodes
//! have a physics-proxy position buffer at all (the ~30 `*_Ballistic` nodes are render-only in this
//! tool — the sandbox builds no ballistic colliders), so rebuilding would cover a fraction of the
//! volumes; and tagging the existing meshes both tames the pile and supplies the render geometry in
//! one stroke, exactly as [`crate::sandbox`] does via `ViewOf`.
//!
//! # On-top rendering
//!
//! "On-top" is a render-layer trick, not a material one: a volume in [`VolumeState::OnTop`] moves to
//! [`OVERLAY_LAYER`], which a second camera — a child of the fly camera, so it shares its pose —
//! renders after the main pass with its own depth buffer and no clear, compositing the volume over
//! the scene even when it sits geometrically inside the hull. That overlay layer gets its own light
//! ([`spawn_overlay_light`]) or the volumes render dark there.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use super::wheel_view::gear_slot;
use super::{Hull, VizLayers};
use crate::bake::TankGeometry;
use crate::track::link_view::TrackLink;

/// Render layer for volumes drawn "on top" — the [`OverlayCamera`] renders only this, with its own
/// depth buffer and no clear, so it composites over the main scene regardless of containment. Layer 0
/// (everything else, including the sandbox's gizmos) is the main pass.
pub(super) const OVERLAY_LAYER: usize = 1;

/// A mesh layer's tri-state: solid (its own material) → x-ray (its material swapped translucent) →
/// hidden. The hull's loop, mirroring the ballistics sandbox's `MeshState` (which this crate cannot
/// import — it is private to the bin-only `sandbox` module).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MeshState {
    #[default]
    Solid,
    Xray,
    Hidden,
}

impl MeshState {
    /// The three states in tap order, for the panel's segmented control. Only the `dev_ui` panel
    /// references it, and the module compiles without that feature — hence the gated `allow`.
    #[cfg_attr(not(feature = "dev_ui"), allow(dead_code))]
    pub(crate) const ALL: [MeshState; 3] = [MeshState::Solid, MeshState::Xray, MeshState::Hidden];

    #[cfg_attr(not(feature = "dev_ui"), allow(dead_code))]
    pub(crate) fn label(self) -> &'static str {
        match self {
            MeshState::Solid => "solid",
            MeshState::Xray => "x-ray",
            MeshState::Hidden => "off",
        }
    }
}

/// A volume layer's four-state: off → drawn-on-top (overlay pass) → solid (depth-tested in the main
/// pass) → x-ray (translucent in the main pass). Mirrors the ballistics sandbox's `VolumeState`.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VolumeState {
    #[default]
    Hidden,
    OnTop,
    Solid,
    Xray,
}

impl VolumeState {
    /// The four states in tap order, for the panel's segmented control. See [`MeshState::ALL`] for
    /// the gated `allow`.
    #[cfg_attr(not(feature = "dev_ui"), allow(dead_code))]
    pub(crate) const ALL: [VolumeState; 4] = [
        VolumeState::Hidden,
        VolumeState::OnTop,
        VolumeState::Solid,
        VolumeState::Xray,
    ];

    #[cfg_attr(not(feature = "dev_ui"), allow(dead_code))]
    pub(crate) fn label(self) -> &'static str {
        match self {
            VolumeState::Hidden => "off",
            VolumeState::OnTop => "on-top",
            VolumeState::Solid => "solid",
            VolumeState::Xray => "x-ray",
        }
    }
}

/// The mesh class a tagged view mesh belongs to. Carries the hull's original material so x-ray can
/// swap it translucent and back; volumes take their colour from [`VolumeMaterials`]. `Skip` marks a
/// running-gear / track-link mesh so the tagger drops it from its query (it is owned by another
/// layer) without perpetually re-scanning it.
#[derive(Component)]
pub(super) enum ViewMesh {
    Hull(Handle<StandardMaterial>),
    Collider,
    Ballistic,
    /// A terrain / course / obstacle mesh (not under the tank root). Carries its original material so
    /// x-ray can swap it translucent and back, exactly like the hull.
    World(Handle<StandardMaterial>),
    Skip,
}

/// Opaque lit + translucent material pairs for the two volume classes, plus the translucent material
/// the hull swaps to for x-ray. Lit (not unlit) so overlapping plates shade apart; the overlay layer
/// gets its own light so the on-top pass is not dark.
#[derive(Resource)]
struct VolumeMaterials {
    collider: Handle<StandardMaterial>,
    collider_xray: Handle<StandardMaterial>,
    ballistic: Handle<StandardMaterial>,
    ballistic_xray: Handle<StandardMaterial>,
    hull_translucent: Handle<StandardMaterial>,
    world_translucent: Handle<StandardMaterial>,
}

/// The camera that renders [`OVERLAY_LAYER`] on top of the main view. Spawned as a child of the fly
/// camera in [`super::spawn_camera`], so it inherits the fly pose and the on-top volumes track the
/// view.
#[derive(Component)]
pub(super) struct OverlayCamera;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, (setup_volume_materials, spawn_overlay_light))
        .add_systems(Update, (tag_view_meshes, apply_view_layers).chain());
}

fn setup_volume_materials(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    let solid = |color: Color| StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.75,
        ..default()
    };
    let xray = |color: Srgba| StandardMaterial {
        base_color: color.with_alpha(0.3).into(),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.75,
        ..default()
    };
    commands.insert_resource(VolumeMaterials {
        // Amber for the physics collision proxies.
        collider: materials.add(solid(Color::srgb(0.95, 0.62, 0.15))),
        collider_xray: materials.add(xray(Srgba::new(0.95, 0.62, 0.15, 1.0))),
        // Steel-blue for the ballistic/armour volumes (the ballistics sandbox's armour hue).
        ballistic: materials.add(solid(Color::srgb(0.35, 0.55, 0.95))),
        ballistic_xray: materials.add(xray(Srgba::new(0.35, 0.55, 0.95, 1.0))),
        hull_translucent: materials.add(StandardMaterial {
            base_color: Color::srgba(0.62, 0.64, 0.68, 0.16),
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
        // A dim, neutral ghost for the course: darker and more transparent than the hull's, so
        // x-raying the terrain drops it into the background and keeps the tank the visual subject.
        world_translucent: materials.add(StandardMaterial {
            base_color: Color::srgba(0.34, 0.36, 0.34, 0.10),
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
    });
}

/// A directional light on the overlay layer, matching the world light's direction — without it the
/// on-top volumes (rendered by [`OverlayCamera`]) get no scene light and read flat/dark.
fn spawn_overlay_light(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            ..default()
        },
        Transform::from_xyz(4.0, 9.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::layer(OVERLAY_LAYER),
    ));
}

/// Classify a view mesh by walking up its `ChildOf` chain to the first named ancestor that carries a
/// role — the same verdict the game's `bind_tank_view` hides by, which since the classifier
/// precedent (2026-08-07) is the BAKE's: a collision proxy is declared in the tank RON and a
/// ballistic volume is a node whose primitives wear a registry substance. The nearest match wins (a
/// volume node's own name is hit before the plain `Hull` node above it).
///
/// The hull fallback is SCOPED to the tank subtree by a real ancestry test against the [`Hull`] body
/// entity: a role-less walk that passes through `hull` is a hull visual, and a role-less walk that
/// runs off the top of the tree WITHOUT reaching `hull` is a world/course mesh. That scope is the fix
/// for the x-ray-the-hull-x-rays-the-terrain bug — the sandbox course slabs are parent-less scene-root
/// meshes, so before the scope they all fell through the walk to `Hull`.
fn classify(
    entity: Entity,
    hull: Entity,
    parents: &Query<&ChildOf>,
    names: &Query<&Name>,
    geometry: Option<&TankGeometry>,
) -> ViewClass {
    let mut probe = entity;
    loop {
        if let Ok(name) = names.get(probe)
            && let Some(class) = role_for_name(name.as_str(), geometry)
        {
            return class;
        }
        // Reached the tank root with no role name → a hull visual mesh.
        if probe == hull {
            return ViewClass::Hull;
        }
        match parents.get(probe) {
            Ok(parent) => probe = parent.parent(),
            // Exhausted the chain without passing through the tank root → a world/course mesh.
            Err(_) => return ViewClass::World,
        }
    }
}

/// The role a single node name carries, or `None` to keep walking up (an unnamed primitive, a plain
/// structural node like `Hull` / `Turret_Yaw`). Never returns [`ViewClass::Hull`] — hull is the
/// walk-exhausted fallback, not a name.
///
/// The running-gear / link check comes FIRST and is the one thing still keyed off the name, because
/// it is an addressing question (which slot of the wheel layer is this?) rather than a
/// classification one — and it must win over the ballistic verdict, since the road wheels are BOTH
/// their own armour volume and the wheel layer's meshes. Everything below it asks the bake.
fn role_for_name(name: &str, geometry: Option<&TankGeometry>) -> Option<ViewClass> {
    // Running gear and the link template/markers are other layers' meshes — skip.
    if gear_slot(name).is_some() || matches!(name, "Link" | "Link_Box" | "Pin_Start" | "Pin_End") {
        return Some(ViewClass::Skip);
    }
    let geometry = geometry?;
    if geometry
        .collision_proxies
        .iter()
        .any(|&index| geometry.nodes[index].name == name)
    {
        return Some(ViewClass::Collider);
    }
    if geometry.is_ballistic(name) {
        return Some(ViewClass::Ballistic);
    }
    None
}

/// The result of [`classify`] before the hull's material is captured into [`ViewMesh`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ViewClass {
    Hull,
    Collider,
    Ballistic,
    World,
    Skip,
}

/// Tag every un-tagged glb mesh with its [`ViewMesh`] class exactly once. Runs each frame (a filtered
/// scan) so late-instantiated / hot-reloaded scene meshes are picked up — `Without<ViewMesh>` makes
/// it a one-time tag per mesh. `Without<TrackLink>` keeps the instanced-shoe pool (nameless children
/// of the hull, which would otherwise fall through to `Hull`) out; the running-gear nodes are named,
/// so [`classify`] excludes them by name.
///
/// A hull or world mesh remembers its current material so x-ray can restore it; volumes and skips
/// carry none. Every tagged mesh gets an explicit `RenderLayers::layer(0)` so [`apply_view_layers`]
/// can move a volume to the overlay layer and back with a uniform query.
///
/// Gated on the [`Hull`] body existing ([`Single`]): the ancestry test [`classify`] runs needs the
/// tank root, and until it is up the course meshes could not be told from a hull visual. The pre-rig
/// frames simply defer — the glb meshes are not instantiated yet either, and the course meshes are
/// tagged the frame the hull lands.
fn tag_view_meshes(
    hull: Single<Entity, With<Hull>>,
    candidates: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        (With<Mesh3d>, Without<ViewMesh>, Without<TrackLink>),
    >,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    blueprint: Option<Res<crate::bake::TankBlueprint>>,
    mut commands: Commands,
) {
    let hull = *hull;
    let geometry = blueprint.as_deref().map(|blueprint| &*blueprint.geometry);
    for (entity, material) in &candidates {
        let view = match classify(entity, hull, &parents, &names, geometry) {
            ViewClass::Hull => ViewMesh::Hull(material.0.clone()),
            ViewClass::Collider => ViewMesh::Collider,
            ViewClass::Ballistic => ViewMesh::Ballistic,
            ViewClass::World => ViewMesh::World(material.0.clone()),
            ViewClass::Skip => ViewMesh::Skip,
        };
        commands
            .entity(entity)
            .insert((view, RenderLayers::layer(0)));
    }
}

/// Apply the per-class layer state to every tagged view mesh, EVERY frame, writing only on change
/// (the ballistics sandbox's continuous-assert discipline: an edge-triggered mirror loses every race
/// against a late writer of `Visibility` on the model tree). The hull swaps material + visibility for
/// its loop; each volume mesh sets visibility, **render layer** (overlay = on-top), and material.
fn apply_view_layers(
    viz: Res<VizLayers>,
    materials: Option<Res<VolumeMaterials>>,
    mut meshes: Query<(
        &ViewMesh,
        &mut Visibility,
        &mut RenderLayers,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let Some(materials) = materials else {
        return;
    };
    let layer0 = RenderLayers::layer(0);
    let overlay = RenderLayers::layer(OVERLAY_LAYER);

    for (view, mut visibility, mut layers, mut material) in &mut meshes {
        let (want_vis, want_layer, want_mat) = match view {
            ViewMesh::Hull(original) => match viz.hull {
                MeshState::Solid => (Visibility::Inherited, &layer0, original),
                MeshState::Xray => (Visibility::Inherited, &layer0, &materials.hull_translucent),
                MeshState::Hidden => (Visibility::Hidden, &layer0, original),
            },
            ViewMesh::Collider => volume_target(
                viz.collider_volumes,
                &layer0,
                &overlay,
                &materials.collider,
                &materials.collider_xray,
            ),
            ViewMesh::Ballistic => volume_target(
                viz.ballistic_volumes,
                &layer0,
                &overlay,
                &materials.ballistic,
                &materials.ballistic_xray,
            ),
            // Terrain / course meshes: the same solid/x-ray/hidden loop as the hull, on its own
            // switch. Visibility only — the static colliders under these meshes are never touched.
            ViewMesh::World(original) => match viz.world {
                MeshState::Solid => (Visibility::Inherited, &layer0, original),
                MeshState::Xray => (Visibility::Inherited, &layer0, &materials.world_translucent),
                MeshState::Hidden => (Visibility::Hidden, &layer0, original),
            },
            // Running gear / track links — owned by their own layers, never touched here.
            ViewMesh::Skip => continue,
        };
        if *visibility != want_vis {
            *visibility = want_vis;
        }
        if *layers != *want_layer {
            *layers = want_layer.clone();
        }
        if material.0 != *want_mat {
            material.0 = want_mat.clone();
        }
    }
}

/// The (visibility, render-layer, material) a volume mesh wants for a given [`VolumeState`].
fn volume_target<'a>(
    state: VolumeState,
    layer0: &'a RenderLayers,
    overlay: &'a RenderLayers,
    opaque: &'a Handle<StandardMaterial>,
    ghost: &'a Handle<StandardMaterial>,
) -> (Visibility, &'a RenderLayers, &'a Handle<StandardMaterial>) {
    match state {
        VolumeState::Hidden => (Visibility::Hidden, layer0, opaque),
        VolumeState::OnTop => (Visibility::Visible, overlay, opaque),
        VolumeState::Solid => (Visibility::Visible, layer0, opaque),
        VolumeState::Xray => (Visibility::Visible, layer0, ghost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped Tiger's own geometry, extracted the way the game extracts it — this classifier
    /// answers off the bake now, so the test drives the real asset rather than a hand-written name
    /// list that could agree with nothing.
    fn tiger_geometry() -> TankGeometry {
        let spec: crate::spec::TankSpec =
            ron::de::from_str(include_str!("../../assets/tiger_1/tiger_1.tank.ron"))
                .expect("tiger_1.tank.ron parses");
        let glb = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(crate::tank::TIGER_GLB_PATH);
        crate::bake::extract_tank_geometry(
            &glb,
            &spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .expect("the Tiger glb extracts")
    }

    /// The classifier, driven by the exact node names the shipped Tiger glb carries: the declared
    /// collision proxies and the substance-wearing volumes each land in their layer, the running
    /// gear / link template / pin markers are skipped (the ROAD WHEELS included — they are their own
    /// armour volume, but the wheel layer owns their meshes), and the decor render meshes plus plain
    /// structural nodes fall through to the hull.
    #[test]
    fn node_names_classify_into_the_right_layer() {
        let geometry = tiger_geometry();
        let role = |name: &str| role_for_name(name, Some(&geometry));

        // Collider proxies → the collider-volume layer.
        assert_eq!(role("Hull_Collider"), Some(ViewClass::Collider));
        assert_eq!(role("Turret_Collider"), Some(ViewClass::Collider));

        // Ballistic volumes (armour plates AND component volumes) → the ballistic-volume layer.
        for name in [
            "Hull_UFP_Upper",
            "Turret_Side",
            "Engine",
            "Ammo_L_0",
            "Coax_MG_Body",
        ] {
            assert_eq!(role(name), Some(ViewClass::Ballistic), "{name}");
        }

        // Running gear, the link template and its box, and the pin markers → skipped (other layers'
        // meshes, or non-mesh empties).
        for name in [
            "Wheel_L_0",
            "Wheel_R_7",
            "Sprocket_L",
            "Idler_R",
            "Link",
            "Link_Box",
            "Pin_Start",
            "Pin_End",
        ] {
            assert_eq!(role(name), Some(ViewClass::Skip), "{name}");
        }

        // The visible render meshes and plain structural nodes carry no role — they fall through
        // the walk to the hull layer.
        for name in ["Hull_Decor", "Turret_Decor", "Hull", "Turret_Yaw"] {
            assert_eq!(role(name), None, "{name}");
        }

        // Without a bake there is no verdict to give: everything but the name-addressed running
        // gear falls through, which is what the track sandbox sees before `TankBlueprint` lands.
        assert_eq!(role_for_name("Hull_UFP_Upper", None), None);
        assert_eq!(role_for_name("Wheel_L_0", None), Some(ViewClass::Skip));
    }

    /// The tank-subtree ancestry scope: a role-less mesh UNDER the [`Hull`] body is a hull visual,
    /// the same mesh OUTSIDE the tank tree (a course/terrain mesh) is World not Hull — the fix for
    /// x-ray-the-hull ghosting the terrain — and a role still wins over ancestry either way (a
    /// substance-wearing node under the hull classifies Ballistic). Driven through a real `World` so
    /// the `ChildOf` walk, not just [`role_for_name`], is exercised.
    #[test]
    fn ancestry_scopes_the_hull_fallback_to_the_tank_subtree() {
        use bevy::ecs::system::SystemState;

        let geometry = tiger_geometry();
        let mut world = World::new();
        let hull = world.spawn_empty().id();
        // A role-less render mesh nested two deep under the tank root → hull visual.
        let structural = world.spawn(ChildOf(hull)).id();
        let hull_mesh = world.spawn(ChildOf(structural)).id();
        // A parent-less course slab at the scene root → World, NOT Hull.
        let world_mesh = world.spawn_empty().id();
        // A named armour volume under the hull → Ballistic (the role beats ancestry).
        let ballistic = world
            .spawn((Name::new("Hull_UFP_Upper"), ChildOf(hull)))
            .id();

        let mut state: SystemState<(Query<&ChildOf>, Query<&Name>)> = SystemState::new(&mut world);
        let (parents, names) = state.get(&world).unwrap();
        let geometry = Some(&geometry);

        assert_eq!(
            classify(hull_mesh, hull, &parents, &names, geometry),
            ViewClass::Hull
        );
        assert_eq!(
            classify(world_mesh, hull, &parents, &names, geometry),
            ViewClass::World
        );
        assert_eq!(
            classify(ballistic, hull, &parents, &names, geometry),
            ViewClass::Ballistic
        );
    }

    /// The two state loops expose their tap order as `ALL`, longest-lived first — the panel's
    /// segmented control iterates it, and the count must match the enum so a future state can't be
    /// silently dropped from the UI.
    #[test]
    fn state_loops_enumerate_every_variant() {
        assert_eq!(MeshState::ALL.len(), 3);
        assert_eq!(VolumeState::ALL.len(), 4);
        assert_eq!(MeshState::default(), MeshState::Solid);
        assert_eq!(VolumeState::default(), VolumeState::Hidden);
    }
}
