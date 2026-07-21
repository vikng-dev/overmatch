//! Diagnostic contact probe (`SPIKE_CONTACT_PROBE=1`): a client-side, env-gated `FixedLast`
//! observer that dumps, every fixed tick (live AND rollback-replay), the full broad/narrow-phase
//! state for each (tank hull collision proxy, static terrain collider) pair — enough to
//! discriminate WHY a rollback-restored client can hold `hc=0` at a physically penetrating pose
//! for hundreds of ticks while the server holds contact:
//!
//! - **B1 zombie graph entry** — the pair IS in the `ContactGraph` but never (re)touches: its
//!   narrow-phase update early-outs (collider query miss / stale flags) while its mere existence
//!   blocks broad-phase re-add via the `contains_key` dedup.
//! - **B2 discovery failure** — the pair is ABSENT and the hull proxy never lands in
//!   `MovedProxies`, so the broad phase never re-queries it.
//! - **B3 tree stomp** — the rollback restore's `set_proxy_aabb` + `refit_all` leaves the BVH
//!   leaf diverged from the `EnlargedAabb`/`ColliderAabb` components, so tree queries miss
//!   geometrically overlapping proxies.
//!
//! PASSIVE, like [`crate::trace`]: read-only system params, one `info!` line per pair per tick
//! under the `contact_probe` target. OFF (not even registered) unless `SPIKE_CONTACT_PROBE` is
//! set at startup, so an unarmed run pays one `std::env::var` at plugin build and nothing after.
//!
//! ## Line schema (space-separated `key=value`, one line per candidate pair)
//! - `tick`  — lightyear predicted tick (or a local counter when `LocalTimeline` is absent).
//! - `rp`    — 1 iff this is a rollback-replay tick (same source as trace.rs's `rp`).
//! - `ctl`   — 1 iff the root carries the game's `Controlled` marker.
//! - `rooty`/`rootvy` — the tank root's `Position.y` / `LinearVelocity.y` (anchoring).
//! - `hull`/`terr` — the pair's entities (hull collision proxy, terrain collider).
//! - `pair`  — 1 iff the pair exists in the `ContactGraph`.
//! - `flags`/`bits` — the `ContactPairFlags` (debug + raw u16), `man` manifold count,
//!   `pen` deepest contact penetration (only when the pair exists; `pen` unclamped, negative =
//!   speculative separation gap).
//! - `moved` — hull proxy in the `MovedProxies` resource; `tmoved` — in the tree's own
//!   `moved_proxies` list (separate bookkeeping, both checked).
//! - `caabb_y`/`eaabb_y` — hull `ColliderAabb` / `EnlargedAabb` COMPONENT min..max y.
//! - `leaf_y` — the BVH tree LEAF's min..max y for the hull proxy; `leaf_dvg` the max abs
//!   difference over all six bounds between leaf and `EnlargedAabb` component (B3 signal);
//!   `tleaf_dvg` the same for the terrain proxy.
//! - `cposy`/`expy`/`cdy` — hull collider child `Position.y`, the coherent value
//!   (root Position + root Rotation · `ColliderTransform.translation`), and their delta;
//!   `cdmag` the full 3D delta magnitude (stale-child-Position signal).
//! - `cto` — the proxy's `ColliderTransform.translation` (root-relative offset from the part
//!   hierarchy): constant for a rigid part; drift = the part's own Transform is moving.
//! - `terr_y` — terrain `ColliderAabb` min..max y, so overlap is checkable per line.
//!
//! Pair selection keeps the wedge scenario's volume sane: a line is emitted iff the pair exists
//! in the graph, OR the terrain is the map-wide GROUND slab (always logged), OR the component
//! `ColliderAabb`s intersect once the hull's is grown by [`NEAR_MARGIN`] — so the slab the tank
//! rests on stays visible even while the defect holds the pair absent/untouching.

use avian3d::collider_tree::{ColliderTreeProxyKey, ColliderTrees, MovedProxies};
use avian3d::collision::collider::EnlargedAabb;
use avian3d::prelude::{
    ColliderAabb, ColliderTransform, CollisionLayers, ContactGraph, LinearVelocity, Position,
    Rotation,
};
use bevy::prelude::*;
use lightyear::prelude::{LocalTimeline, Rollback};

use crate::Layer;
use crate::tank::{Controlled, Tank};

/// How far (m) to grow the hull proxy's `ColliderAabb` when deciding whether a graph-absent
/// terrain pair is still worth a line. Generous relative to avian's 5 cm `AABB_MARGIN` so the
/// probe keeps watching the slab across the whole beached window.
const NEAR_MARGIN: f32 = 1.5;

/// Register the probe iff `SPIKE_CONTACT_PROBE` is enabled. Mirrors the trace.rs pattern: the gate
/// is evaluated once at plugin build, so an unarmed run registers nothing.
pub fn plugin(app: &mut App) {
    if !super::harness::env_flag("SPIKE_CONTACT_PROBE", false) {
        return;
    }
    info!("contact_probe: armed (SPIKE_CONTACT_PROBE enabled)");
    app.add_systems(FixedLast, probe);
}

/// Format an AABB's y-extent compactly.
fn yr(min_y: f32, max_y: f32) -> String {
    format!("{min_y:.4}..{max_y:.4}")
}

/// Max abs difference over all six bounds between a BVH leaf AABB (`obvhs::aabb::Aabb`, whose `min`/`max` are
/// a DIFFERENT glam version's `Vec3A` — passed as plain arrays so neither crate needs naming) and a component AABB.
fn aabb_divergence(leaf_min: [f32; 3], leaf_max: [f32; 3], component: &ColliderAabb) -> f32 {
    let dmin = (Vec3::from(leaf_min) - component.min).abs();
    let dmax = (Vec3::from(leaf_max) - component.max).abs();
    dmin.max_element().max(dmax.max_element())
}

/// The probe body: one line per candidate (hull proxy, terrain collider) pair per fixed tick.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn probe(
    roots: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            Has<Controlled>,
        ),
        With<Tank>,
    >,
    children: Query<&Children>,
    colliders: Query<(
        Entity,
        &CollisionLayers,
        &ColliderAabb,
        &EnlargedAabb,
        &ColliderTreeProxyKey,
        Option<&Position>,
        Option<&ColliderTransform>,
    )>,
    contact_graph: Res<ContactGraph>,
    moved_proxies: Res<MovedProxies>,
    trees: Res<ColliderTrees>,
    timeline: Option<Res<LocalTimeline>>,
    replaying: Query<(), With<Rollback>>,
    mut tick_counter: Local<u64>,
) {
    let tick = match timeline.as_deref() {
        Some(timeline) => u64::from(timeline.tick().0),
        None => {
            let t = *tick_counter;
            *tick_counter += 1;
            t
        }
    };
    let rp = u8::from(!replaying.is_empty());

    // Static terrain colliders (ground slab + course blocks): membership on the Terrain layer.
    // Collected once per tick; the wedge world has ~12.
    let terrain: Vec<_> = colliders
        .iter()
        .filter(|(_, layers, ..)| layers.memberships.has_all(Layer::Terrain))
        .collect();

    for (root, root_pos, root_rot, root_vel, controlled) in &roots {
        for descendant in children.iter_descendants(root) {
            let Ok((hull, layers, caabb, eaabb, key, cpos, ctransform)) = colliders.get(descendant)
            else {
                continue;
            };
            // Only the Vehicle-layer collision proxies (the armor trimeshes are Layer::Armor with
            // an empty filter — they interact with nothing and would only add noise).
            if !layers.memberships.has_all(Layer::Vehicle) {
                continue;
            }

            // Hull-proxy tree-side state, shared across its terrain pairings this tick.
            let moved = moved_proxies.contains(*key);
            let tree = trees.tree_for_type(key.tree_type());
            let tmoved = tree.moved_proxies.contains(&key.id());
            let leaf = tree.get_proxy_aabb(key.id());
            let (leaf_y, leaf_dvg) = match leaf {
                Some(leaf) => (
                    yr(leaf.min.y, leaf.max.y),
                    format!(
                        "{:.6}",
                        aabb_divergence(leaf.min.to_array(), leaf.max.to_array(), &eaabb.get())
                    ),
                ),
                None => ("none".into(), "none".into()),
            };
            // Child collider Position coherence vs the restored root pose. `cto` is the raw
            // `ColliderTransform.translation` (the proxy's ROOT-RELATIVE offset, folded from the
            // part hierarchy): for a rigid part it must be CONSTANT — drift here means the part's
            // own Transform is moving, which would carry the collider away from the root pose
            // while `cdy` (avian's child-position sync residue) stays near zero.
            let (cposy, expy, cdy, cdmag, cto) = match (cpos, ctransform) {
                (Some(cpos), Some(ct)) => {
                    let expected = root_pos.0 + root_rot.0 * ct.translation;
                    let delta = cpos.0 - expected;
                    (
                        format!("{:.4}", cpos.y),
                        format!("{:.4}", expected.y),
                        format!("{:.6}", delta.y),
                        format!("{:.6}", delta.length()),
                        format!(
                            "{:.4},{:.4},{:.4}",
                            ct.translation.x, ct.translation.y, ct.translation.z
                        ),
                    )
                }
                _ => (
                    "none".into(),
                    "none".into(),
                    "none".into(),
                    "none".into(),
                    "none".into(),
                ),
            };

            let near_aabb = caabb.grow(Vec3::splat(NEAR_MARGIN));
            for (terr, _, tcaabb, teaabb, tkey, _, _) in &terrain {
                let pair = contact_graph.get(hull, *terr).map(|(_, pair)| pair);
                // The GROUND slab (the y-span-1.01 collider covering the whole map) is logged
                // UNCONDITIONALLY: it's the pair the server holds in the wedge scenario, and the
                // whole point is watching it while the client's state is far from re-forming it.
                let is_ground =
                    (tcaabb.max.y - tcaabb.min.y) < 1.5 && (tcaabb.max.x - tcaabb.min.x) > 100.0;
                if pair.is_none() && !is_ground && !near_aabb.intersects(tcaabb) {
                    continue;
                }
                let (present, flags, bits, man, pen) = match pair {
                    Some(pair) => (
                        1u8,
                        format!("{:?}", pair.flags),
                        pair.flags.bits(),
                        pair.manifolds.len(),
                        pair.find_deepest_contact()
                            .map_or("none".into(), |p| format!("{:.4}", p.penetration)),
                    ),
                    None => (0u8, "-".into(), 0u16, 0, "none".into()),
                };
                // Terrain-side leaf divergence: the restore stomps EVERY tree (static included).
                let ttree = trees.tree_for_type(tkey.tree_type());
                let tleaf_dvg = ttree
                    .get_proxy_aabb(tkey.id())
                    .map_or("none".into(), |leaf| {
                        format!(
                            "{:.6}",
                            aabb_divergence(
                                leaf.min.to_array(),
                                leaf.max.to_array(),
                                &teaabb.get()
                            )
                        )
                    });

                info!(
                    target: "contact_probe",
                    "tick={tick} rp={rp} ctl={ctl} rooty={rooty:.4} rootvy={rootvy:.4} \
                     hull={hull} terr={terr} pair={present} flags={flags} bits={bits:#06x} \
                     man={man} pen={pen} moved={moved} tmoved={tmoved} \
                     caabb_y={caabb_y} eaabb_y={eaabb_y} leaf_y={leaf_y} leaf_dvg={leaf_dvg} \
                     tleaf_dvg={tleaf_dvg} cposy={cposy} expy={expy} cdy={cdy} cdmag={cdmag} \
                     cto={cto} terr_y={terr_y}",
                    ctl = u8::from(controlled),
                    rooty = root_pos.y,
                    rootvy = root_vel.y,
                    caabb_y = yr(caabb.min.y, caabb.max.y),
                    eaabb_y = yr(eaabb.get().min.y, eaabb.get().max.y),
                    terr_y = yr(tcaabb.min.y, tcaabb.max.y),
                );
            }
        }
    }
}
