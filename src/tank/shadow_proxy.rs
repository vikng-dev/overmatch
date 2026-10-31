//! Tank SHADOW PROXIES: beyond a switch range, a tank's rigid assemblies cast their shadows from the
//! few simple shapes its sheet declares (`shadow_proxies` in `<tank>.tank.ron`), and their drawn
//! geometry only receives. Nearer tanks keep their drawn casters. View-only: the proxies are nodes of
//! the tank's glb scene, bound when the scene is, and nothing here reads or writes sim state.
//!
//! # What a proxy stands in for
//!
//! A proxy stands in for the RIGID ASSEMBLY it is parented under: every mesh below the proxy's parent
//! node, except below an articulated node ([`TankSpec::articulated_nodes`]), which moves relative to
//! that parent and so cannot be followed by a rigid proxy. An articulated part keeps casting its own
//! shadow unless a proxy is parented under it in turn. [`plan`] computes that partition from the
//! instantiated view tree, and `render_policy`'s nearest-scope inheritance realizes it: the assembly's
//! root carries [`CasterSwap::assembly`], each articulated node inside an assembly
//! [`VisualScope::OWN_CASTER`], and each proxy [`CasterSwap::proxy`]. Every caster under a tank
//! therefore resolves to exactly one of its own drawn geometry or one assembly's proxies.
//!
//! # The switch
//!
//! One decision per tank, on the camera distance from the player's view to the tank's root
//! ([`switch_side`]). Every assembly of a tank is written by one system in one frame, so a
//! tank never casts from a mix. Bevy selects a mesh per camera and not per cascade, so the camera
//! distance is the selector the stock renderer offers.
//!
//! The range is one generic value, [`SWITCH_M`], and not the LOD certificate's law. That law
//! would switch where the proxy's deviation from the drawn surface, amplified by `cot(sun elevation)`
//! on flat ground, projects under the pixel budget; the certificate records no proxy deviation, and a
//! surface deviation is set by the thinnest feature a proxy omits (a whip antenna is metres of
//! deviation and sub-texel in a shadow map), which would put the switch past the shadow envelope.
//!
//! # Atomic by construction
//!
//! The proxies arrive in the same glb scene as the geometry they stand in for, so both exist on the
//! frame the bind runs, and a proxy with no mesh under it is refused there: its assembly keeps its own
//! casters. The worst case of a bad declaration is the optimisation not happening, never a tank
//! without a shadow.

use bevy::prelude::*;

use crate::render_policy::{CasterSwap, VisualScope};
use crate::view::PlayerView;

/// The switch range, metres from the player's view to a tank's root: one generic value for every
/// tank. ASSUMED: the shipped cascade ladder's last-cascade near bound, DERIVED as the geometric
/// split `√(40 × 350) = 118.3 m` (`settings.rs`), rounded up, so tanks in the two finer cascades keep
/// their drawn casters. The pricing battery (#169) owns the final value.
const SWITCH_M: f32 = 120.0;

/// How far inside the switch range a tank must come back before it returns to its drawn casters, as
/// a fraction of the range. ASSUMED: a flip rewrites the scopes of every caster in the tank's
/// assemblies, and a camera parked on the range would otherwise pay that every frame.
const SWITCH_HYSTERESIS: f32 = 0.05;

/// THE SWITCH LAW: the side a tank's swap takes at `distance_m` from the player's view, given the
/// side it is on and the range `switch_m` ([`SWITCH_M`] in play). Beyond the range it is the
/// proxies; back inside the range by the hysteresis band it is the drawn geometry; between the two,
/// and for a distance that is not a number (no view to measure from), it holds.
fn switch_side(switch_m: f32, current: CasterSwap, distance_m: f32) -> CasterSwap {
    match current {
        CasterSwap::Detail if distance_m >= switch_m => CasterSwap::Proxy,
        CasterSwap::Proxy if distance_m < switch_m * (1.0 - SWITCH_HYSTERESIS) => {
            CasterSwap::Detail
        }
        _ => current,
    }
}

/// One tank's caster swap: the view entities [`switch_tank_casters`] writes, and the side they are
/// on. On the tank's root, whose distance from the player's view decides the side.
#[derive(Component, Debug)]
pub(crate) struct TankCasterSwap {
    /// Assembly roots: their drawn geometry hands its shadow to the proxies at range.
    assemblies: Vec<Entity>,
    /// The proxies standing in for them.
    proxies: Vec<Entity>,
    side: CasterSwap,
}

/// Why a declared proxy was not bound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Refusal {
    /// Nothing under it draws: silencing its assembly against it would leave no shadow.
    Meshless,
    /// It hangs off the tank's root or the scene instance under it: neither is a rigid assembly a
    /// proxy can stand in for, and the root's own scope is the tank's channel.
    AtRoot,
}

/// The partition [`plan`] derives for one tank.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct CasterPlan {
    /// Each bound proxy's parent, once.
    pub(crate) assemblies: Vec<Entity>,
    /// The bound proxies.
    pub(crate) proxies: Vec<Entity>,
    /// Articulated nodes inside an assembly that are not assembly roots: they keep their own casters.
    pub(crate) fences: Vec<Entity>,
    /// Declared proxies left unbound, and why.
    pub(crate) refused: Vec<(Entity, Refusal)>,
}

/// Partition one tank's view tree into assemblies, their proxies and the articulated parts inside
/// them (module doc).
///
/// `root` is the tank's root; `proxies` each declared proxy's view node with whether any mesh hangs
/// under it; `articulated` the articulated view nodes; `parent` the view tree's parent link.
///
/// The view tree's boundary is the root AND every direct child of it: bevy's glTF loader spawns
/// each scene as one instance entity under the root, whose children are the glTF's root nodes, so
/// the first node a proxy can stand in for is two levels down.
pub(crate) fn plan(
    root: Entity,
    proxies: &[(Entity, bool)],
    articulated: &[Entity],
    parent: impl Fn(Entity) -> Option<Entity>,
) -> CasterPlan {
    let inside = |node: Entity| node != root && parent(node).is_some_and(|up| up != root);
    let mut plan = CasterPlan::default();
    for &(proxy, has_mesh) in proxies {
        let assembly = parent(proxy).filter(|&node| inside(node));
        match (assembly, has_mesh) {
            (_, false) => plan.refused.push((proxy, Refusal::Meshless)),
            (None, true) => plan.refused.push((proxy, Refusal::AtRoot)),
            (Some(assembly), true) => {
                plan.proxies.push(proxy);
                if !plan.assemblies.contains(&assembly) {
                    plan.assemblies.push(assembly);
                }
            }
        }
    }
    for &node in articulated {
        if plan.assemblies.contains(&node) {
            continue;
        }
        let mut ancestor = parent(node);
        while let Some(current) = ancestor.filter(|&current| inside(current)) {
            if plan.assemblies.contains(&current) {
                plan.fences.push(node);
                break;
            }
            ancestor = parent(current);
        }
    }
    plan
}

impl CasterPlan {
    /// Write the plan's scopes, on the detail side, and the swap that switches them onto `root`.
    /// The proxies render at source detail: the shape they stand in with is the authored one.
    pub(crate) fn bind(self, commands: &mut Commands, root: Entity) {
        let side = CasterSwap::Detail;
        for &assembly in &self.assemblies {
            commands.entity(assembly).insert(side.assembly());
        }
        for &fence in &self.fences {
            commands.entity(fence).insert(VisualScope::OWN_CASTER);
        }
        for &proxy in &self.proxies {
            commands
                .entity(proxy)
                .insert((side.proxy(), crate::geometry_lod::SourceDetail));
        }
        for &(proxy, refusal) in &self.refused {
            warn!("tank shadow proxy {proxy:?} not bound ({refusal:?}); its assembly casts itself");
            commands.entity(proxy).insert(Visibility::Hidden);
        }
        if !self.proxies.is_empty() {
            commands.entity(root).insert(TankCasterSwap {
                assemblies: self.assemblies,
                proxies: self.proxies,
                side,
            });
        }
    }
}

/// Put every bound tank on the side [`switch_side`] gives its distance from the player's view.
/// Writes scopes only on a flip; a quiet frame changes nothing.
pub(crate) fn switch_tank_casters(
    views: Query<&GlobalTransform, With<PlayerView>>,
    mut tanks: Query<(&GlobalTransform, &mut TankCasterSwap)>,
    mut scopes: Query<&mut VisualScope>,
) {
    // No declared view, or several, measures no distance; the law then holds every tank where it
    // is.
    let eye = views.single().ok().map(GlobalTransform::translation);
    for (anchor, mut swap) in &mut tanks {
        let distance = eye.map_or(f32::NAN, |eye| eye.distance(anchor.translation()));
        let side = switch_side(SWITCH_M, swap.side, distance);
        if side == swap.side {
            continue;
        }
        swap.side = side;
        let writes = swap
            .assemblies
            .iter()
            .map(|&entity| (entity, side.assembly()))
            .chain(swap.proxies.iter().map(|&entity| (entity, side.proxy())));
        for (entity, scope) in writes {
            // An entity gone with a despawned subtree has no shadow left to swap.
            if let Ok(mut current) = scopes.get_mut(entity) {
                current.set_if_neq(scope);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_policy::{self, casts_shadow};

    /// The switch law at every distance, walked out and back: the proxies take over at the range
    /// and not before, the drawn geometry takes back over only inside the hysteresis band, and the
    /// side between the two is whichever the tank arrived with. Stated for any range, so it holds
    /// for whatever value the pricing battery settles on.
    #[test]
    fn the_switch_flips_at_the_range_and_returns_inside_the_band() {
        for switch_m in [1.0, SWITCH_M, 900.0] {
            let back = switch_m * (1.0 - SWITCH_HYSTERESIS);
            let steps: Vec<f32> = (0..=400)
                .map(|i| switch_m * 2.0 * i as f32 / 400.0)
                .collect();
            let mut side = CasterSwap::Detail;
            for &d in &steps {
                side = switch_side(switch_m, side, d);
                assert_eq!(
                    side == CasterSwap::Proxy,
                    d >= switch_m,
                    "out, {d} of {switch_m}"
                );
            }
            for &d in steps.iter().rev() {
                side = switch_side(switch_m, side, d);
                assert_eq!(
                    side == CasterSwap::Proxy,
                    d >= back,
                    "back, {d} of {switch_m}"
                );
            }
        }
    }

    /// A distance nobody measured (no view, or several) moves no tank, on either side.
    #[test]
    fn an_unmeasured_distance_holds_the_side() {
        for side in [CasterSwap::Detail, CasterSwap::Proxy] {
            assert_eq!(switch_side(SWITCH_M, side, f32::NAN), side);
        }
    }

    /// A tank-shaped tree with no tank in it: a body with a proxy, a turning ring under the body
    /// with a proxy, a pitch frame under the ring with no proxy, a recoiling part under the frame
    /// with a proxy, a wheel under the body with no proxy, and one mesh at every level.
    struct Tree {
        app: App,
        root: Entity,
        /// The player's view, standing on the x axis at the distance under test.
        eye: Entity,
        /// Every drawn mesh, with the proxy mesh that carries its shadow at range, or `None` when
        /// it carries its own.
        drawn: Vec<(Entity, Option<Entity>)>,
        /// Every proxy's own mesh.
        stand_ins: Vec<Entity>,
        plan_input: (Vec<(Entity, bool)>, Vec<Entity>),
    }

    impl Tree {
        fn new() -> Self {
            let mut app = App::new();
            app.add_plugins(render_policy::plugin)
                .add_systems(Update, switch_tank_casters);
            let world = app.world_mut();
            let root = world
                .spawn((VisualScope::WORLD_SOLID, GlobalTransform::IDENTITY))
                .id();
            let eye = world.spawn((PlayerView, GlobalTransform::IDENTITY)).id();
            let node = |world: &mut World, parent: Entity| world.spawn(ChildOf(parent)).id();
            let mesh = |world: &mut World, parent: Entity| {
                world
                    .spawn((Mesh3d(Handle::default()), ChildOf(parent)))
                    .id()
            };
            // The scene instance bevy's glTF loader puts between the root and the glTF's nodes.
            let scene = node(world, root);
            let body = node(world, scene);
            let body_proxy = node(world, body);
            let ring = node(world, body);
            let ring_proxy = node(world, ring);
            let pitch = node(world, ring);
            let recoil = node(world, pitch);
            let recoil_proxy = node(world, recoil);
            let wheel = node(world, body);
            let stand_ins = vec![
                mesh(world, body_proxy),
                mesh(world, ring_proxy),
                mesh(world, recoil_proxy),
            ];
            let decor = node(world, body);
            let drawn = vec![
                (mesh(world, body), Some(stand_ins[0])),
                (mesh(world, decor), Some(stand_ins[0])),
                (mesh(world, ring), Some(stand_ins[1])),
                (mesh(world, pitch), None),
                (mesh(world, recoil), Some(stand_ins[2])),
                (mesh(world, wheel), None),
            ];
            Self {
                app,
                root,
                eye,
                drawn,
                stand_ins,
                plan_input: (
                    vec![(body_proxy, true), (ring_proxy, true), (recoil_proxy, true)],
                    vec![ring, pitch, recoil, wheel],
                ),
            }
        }

        fn plan(&self) -> CasterPlan {
            let world = self.app.world();
            plan(self.root, &self.plan_input.0, &self.plan_input.1, |e| {
                world.entity(e).get::<ChildOf>().map(ChildOf::parent)
            })
        }

        fn bind(&mut self) {
            let plan = self.plan();
            let world = self.app.world_mut();
            plan.bind(&mut world.commands(), self.root);
            world.flush();
            self.app.update();
        }

        /// Stand the player's view `distance_m` from the tank's root and run a frame.
        fn stand_at(&mut self, distance_m: f32) {
            self.app
                .world_mut()
                .entity_mut(self.eye)
                .insert(GlobalTransform::from_translation(Vec3::X * distance_m));
            self.app.update();
        }
    }

    /// NO DOUBLE SHADOW, NO MISSING SHADOW, at any distance: every drawn mesh's shadow is cast by
    /// exactly one of itself or the one proxy of the assembly it belongs to, walked out past the
    /// range and back. A part a rigid proxy cannot follow keeps casting itself; a part with a proxy
    /// of its own hands its shadow to that proxy and to no other.
    #[test]
    fn every_caster_is_exactly_one_of_drawn_or_proxy_at_any_distance() {
        let mut tree = Tree::new();
        tree.bind();
        let mut walk: Vec<f32> = (0..=60).map(|i| i as f32 * 5.0).collect();
        walk.extend(walk.clone().into_iter().rev());
        let mut flips = 0;
        let mut was = false;
        for d in walk {
            tree.stand_at(d);
            let world = tree.app.world();
            let proxied = world
                .entity(tree.root)
                .get::<TankCasterSwap>()
                .expect("bound")
                .side
                == CasterSwap::Proxy;
            for &(mesh, stand_in) in &tree.drawn {
                let carried_by_proxy = stand_in.is_some_and(|p| casts_shadow(world, p));
                assert_ne!(
                    casts_shadow(world, mesh),
                    carried_by_proxy,
                    "at {d} m: a mesh's shadow is cast by exactly one of itself and its proxy"
                );
                assert_eq!(
                    carried_by_proxy,
                    proxied && stand_in.is_some(),
                    "at {d} m: a proxied mesh hands its shadow over exactly when the tank is past \
                     the range"
                );
            }
            for &stand_in in &tree.stand_ins {
                assert_eq!(casts_shadow(world, stand_in), proxied, "at {d} m");
            }
            flips += usize::from(proxied != was);
            was = proxied;
        }
        assert_eq!(flips, 2, "the walk crosses the range once each way");
    }

    /// The partition itself: one assembly per proxy parent, the articulated parts inside an
    /// assembly fenced unless they are assemblies themselves, and the articulated parts outside
    /// every assembly left alone.
    #[test]
    fn the_plan_fences_articulated_parts_inside_assemblies() {
        let tree = Tree::new();
        let plan = tree.plan();
        let (proxies, articulated) = &tree.plan_input;
        let parent = |e: Entity| -> Entity {
            tree.app
                .world()
                .entity(e)
                .get::<ChildOf>()
                .unwrap()
                .parent()
        };
        let [ring, pitch, recoil, wheel] = articulated[..] else {
            unreachable!()
        };
        let body = parent(proxies[0].0);
        assert_eq!(plan.assemblies, vec![body, ring, recoil]);
        assert_eq!(
            plan.proxies,
            proxies.iter().map(|(p, _)| *p).collect::<Vec<_>>()
        );
        assert_eq!(
            plan.fences,
            vec![pitch, wheel],
            "the pitch frame sits under the ring's assembly, the wheel under the body's"
        );
        assert!(plan.refused.is_empty());
    }

    /// A proxy that cannot stand in is refused, and nothing is silenced against it: no mesh under
    /// it, or no rigid node above it but the tank's root or the scene instance under the root.
    #[test]
    fn an_unusable_proxy_is_refused_and_silences_nothing() {
        let mut app = App::new();
        app.add_plugins(render_policy::plugin);
        let world = app.world_mut();
        let root = world.spawn(VisualScope::WORLD_SOLID).id();
        let scene = world.spawn(ChildOf(root)).id();
        let body = world.spawn(ChildOf(scene)).id();
        let drawn = world.spawn((Mesh3d(Handle::default()), ChildOf(body))).id();
        let empty = world.spawn(ChildOf(body)).id();
        let mut drawing_proxy_under = |parent: Entity| {
            let proxy = world.spawn(ChildOf(parent)).id();
            world.spawn((Mesh3d(Handle::default()), ChildOf(proxy)));
            proxy
        };
        let under_scene = drawing_proxy_under(scene);
        let under_root = drawing_proxy_under(root);
        let parent = |e: Entity| app.world().entity(e).get::<ChildOf>().map(ChildOf::parent);
        let plan = plan(
            root,
            &[(empty, false), (under_scene, true), (under_root, true)],
            &[],
            parent,
        );
        assert_eq!(
            plan.refused,
            vec![
                (empty, Refusal::Meshless),
                (under_scene, Refusal::AtRoot),
                (under_root, Refusal::AtRoot),
            ]
        );
        assert!(plan.assemblies.is_empty() && plan.proxies.is_empty());
        let world = app.world_mut();
        plan.bind(&mut world.commands(), root);
        world.flush();
        app.update();
        assert!(casts_shadow(app.world(), drawn));
        assert!(app.world().entity(root).get::<TankCasterSwap>().is_none());
    }
}
