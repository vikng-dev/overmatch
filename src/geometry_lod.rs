//! GEOMETRY LOD: the tank build's certificate, applied (ADR-0035's Runtime section).
//!
//! One module owns the whole seam. It reads `<id>.lod.json` as data, fingerprints the trio members
//! this process opens, and — on the render side — gives every spawned scene primitive the
//! certificate names a [`VisibilityRange`] plus one coincident sibling per rung. Nothing a SCENE
//! PRIMITIVE draws swaps at runtime: bevy selects per view, and the entities are all present from
//! the bind.
//!
//! # The mechanism: coincident siblings, one placement
//!
//! A rung is spawned as a CHILD of the rung-0 primitive entity at `Transform::IDENTITY`. Every
//! rung mesh is authored in the same mesh-local frame as its source, so ordinary propagation puts
//! all levels at one world origin — which is what makes the ranges' tiling meaningful (no two
//! levels can be in range on the same frame) and what leaves ONE transform per primitive for
//! anything that moves it. `render_policy` resolves a mesh against its nearest scoped ANCESTOR, so
//! a rung inherits its tank's channel and its shadow policy; this module writes no layer integer
//! and no shadow marker. Despawn is recursive, so a rung dies with the primitive it belongs to.
//!
//! A RUNG WEARS ITS SOURCE PRIMITIVE'S MATERIAL. Rung mesh records carry no glTF material — they
//! are bare machine reductions — and the material is cloned off the rung-0 entity at bind time, so
//! a swap changes the silhouette and nothing else.
//!
//! THE TRACK'S POOLED SHOES ARE THE EXCEPTION, and they are not spawned here ([`attach_rungs`] is
//! gated on `GltfMeshName`, which the loader writes on scene-primitive leaves only). A shoe is one
//! entity whose mesh HANDLE its belt swaps, because 194 shoes per tank moving every frame make the
//! sibling's per-frame cost — propagation, the visibility sweep, the extract scan — the dominant
//! one. It carries no `GeometryLodLevel` and no band; the certificate reaches it as
//! [`Chain::switches`] instead of as ranges.
//!
//! # Switch distances are derived, never shipped
//!
//! The certificate carries deviations and a bounding radius; [`ViewProfile`] carries the view —
//! `crate::view`'s shared facts spent at this ladder's own pixel budget. The metres come out of
//! `view::ViewProfile::switch_distance_m` and are rewritten only when that profile moves — a
//! resolution change, a render-scale change, an optic/fov change, a pixel-budget change — with
//! hysteresis on the field (ADR-0033 §11). Bevy's render table retains every distinct
//! `VisibilityRange` slot for the app's lifetime, so per-frame threshold mutation is off the table
//! by construction.
//!
//! Shadows inherit observer-based range selection; no shadow-specific derivation exists.
//!
//! # Who mounts what
//!
//! [`sim_plugin`] runs on every composition (server, net client, single player): it loads the
//! certificate and fingerprints `<id>.sim.glb`, the artifact the ballistic walk reads on both ends.
//! [`view_plugin`] runs on the windowed compositions only: it fingerprints `<id>.glb` and mounts
//! the chain resolution and the range writers.

pub(crate) mod certificate;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::{Gltf, GltfMesh, GltfMeshName};
use bevy::prelude::*;

pub(crate) use certificate::{Chain, TrioMember};

use crate::view::ViewProfile;

/// The shipped tank's asset id. The trio lives at `assets/<id>/<id>.{glb,sim.glb,lod.json}`.
pub(crate) const TIGER_ID: &str = "tiger_1";

/// The certificate key of one source primitive: `<meshName>#<primitiveIndex>`.
pub(crate) fn chain_key(mesh: &str, primitive: usize) -> String {
    format!("{mesh}#{primitive}")
}

/// The loaded certificate, shared. Present on every composition from `Startup`.
#[derive(Resource, Clone)]
pub(crate) struct TankCertificate(pub(crate) Arc<certificate::Certificate>);

impl TankCertificate {
    /// The chain the certificate names for a key, or `None` — a primitive with no chain renders at
    /// source detail.
    pub(crate) fn chain(&self, key: &str) -> Option<&Chain> {
        self.0.chains.get(key)
    }
}

/// Which level of which chain a render entity is: `0` is the source primitive, `1 + i` is
/// `chain.rungs[i]`. Written on rung 0 and on every sibling, so the adaptive layer can rewrite a
/// range without re-deriving which entity is what.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct GeometryLodLevel {
    /// Index into [`GeometryLodChains::chains`].
    pub(crate) chain: usize,
    /// Position in the chain's ladder, rung 0 included.
    pub(crate) level: usize,
}

/// This scene primitive has been through the chain scan. Primitives the certificate does not name
/// carry this and nothing else.
#[derive(Component)]
struct GeometryLodScanned;

/// AN INSTRUMENT DRAWS WHAT IT MEASURES: this composition renders every certified surface at the
/// level the artist authored, at every distance.
///
/// Inserted by the SANDBOXES, and by nothing that has a player. They are the tools the belt wrap,
/// the suspension envelope, the tooth mesh and the penetration march are certified with, and the
/// calibrations they carry are millimetre-scale — `track_sandbox::suspension_viz` asks a person to
/// read a tooth against a tick, and the shoe's first rung deviates 3.7 mm, which is inside the
/// resolution of that check. An instrument must not silently reduce the thing it is used to
/// certify, and it does not need the ladder for anything: nothing is measured about its own frame
/// cost.
///
/// BOTH ladders the certificate feeds obey it — [`attach_rungs`] spawns no sibling and writes no
/// band, and `track::link_view`'s belt selector is not scheduled, so a pooled shoe keeps the
/// authored mesh it spawns with. The terrain ladder is a different certificate and has its own
/// clamp (`terrain_lod::TerrainLodClamp`).
#[derive(Resource)]
pub(crate) struct SourceDetailOnly;

// ---------------------------------------------------------------------------------------------
// The plugins
// ---------------------------------------------------------------------------------------------

/// Certificate + sim-artifact fingerprint. Mounted on every composition.
pub(crate) fn sim_plugin(app: &mut App) {
    app.add_systems(Startup, load_certificate);
}

/// The render half: the view artifact's fingerprint, the chain resolution, and the two range
/// writers. Mounted on windowed compositions only.
pub(crate) fn view_plugin(app: &mut App) {
    app.add_systems(
        Startup,
        (verify_view_artifact, request_tank_gltf).after(load_certificate),
    );
    app.add_systems(
        Update,
        (
            resolve_chains.run_if(
                resource_exists::<TankCertificate>
                    .and_then(not(resource_exists::<GeometryLodChains>)),
            ),
            // THE BIND WAITS FOR THE VIEW. A band is metres derived from the profile, so a
            // primitive bound before there are facts would carry thresholds nobody measured — and
            // there is no profile to derive them from anyway (`crate::view`: no default view). A
            // primitive stays unscanned and at source detail until the frame the profile exists,
            // which is the first frame with a window and a declared camera.
            attach_rungs.run_if(
                resource_exists::<GeometryLodChains>
                    .and_then(resource_exists::<ViewProfile>)
                    .and_then(not(resource_exists::<SourceDetailOnly>)),
            ),
            compose_view_profile.run_if(resource_exists::<crate::view::ViewFacts>),
            adapt_bands
                .run_if(resource_exists_and_changed::<ViewProfile>)
                .after(compose_view_profile),
        )
            .chain()
            // The facts are read once per frame, upstream of every consumer: without this the
            // profile would compose from LAST frame's view and the bands would lag a resize.
            .after(crate::view::track_view_facts),
    );
}

/// Read `<id>.lod.json` and fingerprint the artifact the walk reads.
///
/// The sim artifact is the one file BOTH ends open, so both ends check it. Failure is ADR-0011's:
/// a panic in every build, naming the file.
pub(crate) fn load_certificate(mut commands: Commands) {
    let root = crate::assets::asset_root();
    let certificate = certificate::load(&root, TIGER_ID);
    certificate::verify_member(&certificate, &root, TIGER_ID, TrioMember::Sim);
    info!(
        "geometry_lod: certificate loaded — {} chain(s) over {} scene mesh(es), sim artifact \
         verified",
        certificate.chains.len(),
        certificate.mesh_count,
    );
    commands.insert_resource(TankCertificate(Arc::new(certificate)));
}

/// Fingerprint the view artifact. Only a composition that renders opens it.
fn verify_view_artifact(certificate: Res<TankCertificate>) {
    let root = crate::assets::asset_root();
    certificate::verify_member(&certificate.0, &root, TIGER_ID, TrioMember::View);
}

/// The whole-glTF handle the rung meshes are read from — the SAME file the scene is instantiated
/// from, so the rung records and the primitives they reduce come out of one load.
#[derive(Resource)]
struct TankGltf(Handle<Gltf>);

fn request_tank_gltf(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(TankGltf(
        asset_server.load(certificate_glb_path(TrioMember::View)),
    ));
}

/// `<id>/<id><suffix>` as the asset server addresses it.
pub(crate) fn certificate_glb_path(member: TrioMember) -> String {
    format!("{TIGER_ID}/{TIGER_ID}{}", member.suffix())
}

// ---------------------------------------------------------------------------------------------
// Chain resolution
// ---------------------------------------------------------------------------------------------

/// One chain, joined to the meshes it addresses.
struct ResolvedChain {
    /// The certificate key, `<meshName>#<primitiveIndex>`.
    key: String,
    /// The certificate's own record: bounding radius and ordered deviations.
    chain: Chain,
    /// One handle per rung, in chain order — the embedded rung mesh records, by name.
    rungs: Vec<Handle<Mesh>>,
}

/// Every chain the certificate names, joined to the loaded glTF.
///
/// The join is by MESH ASSET, not by node name: the certificate keys a chain on the source
/// primitive, and glTF meshes are shared between nodes (the Tiger's eight road-wheel nodes consume
/// one wheel chain), so an asset id is exactly the identity a chain is per.
#[derive(Resource)]
pub(crate) struct GeometryLodChains {
    chains: Vec<ResolvedChain>,
    by_mesh: HashMap<AssetId<Mesh>, usize>,
}

impl GeometryLodChains {
    /// The chain a spawned primitive's mesh belongs to, or `None` — source detail.
    pub(crate) fn of_mesh(&self, mesh: AssetId<Mesh>) -> Option<ChainRef<'_>> {
        self.by_mesh.get(&mesh).map(|&index| ChainRef {
            index,
            resolved: &self.chains[index],
        })
    }

    /// The chain at `index` — the value [`GeometryLodLevel::chain`] carries.
    pub(crate) fn at(&self, index: usize) -> Option<ChainRef<'_>> {
        self.chains
            .get(index)
            .map(|resolved| ChainRef { index, resolved })
    }
}

/// One resolved chain, borrowed.
#[derive(Clone, Copy)]
pub(crate) struct ChainRef<'a> {
    index: usize,
    resolved: &'a ResolvedChain,
}

impl<'a> ChainRef<'a> {
    /// Index into the resolved set — what a [`GeometryLodLevel`] records.
    pub(crate) fn index(self) -> usize {
        self.index
    }

    /// The certificate key.
    pub(crate) fn key(self) -> &'a str {
        &self.resolved.key
    }

    /// The certificate's record.
    pub(crate) fn chain(self) -> &'a Chain {
        &self.resolved.chain
    }

    /// One mesh handle per rung, in chain order.
    pub(crate) fn rungs(self) -> &'a [Handle<Mesh>] {
        &self.resolved.rungs
    }

    /// The complementary ranges the levels own under `view`, rung 0 first.
    pub(crate) fn bands(self, view: ViewProfile) -> Vec<VisibilityRange> {
        self.resolved.chain.bands(view)
    }
}

/// Join the certificate to the loaded glTF, once.
///
/// Latches on inserting [`GeometryLodChains`]; retries every frame until the asset lands. A rung
/// mesh the certificate names and the glb does not hold is fatal: the artifact already reproduced
/// its recorded sha256, so a missing record means the certificate and the bytes disagree about
/// what is in the file.
fn resolve_chains(
    mut commands: Commands,
    certificate: Res<TankCertificate>,
    handle: Res<TankGltf>,
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
) {
    // A LOAD THAT FAILED IS NOT A LOAD STILL IN FLIGHT. The bytes already reproduced the recorded
    // sha256, so a refusal here is the loader refusing the artifact itself — and a retry loop would
    // spend the session waiting for a file that will never arrive, with every chained primitive
    // silently at source detail. Same law as the certificate (ADR-0011).
    if let bevy::asset::LoadState::Failed(err) = asset_server.load_state(&handle.0) {
        panic!(
            "geometry_lod: {} matched the certificate's view_glb_sha and then failed to load              ({err}) — the file is the certified one and the loader cannot read it",
            certificate_glb_path(TrioMember::View),
        );
    }
    let Some(gltf) = gltfs.get(&handle.0) else {
        return;
    };
    let mesh_handle = |name: &str| -> Option<Handle<Mesh>> {
        let gltf_mesh = gltf_meshes.get(gltf.named_meshes.get(name)?)?;
        gltf_mesh.primitives.first().map(|p| p.mesh.clone())
    };

    let mut chains: Vec<ResolvedChain> = Vec::new();
    let mut by_mesh: HashMap<AssetId<Mesh>, usize> = HashMap::new();
    for source in &gltf.meshes {
        let Some(source) = gltf_meshes.get(source) else {
            return;
        };
        for primitive in &source.primitives {
            let key = chain_key(&source.name, primitive.index);
            let Some(chain) = certificate.chain(&key) else {
                continue;
            };
            let rungs: Vec<Handle<Mesh>> = chain
                .rungs
                .iter()
                .map(|rung| {
                    mesh_handle(&rung.mesh).unwrap_or_else(|| {
                        panic!(
                            "geometry_lod: the certificate's chain `{key}` names the rung mesh \
                             `{}` and {} holds no such mesh record, though its bytes reproduce the \
                             recorded sha256 — rebuild the trio",
                            rung.mesh,
                            certificate_glb_path(TrioMember::View),
                        )
                    })
                })
                .collect();
            by_mesh.insert(primitive.mesh.id(), chains.len());
            chains.push(ResolvedChain {
                key,
                chain: chain.clone(),
                rungs,
            });
        }
    }

    let rungs: usize = chains.iter().map(|c| c.rungs.len()).sum();
    info!(
        "geometry_lod: {} chain(s) resolved against {}, {rungs} rung mesh(es) bound; {} scene \
         primitive(s) render at source detail",
        chains.len(),
        certificate_glb_path(TrioMember::View),
        gltf.meshes
            .iter()
            .filter_map(|m| gltf_meshes.get(m))
            .map(|m| m.primitives.len())
            .sum::<usize>()
            .saturating_sub(chains.len()),
    );
    commands.insert_resource(GeometryLodChains { chains, by_mesh });
}

// ---------------------------------------------------------------------------------------------
// The bind
// ---------------------------------------------------------------------------------------------

/// Give every freshly spawned scene primitive its band, and one coincident sibling per rung.
///
/// `GltfMeshName` is the scene-primitive discriminator: it is written by the glTF loader on
/// primitive leaves and by nothing else, so a pooled mesh (the track's shoes) and the rung siblings
/// spawned below are never rescanned. `Without<GeometryLodScanned>` rather than `Added<Mesh3d>`
/// because the chain resolution and the scene instantiation land on whichever frame the asset
/// server finishes them.
fn attach_rungs(
    mut commands: Commands,
    chains: Res<GeometryLodChains>,
    view: Res<ViewProfile>,
    fresh: Query<
        (
            Entity,
            &Mesh3d,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&Name>,
        ),
        (With<GltfMeshName>, Without<GeometryLodScanned>),
    >,
) {
    for (entity, mesh, material, name) in &fresh {
        let mut primitive = commands.entity(entity);
        primitive.insert(GeometryLodScanned);
        let Some(chain) = chains.of_mesh(mesh.0.id()) else {
            continue;
        };
        let mut bands = chain.bands(*view);
        primitive.insert((
            GeometryLodLevel {
                chain: chain.index(),
                level: 0,
            },
            bands.remove(0),
        ));
        let Some(material) = material else {
            // A rung carries no material of its own; without one to clone it would render as the
            // glTF default while its source does not.
            warn_once!(
                "geometry_lod: `{}` matches chain `{}` and carries no StandardMaterial — its \
                 rungs are not bound, so it renders at source detail",
                name.map_or("<unnamed>", Name::as_str),
                chain.key(),
            );
            continue;
        };
        let material = material.0.clone();
        let index = chain.index();
        let rungs: Vec<(Handle<Mesh>, VisibilityRange)> =
            chain.rungs().iter().cloned().zip(bands).collect();
        primitive.with_children(|parent| {
            for (level, (mesh, band)) in rungs.into_iter().enumerate() {
                parent.spawn((
                    GeometryLodLevel {
                        chain: index,
                        level: level + 1,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material.clone()),
                    band,
                    // IDENTITY: every rung is authored in its source primitive's mesh-local frame,
                    // so riding the parent puts it exactly where the source was.
                    Transform::IDENTITY,
                ));
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// The adaptive layer
// ---------------------------------------------------------------------------------------------

/// Compose this ladder's [`ViewProfile`]: the shared [`ViewFacts`] spent at the player's pixel
/// budget.
///
/// NOT A READER OF THE VIEW. `view::track_view_facts` is the one system that reads a `Projection`,
/// a `Window` and the render scale, and it already carries the dead band on the field; what is
/// local to this ladder is the BUDGET (`settings::LodPixelBudget`, the detail row), which is a
/// tuning knob and not view state. The profile therefore moves on exactly two human-rate events:
/// the facts moved, or the player moved the row.
///
/// It INSERTS the profile rather than updating a default one, and is scheduled behind the facts
/// existing: there is no view to spend a budget on until somebody has read the declared camera, so
/// this ladder has no profile either (`crate::view`: no default view).
pub(crate) fn compose_view_profile(
    mut commands: Commands,
    facts: Res<crate::view::ViewFacts>,
    budget: Option<Res<crate::settings::LodPixelBudget>>,
    view: Option<ResMut<ViewProfile>>,
) {
    let wanted = ViewProfile::of(
        *facts,
        budget.map_or(crate::view::DEFAULT_BUDGET_PX, |budget| budget.0),
    );
    match view {
        Some(mut view) => {
            if *view == wanted {
                return;
            }
            *view = wanted;
        }
        None => commands.insert_resource(wanted),
    }
    info!(
        "geometry_lod: view profile → {:.4} rad × {:.0} px at {:.2} px budget, bands rewritten",
        wanted.facts.vfov_rad, wanted.facts.height_px, wanted.budget_px,
    );
}

/// Rewrite every band on a profile move. THE ONE WRITER, apart from the bind.
///
/// `GeometryLodLevel` is exactly the set of entities bevy selects for. A pooled track shoe carries
/// none: its level is a fact its BELT owns and rewrites (`track::link_view`), and one on a shoe
/// would be a second writer of it.
fn adapt_bands(
    mut commands: Commands,
    chains: Option<Res<GeometryLodChains>>,
    view: Res<ViewProfile>,
    levels: Query<(Entity, &GeometryLodLevel)>,
) {
    let Some(chains) = chains else {
        return;
    };
    for (entity, level) in &levels {
        let Some(chain) = chains.at(level.chain) else {
            continue;
        };
        let Some(band) = chain.bands(*view).into_iter().nth(level.level) else {
            continue;
        };
        commands.entity(entity).insert(band);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::ViewFacts;

    /// The reference view every certified distance was quoted in when the corpus was cut: the
    /// gunner optic at 4K native, one pixel of budget (`scripts/lod/config.py::REFERENCE_VIEW`).
    pub(crate) fn reference_view() -> ViewProfile {
        ViewProfile::of(
            ViewFacts::new(crate::camera::GUNNER_FOV_FALLBACK, 2160.0)
                .expect("the reference view is measured"),
            1.0,
        )
    }

    /// EVERY RUNG MESH RECORD SHIPS TANGENTS.
    ///
    /// A rung carries no glTF material, which is exactly the case `bevy_gltf` does not generate
    /// tangents for — and it renders under its SOURCE's material, which on this model is
    /// normal-mapped. bevy's PBR shader drops normal mapping, silently, on a mesh with no
    /// `ATTRIBUTE_TANGENT`, so an untangented rung would change the LIGHTING as well as the
    /// silhouette and the certified deviation is a claim about the silhouette alone.
    #[test]
    fn every_shipped_rung_record_carries_tangents() {
        let root = crate::assets::asset_root();
        let certificate = certificate::load(&root, TIGER_ID);
        let path = certificate::member_path(&root, TIGER_ID, TrioMember::View);
        let gltf::Gltf { document, .. } = gltf::Gltf::open(&path).expect("the view glb opens");
        let named: std::collections::HashMap<&str, gltf::Mesh> = document
            .meshes()
            .filter_map(|mesh| mesh.name().map(|name| (name, mesh)))
            .collect();
        for (key, chain) in &certificate.chains {
            for rung in &chain.rungs {
                let mesh = named
                    .get(rung.mesh.as_str())
                    .unwrap_or_else(|| panic!("{key}: the view glb holds no `{}`", rung.mesh));
                for primitive in mesh.primitives() {
                    assert!(
                        primitive.get(&gltf::Semantic::Tangents).is_some(),
                        "{key}: rung `{}` ships no TANGENT accessor — it renders under its \
                         source's normal-mapped material and would light flat",
                        rung.mesh,
                    );
                    assert!(
                        primitive.material().index().is_none(),
                        "{key}: rung `{}` carries a material of its own — a rung wears its \
                         source primitive's material at bind time",
                        rung.mesh,
                    );
                }
            }
        }
    }

    /// THE SHIPPED CORPUS REACHES THE WORLD'S FAR CORNER, derived rather than transcribed.
    ///
    /// The ladder's depth is a property of generation: the build stops cutting rungs once a level's
    /// switch distance passes the right wall — the world's diagonal, beyond which no camera can
    /// stand. A map that GROWS invalidates the corpus in the one way nothing else notices, so the
    /// two are compared where a map edit and the certificate meet.
    #[test]
    fn the_shipped_chains_span_the_worlds_far_corner() {
        let root = crate::assets::asset_root();
        let certificate = certificate::load(&root, TIGER_ID);
        let extent = crate::map::load(&root)
            .map(|manifest| manifest.extent)
            .unwrap_or(crate::terrain_grid::FIXTURE_EXTENT);
        let diagonal_m = extent.world_size_m * std::f32::consts::SQRT_2;
        let view = reference_view();

        // A chain reaches the corner when its LAST rung opens at or before the diagonal, OR when
        // the generator could cut no deeper (the certificate cannot say which; what it can say is
        // that no chain's coarsest band opens beyond the far corner AND leaves the world with a
        // finer level than the ladder was cut for). The tripwire is the first form: at least one
        // chain must still have its last switch inside the world, or the whole corpus was cut for
        // a smaller map than the one that loads.
        let deepest = certificate
            .chains
            .values()
            .filter_map(|chain| chain.rungs.last().map(|rung| (chain, rung)))
            .map(|(chain, rung)| view.switch_distance_m(rung.deviation_m(), chain.radius_m))
            .fold(f32::INFINITY, f32::min);
        assert!(
            deepest <= diagonal_m,
            "every chain's last switch opens beyond the world's {diagonal_m:.1} m diagonal (the \
             nearest is {deepest:.1} m), so the corpus was cut for a smaller map than the one that \
             loads — rebuild the trio with `python3 scripts/tank/build.py build \
             assets/{TIGER_ID}/{TIGER_ID}.blend`",
        );
    }
}
