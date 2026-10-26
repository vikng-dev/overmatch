//! Trees in the game's damage model: a hard body with an HP pool and a state. Game rounds dose it,
//! tanks push it, and it falls at a segment.
//!
//! A TREE IS DATA (ADR-0034). `assets/trees/<id>/<id>.tree.ron` ([`TreeAsset`]) is the trunk as a
//! chain of tapered segments; `assets/trees/species.ron` ([`Species`]) is what the sim reads of a
//! species: its substance, the HP pool and the ram threshold. Both are read once at startup into
//! the map's [`TreeLibrary`] and validated there; every number the sim uses comes from them, the
//! glb is the view (ADR-0014).
//!
//! THE BODY. One static root per tree — the [`BallisticVolume`] of the species' substance and the
//! HP pool, the way a module's pool is charged — and one child per segment with its own convex
//! frustum collider (two 12-gon rings) on `[Terrain, Armor]` with filters `ALL`: hulls contact it
//! and the union walk charges chord × factor and doses the pool. Never a compound: the walk's
//! convex collector reads one solid per collider. The instance's uniform scale is applied to the
//! dimensions at spawn and the root's own scale is one.
//!
//! THE STATE. [`TreeState`] is `Standing` or `Down { cut, dir }`: the segment the trunk broke at
//! and the fall azimuth's bin. The authority sets it once — when the pool empties
//! ([`PoolEmptied`], at the dose's own entry height and direction) or when a ram's torque beats
//! `fell_k × dbh³` — and every peer derives the rest from it.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::{FRAC_PI_2, PI};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::{Collider, CollisionLayers, Collisions, LayerMask, RigidBody};
use avian3d::schedule::PhysicsSystems;
use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};

use crate::Layer;
use crate::ballistics::{BallisticVolume, ComponentHealth, PoolEmptied, VolumeMaterial};
use crate::provenance::Sourced;
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;
use crate::terrain_grid::HeightGrid;
use crate::{ClientReplica, ShotClock};

/// Directory under the asset root holding the species table and one folder per tree asset.
const TREES_DIR: &str = "trees";

/// The species table's file name inside [`TREES_DIR`].
const SPECIES_FILE: &str = "species.ron";

/// How far below 1 an instance's up vector's Y may fall before the instance counts as tilted:
/// float rounding of a pure yaw (a few ulps), never a real lean — the cut reads world heights
/// against upright prefix sums, so a leaning trunk would cut at the wrong segment.
const UPRIGHT_EPSILON: f32 = 1e-6;

/// The height above the base the breast-height diameter is read at, metres at asset scale 1.
pub(crate) const BREAST_HEIGHT_M: f32 = 1.3;

/// Most segments a trunk may have: `cut` travels as one byte, and the wire word reserves
/// `cut + 1 == 0` for a standing tree.
const MAX_SEGMENTS: usize = 255;

/// The fall azimuth's resolution: bins round the compass, from `+X` toward `+Z`.
pub(crate) const AZIMUTH_BINS: u32 = 256;

/// One trunk segment: a frustum `height_m` tall, `r_bottom_m` at its base and `r_top_m` at its
/// top, metres at asset scale 1.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Segment {
    pub(crate) height_m: f32,
    pub(crate) r_bottom_m: f32,
    pub(crate) r_top_m: f32,
}

/// `<id>.tree.ron`: the trunk, base to tip, and the species it belongs to.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TreeAsset {
    pub(crate) species: String,
    pub(crate) segments: Vec<Segment>,
}

/// What the sim reads of a `species.ron` row. The row also carries the generator's look fields,
/// which are not read here.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct Species {
    substance: String,
    /// The ram threshold's constant, N·s/m²: a tick's `Σ impulse × lever` fells the trunk at
    /// `fell_k × dbh³`.
    fell_k: Sourced<f32>,
    /// The trunk's HP pool, species-fixed.
    hp: Sourced<u32>,
}

/// One tree asset resolved against its species and the substance registry, validated: what every
/// instance of it spawns from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TreeKind {
    pub(crate) id: String,
    pub(crate) segments: Vec<Segment>,
    pub(crate) material: VolumeMaterial,
    pub(crate) substance: String,
    pub(crate) fell_k: f32,
    pub(crate) hp: u32,
    /// Breast-height diameter at asset scale 1, metres: twice the taper's radius at
    /// [`BREAST_HEIGHT_M`] (DERIVED at load).
    pub(crate) dbh_m: f32,
}

impl TreeKind {
    /// Resolve and validate one asset. Every refusal is a broken ship (ADR-0011): the message
    /// names what, the caller names the file.
    fn resolve(
        id: &str,
        asset: TreeAsset,
        table: &BTreeMap<String, Species>,
        registry: &SubstanceRegistry,
    ) -> Result<Self, String> {
        let TreeAsset { species, segments } = asset;
        if segments.is_empty() || segments.len() > MAX_SEGMENTS {
            return Err(format!(
                "{} segments; a trunk has 1 to {MAX_SEGMENTS}",
                segments.len()
            ));
        }
        for (k, segment) in segments.iter().enumerate() {
            let positive = |v: f32| v.is_finite() && v > 0.0;
            if !(positive(segment.height_m)
                && positive(segment.r_bottom_m)
                && positive(segment.r_top_m))
            {
                return Err(format!(
                    "segment {k} has a non-positive dimension: {segment:?}"
                ));
            }
            if let Some(next) = segments.get(k + 1)
                && segment.r_top_m != next.r_bottom_m
            {
                return Err(format!(
                    "segment {k} tops at r {} and segment {} starts at r {}; the taper is \
                     continuous",
                    segment.r_top_m,
                    k + 1,
                    next.r_bottom_m,
                ));
            }
        }
        let row = table
            .get(&species)
            .ok_or_else(|| format!("species {species:?} is not in {SPECIES_FILE}"))?;
        let material = VolumeMaterial::of(registry, &row.substance)
            .map_err(|err| format!("species {species:?} substance: {err}"))?;
        let (fell_k, hp) = (row.fell_k.0, row.hp.0);
        if !(fell_k.is_finite() && fell_k > 0.0) {
            return Err(format!(
                "species {species:?} fell_k {fell_k} is not positive"
            ));
        }
        if hp == 0 {
            return Err(format!("species {species:?} hp is zero"));
        }
        let mut kind = Self {
            id: id.to_owned(),
            segments,
            material,
            substance: row.substance.clone(),
            fell_k,
            hp,
            dbh_m: 0.0,
        };
        if kind.height_m() <= BREAST_HEIGHT_M {
            return Err(format!(
                "{} m tall, shorter than breast height ({BREAST_HEIGHT_M} m)",
                kind.height_m()
            ));
        }
        kind.dbh_m = 2.0 * kind.radius_at(BREAST_HEIGHT_M);
        Ok(kind)
    }

    /// Base to tip, metres at asset scale 1.
    pub(crate) fn height_m(&self) -> f32 {
        self.segments.iter().map(|segment| segment.height_m).sum()
    }

    /// The trunk's radius `height` metres above the base along the authored taper, at asset scale
    /// 1; the base radius below the base, the tip radius above the tip.
    pub(crate) fn radius_at(&self, height: f32) -> f32 {
        let mut bottom = 0.0;
        for segment in &self.segments {
            let top = bottom + segment.height_m;
            if height < top {
                let t = ((height - bottom) / segment.height_m).clamp(0.0, 1.0);
                return segment.r_bottom_m + (segment.r_top_m - segment.r_bottom_m) * t;
            }
            bottom = top;
        }
        self.segments
            .last()
            .expect("resolve refused an empty trunk")
            .r_top_m
    }
}

/// Every tree asset the selected map places, resolved and validated, keyed by asset id.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct TreeLibrary {
    kinds: BTreeMap<String, Arc<TreeKind>>,
    /// The texts the kinds were read from — the species table first, then each asset in id order
    /// — for the map's content digest.
    texts: Vec<String>,
}

impl TreeLibrary {
    /// Read and validate the assets `ids` name under `root`. Nothing is read when `ids` is empty;
    /// otherwise every file must exist and hold (ADR-0011): a map that places a tree this build
    /// cannot read is present-but-broken.
    pub(crate) fn load<'a>(root: &Path, ids: impl IntoIterator<Item = &'a str>) -> Self {
        let ids: BTreeSet<&str> = ids.into_iter().collect();
        if ids.is_empty() {
            return Self::default();
        }
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("trees: {} cannot be read: {err}", path.display()))
        };
        let species_path = root.join(TREES_DIR).join(SPECIES_FILE);
        let species_text = read(&species_path);
        let table: BTreeMap<String, Species> = ron::from_str(&species_text).unwrap_or_else(|err| {
            panic!("trees: {} failed to parse: {err}", species_path.display())
        });
        let registry = SubstanceRegistry::shipped_static();
        let mut texts = vec![species_text];
        let mut kinds = BTreeMap::new();
        for id in ids {
            let path = root.join(TREES_DIR).join(id).join(format!("{id}.tree.ron"));
            let text = read(&path);
            let asset: TreeAsset = ron::from_str(&text)
                .unwrap_or_else(|err| panic!("trees: {} failed to parse: {err}", path.display()));
            let kind = TreeKind::resolve(id, asset, &table, registry)
                .unwrap_or_else(|err| panic!("trees: {}: {err}", path.display()));
            texts.push(text);
            kinds.insert(id.to_owned(), Arc::new(kind));
        }
        Self { kinds, texts }
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Arc<TreeKind>> {
        self.kinds.get(id)
    }

    /// The bytes of every file the library was read from, in a fixed order.
    pub(crate) fn texts(&self) -> impl Iterator<Item = &[u8]> {
        self.texts.iter().map(String::as_bytes)
    }
}

/// The tree's state, authority-set, peer-derived (module doc).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeState {
    Standing,
    /// Broken at the bottom of segment `cut`, fallen toward azimuth bin `dir`.
    Down {
        cut: u8,
        dir: u8,
    },
}

impl TreeState {
    /// The state's wire word: `Standing` is `0`, `Down { cut, dir }` is `((cut + 1) << 8) | dir`.
    /// The `+ 1` keeps every `Down` word non-zero, so the high byte alone tells the two apart and
    /// `Standing` has exactly one encoding.
    pub(crate) fn encode(self) -> u16 {
        match self {
            Self::Standing => 0,
            Self::Down { cut, dir } => {
                assert!(
                    usize::from(cut) < MAX_SEGMENTS,
                    "tree cut {cut} has no word: the high byte carries cut + 1"
                );
                ((u16::from(cut) + 1) << 8) | u16::from(dir)
            }
        }
    }

    /// The inverse of [`Self::encode`] for a tree of `segments` segments; a word that no state
    /// of that tree encodes to (`Standing` with direction bits, a cut at or past the last segment)
    /// is refused rather than clamped, so a peer never derives a pose the authority did not set.
    pub(crate) fn decode(word: u16, segments: usize) -> Result<Self, String> {
        let (high, dir) = (word >> 8, (word & 0xFF) as u8);
        if high == 0 {
            return if word == 0 {
                Ok(Self::Standing)
            } else {
                Err(format!(
                    "tree word {word:#06x}: standing with direction bits"
                ))
            };
        }
        let cut = high - 1;
        if usize::from(cut) >= segments {
            return Err(format!(
                "tree word {word:#06x}: cut {cut} on a tree of {segments} segments"
            ));
        }
        Ok(Self::Down {
            cut: cut as u8,
            dir,
        })
    }
}

/// A tree's body as spawned: its kind at the instance's uniform scale, every dimension here in
/// world metres.
#[derive(Component, Debug, Clone)]
pub(crate) struct TreeBody {
    pub(crate) kind: Arc<TreeKind>,
    /// Segment bottoms above the base, then the tip: `prefix[k]` is segment `k`'s bottom,
    /// `prefix[n]` the tip.
    pub(crate) prefix: Vec<f32>,
    /// Breast-height diameter: the kind's at the instance's scale.
    pub(crate) dbh_m: f32,
    /// The instance's uniform scale, asset metres to world metres.
    pub(crate) scale: f32,
}

impl TreeBody {
    pub(crate) fn new(kind: Arc<TreeKind>, scale: f32) -> Self {
        let mut prefix = Vec::with_capacity(kind.segments.len() + 1);
        let mut bottom = 0.0;
        for segment in &kind.segments {
            prefix.push(bottom);
            bottom += segment.height_m * scale;
        }
        prefix.push(bottom);
        let dbh_m = kind.dbh_m * scale;
        Self {
            kind,
            prefix,
            dbh_m,
            scale,
        }
    }

    /// The trunk's world radius `height` world metres above the base.
    pub(crate) fn radius_at(&self, height: f32) -> f32 {
        self.kind.radius_at(height / self.scale) * self.scale
    }

    /// The segment a point `height` world metres above the base lies in. A height on a boundary
    /// belongs to the UPPER segment; below the base is the first, past the tip the last.
    pub(crate) fn cut_at(&self, height: f32) -> u8 {
        let n = self.prefix.len() - 1;
        let above = self.prefix[1..n]
            .iter()
            .filter(|&&bottom| height >= bottom)
            .count();
        u8::try_from(above).expect("resolve bounded the segment count")
    }

    /// The torque (N·s·m per tick) a ram must put on the trunk to fell it: `fell_k × dbh³`.
    pub(crate) fn fell_torque(&self) -> f32 {
        self.kind.fell_k * self.dbh_m * self.dbh_m * self.dbh_m
    }
}

/// Marks a segment's collider entity, a child of the tree's root.
#[derive(Component)]
pub(crate) struct TreeSegment;

/// Marks the glb view's root, a child of the tree's root.
#[derive(Component)]
pub(crate) struct TreeView;

/// The authority tick a tree's state last changed at (`ShotClock` units, the wire's `tick`).
/// Inserted beside the `Down` state by whoever sets it — the authority's laws here, the ledger
/// apply on a replica — so the presentation fuses the fall to the tick it happened at.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fell {
    pub(crate) tick: u32,
}

/// The set the authority's fall laws run in; the ledger publish and the body derive order after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TreeFallSet;

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

/// A fallen tree's resting pose, derived once from its state: the piece above `cut` lies tilted
/// by `tilt` radians from upright toward the horizontal unit direction `(dx, dz)`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct Resting {
    pub(crate) tilt: f32,
    pub(crate) dx: f32,
    pub(crate) dz: f32,
}

/// The largest tilt a fallen piece is searched over: past the horizontal by 10°, for a piece
/// hanging over a downhill edge.
const MAX_TILT: f32 = 100.0 * PI / 180.0;

/// The bisection steps of the resting tilt: 24 halvings of [0°, 100°] resolve below 1e-5 rad.
const TILT_STEPS: u32 = 24;

/// The horizontal unit direction of azimuth bin `dir`, libm's trig so every peer gets the same
/// bits.
pub(crate) fn azimuth(dir: u8) -> (f32, f32) {
    let theta = f32::from(dir) / AZIMUTH_BINS as f32 * std::f32::consts::TAU;
    (libm::cosf(theta), libm::sinf(theta))
}

/// The tilt a fallen piece rests at: the smallest angle in [0°, 100°] at which any sample of the
/// piece — each segment boundary above the cut and the tip, each offset downward by its local
/// radius — reaches the terrain, by [`TILT_STEPS`] bisections in a fixed order; a piece touching
/// nothing by 100° lies at 90°. Pure in its inputs (the body, the world base, the decoded state,
/// the terrain oracle) and libm-only in its trig, so every peer and CI derive the same bits.
pub(crate) fn resting_tilt(
    body: &TreeBody,
    base: Vec3,
    cut: u8,
    dir: u8,
    height_at: impl Fn(f32, f32) -> f32,
) -> f32 {
    let cut = usize::from(cut);
    let pivot = base + Vec3::Y * body.prefix[cut];
    let (dx, dz) = azimuth(dir);
    let touching = |tilt: f32| {
        let (sin, cos) = (libm::sinf(tilt), libm::cosf(tilt));
        (cut + 1..body.prefix.len()).any(|j| {
            let h = body.prefix[j] - body.prefix[cut];
            let r = body.radius_at(body.prefix[j]);
            let x = pivot.x + dx * h * sin;
            let y = pivot.y + h * cos - r;
            let z = pivot.z + dz * h * sin;
            y <= height_at(x, z)
        })
    };
    if touching(0.0) {
        return 0.0;
    }
    if !touching(MAX_TILT) {
        return FRAC_PI_2;
    }
    let (mut lo, mut hi) = (0.0_f32, MAX_TILT);
    for _ in 0..TILT_STEPS {
        let mid = 0.5 * (lo + hi);
        if touching(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// The world rotation that tilts an upright piece by `tilt` toward `(dx, dz)`: about the
/// horizontal axis `(dz, 0, -dx)`, built from libm's half-angle trig rather than glam's.
fn tilt_rotation(resting: &Resting) -> Quat {
    let (s, c) = (
        libm::sinf(0.5 * resting.tilt),
        libm::cosf(0.5 * resting.tilt),
    );
    Quat::from_xyzw(resting.dz * s, 0.0, -resting.dx * s, c)
}

/// Every peer derives the body from the state (ADR-0016): a fallen tree's stump keeps its
/// segments standing on `[Terrain, Armor]`, the lying piece comes to the resting tilt on
/// `[Armor]` alone — a hard body for the walk, no longer an obstacle for a hull — and the pool is
/// gone: what is down takes no more dose. Standing derives nothing beyond what spawn built.
fn derive_state(
    trees: Query<
        (Entity, &TreeState, &TreeBody, &Transform, &Children),
        (Changed<TreeState>, Without<TreeSegment>),
    >,
    mut segments: Query<&mut Transform, With<TreeSegment>>,
    grid: Option<Res<HeightGrid>>,
    mut commands: Commands,
) {
    // Trees are placed by the map whose grid this is; a composition without the grid (the flat
    // slab) has no trees to derive.
    let Some(grid) = grid else {
        return;
    };
    for (root, state, body, transform, children) in &trees {
        let TreeState::Down { cut, dir } = *state else {
            continue;
        };
        let tilt = resting_tilt(body, transform.translation, cut, dir, |x, z| {
            grid.height_at(x, z)
        });
        let (dx, dz) = azimuth(dir);
        let resting = Resting { tilt, dx, dz };
        let rotation = tilt_rotation(&resting);
        let root_inv = transform.rotation.inverse();
        let cut = usize::from(cut);
        let pivot = Vec3::Y * body.prefix[cut];
        let mut k = 0;
        for &child in children {
            let Ok(mut local) = segments.get_mut(child) else {
                continue;
            };
            if k >= cut {
                let along = rotation * Vec3::Y * (body.prefix[k] - body.prefix[cut]);
                local.translation = root_inv * (pivot + along);
                local.rotation = root_inv * rotation;
                commands
                    .entity(child)
                    .insert(CollisionLayers::new([Layer::Armor], LayerMask::NONE));
            }
            k += 1;
        }
        commands
            .entity(root)
            .insert(resting)
            .remove::<ComponentHealth>();
    }
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

/// The 12-gon the frustum colliders are hulled from: exact unit-circle points at 30° steps, pinned
/// as literals so every peer builds the same solid.
const RING: [(f32, f32); 12] = [
    (1.0, 0.0),
    (0.866_025_4, 0.5),
    (0.5, 0.866_025_4),
    (0.0, 1.0),
    (-0.5, 0.866_025_4),
    (-0.866_025_4, 0.5),
    (-1.0, 0.0),
    (-0.866_025_4, -0.5),
    (-0.5, -0.866_025_4),
    (0.0, -1.0),
    (0.5, -0.866_025_4),
    (0.866_025_4, -0.5),
];

/// One segment's collider in its own frame: base ring at y = 0, top ring at the segment's height.
fn segment_collider(segment: &Segment, scale: f32) -> Collider {
    let (height, r_bottom, r_top) = (
        segment.height_m * scale,
        segment.r_bottom_m * scale,
        segment.r_top_m * scale,
    );
    let points: Vec<Vec3> = RING
        .iter()
        .flat_map(|&(c, s)| {
            [
                Vec3::new(r_bottom * c, 0.0, r_bottom * s),
                Vec3::new(r_top * c, height, r_top * s),
            ]
        })
        .collect();
    Collider::convex_hull(points).expect("two rings of positive radius at distinct heights")
}

/// The azimuth bin of a horizontal `(x, z)` direction, [`AZIMUTH_BINS`] round from `+X` toward
/// `+Z`. A zero vector lands in bin 0. `libm`, so the bin is the same on every platform.
pub(crate) fn azimuth_bin(x: f32, z: f32) -> u8 {
    let turns = libm::atan2f(z, x) / std::f32::consts::TAU;
    // `turns` is in [-0.5, 0.5], so the rounded bin is in [-128, 128] and wraps once.
    let bin = libm::roundf(turns * AZIMUTH_BINS as f32) as i32;
    u8::try_from(bin.rem_euclid(AZIMUTH_BINS as i32)).expect("reduced modulo the bin count")
}

/// Spawn one tree of `kind` at `pose` — the ground pose: the authored XZ, the grid's Y, the
/// authored yaw and uniform scale. A tilted or non-uniformly scaled instance is refused (ADR-0011):
/// the resting pose and the cut both read the trunk as upright. The glb view attaches only where
/// `view` offers a server to load it with.
pub(crate) fn spawn(
    commands: &mut Commands,
    kind: &Arc<TreeKind>,
    identity: MapInstance,
    pose: Transform,
    view: Option<&AssetServer>,
) {
    let scale = pose.scale.x;
    assert!(
        pose.scale.y == scale && pose.scale.z == scale && scale > 0.0,
        "trees: instance {} has scale {:?}; a tree scales uniformly",
        identity.id,
        pose.scale,
    );
    assert!(
        (pose.rotation * Vec3::Y).y >= 1.0 - UPRIGHT_EPSILON,
        "trees: instance {} is tilted ({:?}); a tree stands upright",
        identity.id,
        pose.rotation,
    );
    let body = TreeBody::new(kind.clone(), scale);
    let prefix = body.prefix.clone();
    let root = commands
        .spawn((
            Name::new(format!("tree {}", identity.id)),
            Transform {
                translation: pose.translation,
                rotation: pose.rotation,
                scale: Vec3::ONE,
            },
            RigidBody::Static,
            BallisticVolume {
                material: kind.material,
                substance: kind.substance.clone(),
            },
            ComponentHealth {
                current: kind.hp,
                max: kind.hp,
            },
            TreeState::Standing,
            identity,
            body,
        ))
        .id();
    for (k, segment) in kind.segments.iter().enumerate() {
        commands.spawn((
            ChildOf(root),
            Transform::from_xyz(0.0, prefix[k], 0.0),
            segment_collider(segment, scale),
            CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL),
            TreeSegment,
        ));
    }
    if let Some(assets) = view {
        let id = &kind.id;
        commands
            .spawn((
                ChildOf(root),
                Transform::from_scale(Vec3::splat(scale)),
                WorldAssetRoot(
                    assets
                        .load(GltfAssetLabel::Scene(0).from_asset(format!("trees/{id}/{id}.glb"))),
                ),
                TreeView,
            ))
            .observe(bind_tree_view);
    }
}

/// A standing tree whose pool a dose emptied falls: at the dose's entry height (the segment that
/// height lies in) and along the round's travel; a field (no direction) fells it whole, away from
/// the field's centre. A tree already down ignores it.
fn fell_on_pool_emptied(
    emptied: On<PoolEmptied>,
    mut trees: Query<(&mut TreeState, &TreeBody, &Transform)>,
    clock: Option<Res<ShotClock>>,
    mut commands: Commands,
) {
    let Ok((mut state, body, transform)) = trees.get_mut(emptied.entity) else {
        return;
    };
    if *state != TreeState::Standing {
        return;
    }
    let base = transform.translation;
    let (cut, along) = match emptied.along {
        Some(along) => (body.cut_at(emptied.at.y - base.y), along),
        None => (0, base - emptied.at),
    };
    *state = TreeState::Down {
        cut,
        dir: azimuth_bin(along.x, along.z),
    };
    commands.entity(emptied.entity).insert(Fell {
        tick: clock.map_or(0, |clock| clock.0),
    });
}

/// One hull–trunk contact as the ram law reads it: the solver's normal impulse (N·s), the
/// contact's height above the tree's base (m), and the horizontal push into the tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RamContact {
    pub(crate) impulse: f32,
    pub(crate) lever_m: f32,
    pub(crate) push: Vec2,
}

/// THE RAM LAW: one tick's contacts on one tree, summed — `Σ impulse × lever` (N·s·m) and the
/// impulse-weighted push. Compared once against [`TreeBody::fell_torque`] and never carried into
/// the next tick, so sub-threshold ticks do not add up.
///
/// `impulse` is avian's `ContactPoint::normal_impulse`: the clamped accumulated impulse summed
/// over every solve call of the tick (each substep's bias and relax passes), so a sustained
/// contact reads about twice the momentum it transfers and a one-call stop about once. `fell_k`
/// is calibrated on this reading (`ram-impulses.md`, 2026-10-26: 64 Hz, 6 substeps, the game's
/// own settings), so the law applies no correction.
pub(crate) fn ram_torque(contacts: &[RamContact]) -> (f32, Vec2) {
    contacts
        .iter()
        .fold((0.0, Vec2::ZERO), |(torque, push), contact| {
            (
                torque + contact.impulse * contact.lever_m,
                push + contact.push * contact.impulse,
            )
        })
}

/// The authority's ram rule, read after the solver stored this tick's impulses: a standing tree
/// whose trunk took its [`TreeBody::fell_torque`] of hull torque in one tick falls whole, along
/// the push. The manifold normal runs from the pair's first collider to its second, so it points
/// into the tree exactly when the tree is second.
fn fell_on_ram(
    collisions: Collisions,
    segments: Query<&ChildOf, With<TreeSegment>>,
    mut trees: Query<(&mut TreeState, &TreeBody, &Transform)>,
    clock: Option<Res<ShotClock>>,
    mut commands: Commands,
) {
    let mut per_tree: BTreeMap<Entity, Vec<RamContact>> = BTreeMap::new();
    for pair in collisions.iter() {
        let (tree_first, root) = match (segments.get(pair.collider1), segments.get(pair.collider2))
        {
            (Ok(parent), Err(_)) => (true, parent.parent()),
            (Err(_), Ok(parent)) => (false, parent.parent()),
            _ => continue,
        };
        let Ok((state, _, transform)) = trees.get(root) else {
            continue;
        };
        if *state != TreeState::Standing {
            continue;
        }
        let base_y = transform.translation.y;
        let contacts = per_tree.entry(root).or_default();
        for manifold in &pair.manifolds {
            let into = if tree_first {
                -manifold.normal
            } else {
                manifold.normal
            };
            for point in &manifold.points {
                if point.normal_impulse <= 0.0 {
                    continue;
                }
                contacts.push(RamContact {
                    impulse: point.normal_impulse,
                    lever_m: (point.point.y - base_y).max(0.0),
                    push: into.xz(),
                });
            }
        }
    }
    for (root, contacts) in per_tree {
        let (torque, push) = ram_torque(&contacts);
        let Ok((mut state, body, _)) = trees.get_mut(root) else {
            continue;
        };
        if torque >= body.fell_torque() {
            *state = TreeState::Down {
                cut: 0,
                dir: azimuth_bin(push.x, push.y),
            };
            commands.entity(root).insert(Fell {
                tick: clock.as_ref().map_or(0, |clock| clock.0),
            });
        }
    }
}

/// The sim half: the authority's state transitions. The emptying observer is mounted in every
/// composition (a replica's march empties no pool, so it never fires on a client); the ram rule
/// runs only where this process is the authority.
pub(crate) fn sim_plugin(app: &mut App) {
    app.init_resource::<FallClock>()
        .add_observer(fell_on_pool_emptied)
        .configure_sets(
            FixedPostUpdate,
            TreeFallSet.after(PhysicsSystems::Writeback),
        )
        .add_systems(
            FixedPostUpdate,
            (
                fell_on_ram
                    .in_set(TreeFallSet)
                    .run_if(not(resource_exists::<ClientReplica>)),
                derive_state.after(TreeFallSet),
            ),
        )
        .add_systems(
            Update,
            publish_sim_fall_clock.run_if(not(resource_exists::<ClientReplica>)),
        )
        .add_systems(PostUpdate, present_fall.before(TransformSystems::Propagate));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terrain_grid::FIXTURE_EXTENT;

    /// A three-segment fixture trunk (fixture data): 2 m segments, radii 0.3 → 0.2 → 0.1 → 0.05.
    pub(crate) fn fixture_asset() -> TreeAsset {
        TreeAsset {
            species: "fixture".to_owned(),
            segments: vec![
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.3,
                    r_top_m: 0.2,
                },
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.2,
                    r_top_m: 0.1,
                },
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.1,
                    r_top_m: 0.05,
                },
            ],
        }
    }

    fn fixture_table() -> BTreeMap<String, Species> {
        BTreeMap::from([(
            "fixture".to_owned(),
            Species {
                substance: "RHA".to_owned(),
                fell_k: Sourced(1.0e5),
                hp: Sourced(100),
            },
        )])
    }

    pub(crate) fn fixture_kind() -> Arc<TreeKind> {
        Arc::new(
            TreeKind::resolve(
                "fixture",
                fixture_asset(),
                &fixture_table(),
                SubstanceRegistry::shipped_static(),
            )
            .expect("the fixture trunk holds"),
        )
    }

    /// DBH IS READ OFF THE TAPER, not off the base: at 1.3 m into a 2 m segment going 0.3 → 0.2 the
    /// radius is 0.235. A mutant reading the base radius would answer 0.6.
    #[test]
    fn dbh_is_twice_the_taper_radius_at_breast_height() {
        let kind = fixture_kind();
        assert!((kind.dbh_m - 0.47).abs() < 1e-6, "dbh {}", kind.dbh_m);
        assert_eq!(kind.radius_at(-1.0), 0.3, "below the base: the base radius");
        assert_eq!(kind.radius_at(9.0), 0.05, "past the tip: the tip radius");
    }

    /// A taper with a step in it is two trunks, not one: refused at load (A6), never spawned.
    #[test]
    fn a_discontinuous_taper_is_refused() {
        let mut asset = fixture_asset();
        asset.segments[1].r_bottom_m = 0.25;
        let err = TreeKind::resolve(
            "fixture",
            asset,
            &fixture_table(),
            SubstanceRegistry::shipped_static(),
        )
        .expect_err("a step in the taper");
        assert!(err.contains("continuous"), "{err}");
    }

    /// A species whose substance the registry does not carry, or carries as a witness, cannot
    /// be a layer of the walk: refused at load.
    #[test]
    fn an_unknown_or_witness_substance_is_refused() {
        for substance in ["Balsa", "Flesh"] {
            let mut table = fixture_table();
            table.get_mut("fixture").unwrap().substance = substance.to_owned();
            TreeKind::resolve(
                "fixture",
                fixture_asset(),
                &table,
                SubstanceRegistry::shipped_static(),
            )
            .expect_err(substance);
        }
    }

    /// THE BOUNDARY BELONGS TO THE UPPER SEGMENT (A1), at the instance's scale: a scale-2 fixture
    /// has its segment bottoms at 0, 4 and 8 m. A mutant comparing with `>` would put 4.0 in
    /// segment 0; one ignoring the scale would put 5.0 in segment 2.
    #[test]
    fn the_cut_is_the_segment_the_height_lies_in() {
        let body = TreeBody::new(fixture_kind(), 2.0);
        assert_eq!(body.prefix, [0.0, 4.0, 8.0, 12.0]);
        assert!(
            (body.dbh_m - 0.94).abs() < 1e-6,
            "dbh scales with the instance"
        );
        assert_eq!(body.cut_at(-0.5), 0);
        assert_eq!(body.cut_at(3.99), 0);
        assert_eq!(body.cut_at(4.0), 1);
        assert_eq!(body.cut_at(5.0), 1);
        assert_eq!(body.cut_at(8.0), 2);
        assert_eq!(body.cut_at(40.0), 2);
    }

    /// THE TORQUE IS IMPULSE × LEVER, SUMMED ONCE: 50 N·s at 0.5 m and 50 N·s at 2.0 m read
    /// 125 N·s·m (a mutant dropping the lever reads 100), and a contact at the base adds nothing
    /// to the torque while still weighting the push by its impulse.
    #[test]
    fn ram_torque_sums_impulse_times_lever_and_weights_the_push() {
        let contacts = [
            RamContact {
                impulse: 50.0,
                lever_m: 0.5,
                push: Vec2::X,
            },
            RamContact {
                impulse: 50.0,
                lever_m: 2.0,
                push: Vec2::Y,
            },
            RamContact {
                impulse: 100.0,
                lever_m: 0.0,
                push: Vec2::NEG_X,
            },
        ];
        let (torque, push) = ram_torque(&contacts);
        assert_eq!(torque, 125.0);
        assert_eq!(push, Vec2::new(-50.0, 50.0));
        assert_eq!(
            ram_torque(&[]),
            (0.0, Vec2::ZERO),
            "an empty tick adds nothing"
        );
    }

    /// The threshold is the species constant times the CUBE of the breast-height diameter at the
    /// instance's scale: doubling the scale raises it eightfold (a mutant squaring it: fourfold).
    #[test]
    fn the_fell_torque_is_fell_k_times_dbh_cubed() {
        let one = TreeBody::new(fixture_kind(), 1.0);
        let two = TreeBody::new(fixture_kind(), 2.0);
        assert!((one.fell_torque() - 1.0e5 * 0.47f32.powi(3)).abs() < 1.0);
        assert!((two.fell_torque() / one.fell_torque() - 8.0).abs() < 1e-3);
    }

    /// The bins run from `+X` toward `+Z` and wrap once: the four axes land on 0, 64, 128, 192,
    /// and the bin just short of a full turn is 255, not 256.
    #[test]
    fn azimuth_bins_run_from_x_toward_z_and_wrap() {
        assert_eq!(azimuth_bin(1.0, 0.0), 0);
        assert_eq!(azimuth_bin(0.0, 1.0), 64);
        assert_eq!(azimuth_bin(-1.0, 0.0), 128);
        assert_eq!(azimuth_bin(0.0, -1.0), 192);
        assert_eq!(
            azimuth_bin(1.0, -0.01),
            0,
            "a hair short of a turn rounds to 0"
        );
        assert_eq!(azimuth_bin(1.0, -0.02), 255);
        assert_eq!(azimuth_bin(0.0, 0.0), 0);
    }

    /// THE WIRE WORD IS A BIJECTION ONTO THE TREE'S STATES: every state of a tree of `n`
    /// segments survives a round trip, `Standing` is the only word `0`, and the words no state
    /// encodes to are refused, not clamped. A decoder that drops the `+ 1` maps `0x0100` to cut 1
    /// (the round trip fails at cut 0); one that clamps the cut accepts `0x0400` on a 3-segment
    /// tree; one that masks the high byte for `Standing` accepts `0x0040`. The peer-side derive
    /// (ADR-0016) relies on a refused word never reaching a pose.
    #[test]
    fn the_wire_word_round_trips_every_state_and_refuses_the_rest() {
        let segments = 3;
        assert_eq!(TreeState::Standing.encode(), 0);
        assert_eq!(TreeState::decode(0, segments), Ok(TreeState::Standing));
        for cut in 0..segments as u8 {
            for dir in [0u8, 1, 64, 255] {
                let state = TreeState::Down { cut, dir };
                let word = state.encode();
                assert_eq!(word >> 8, u16::from(cut) + 1, "{state:?}");
                assert_eq!(TreeState::decode(word, segments), Ok(state));
            }
        }
        assert_eq!(
            TreeState::Down { cut: 0, dir: 0 }.encode(),
            0x0100,
            "the first down word is the cut-0 word, never 0"
        );
        assert!(
            TreeState::decode(0x0040, segments).is_err(),
            "standing with dir bits"
        );
        assert!(
            TreeState::decode(0x0400, segments).is_err(),
            "cut == segments"
        );
        assert!(
            TreeState::decode(0xFFFF, segments).is_err(),
            "cut 254 past this tree (legal only on a 255-segment one)"
        );
        assert_eq!(
            TreeState::decode(0x0300, segments),
            Ok(TreeState::Down { cut: 2, dir: 0 }),
            "the last segment is a legal cut"
        );
    }

    /// THE DOSE DECIDES THE CUT AND THE DIRECTION: a line dose that empties the pool breaks the
    /// trunk at the segment its entry height lies in and fells it along the round's travel; a
    /// field dose fells it whole, away from the centre; a tree already down keeps its state.
    #[test]
    fn an_emptied_pool_fells_the_tree_at_the_dose() {
        let mut world = World::new();
        world.add_observer(fell_on_pool_emptied);
        world.insert_resource(ShotClock(7));
        let base = Vec3::new(10.0, 3.0, -4.0);
        let spawn = |world: &mut World| {
            world
                .spawn((
                    TreeState::Standing,
                    TreeBody::new(fixture_kind(), 1.0),
                    Transform::from_translation(base),
                ))
                .id()
        };
        let shot = spawn(&mut world);
        world.trigger(PoolEmptied {
            entity: shot,
            at: base + Vec3::new(0.2, 2.5, 0.0),
            along: Some(Vec3::new(0.0, -0.3, 1.0)),
        });
        world.flush();
        assert_eq!(
            world.get::<TreeState>(shot),
            Some(&TreeState::Down { cut: 1, dir: 64 }),
        );
        assert_eq!(
            world.get::<Fell>(shot),
            Some(&Fell { tick: 7 }),
            "the transition carries the tick it happened at"
        );

        let burst = spawn(&mut world);
        world.trigger(PoolEmptied {
            entity: burst,
            at: base + Vec3::new(5.0, 0.5, 0.0),
            along: None,
        });
        assert_eq!(
            world.get::<TreeState>(burst),
            Some(&TreeState::Down { cut: 0, dir: 128 }),
            "a field fells the whole trunk away from its centre",
        );

        world.trigger(PoolEmptied {
            entity: shot,
            at: base + Vec3::new(0.0, 5.0, 0.0),
            along: Some(Vec3::X),
        });
        assert_eq!(
            world.get::<TreeState>(shot),
            Some(&TreeState::Down { cut: 1, dir: 64 }),
            "a tree already down keeps its state",
        );
    }

    /// The spawn builds the body the module doc describes: one root with the pool and the
    /// volume, one segment collider per segment on `[Terrain, Armor]`, nothing on the root's
    /// scale, and no view where no server offers one.
    #[test]
    fn spawn_builds_a_root_and_a_collider_per_segment() {
        let mut world = World::new();
        let kind = fixture_kind();
        let pose = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.7),
            scale: Vec3::splat(1.5),
        };
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        spawn(
            &mut commands,
            &kind,
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 7,
            },
            pose,
            None,
        );
        queue.apply(&mut world);
        let (root, body, health, transform) = world
            .query::<(Entity, &TreeBody, &ComponentHealth, &Transform)>()
            .single(&world)
            .expect("one root");
        assert!(
            (body.dbh_m - 0.705).abs() < 1e-6,
            "the kind's dbh at scale 1.5"
        );
        assert_eq!((health.current, health.max), (100, 100));
        assert_eq!(transform.scale, Vec3::ONE);
        assert_eq!(transform.translation, pose.translation);
        let segments: Vec<(Entity, f32)> = world
            .query_filtered::<(&ChildOf, &Transform), (With<TreeSegment>, With<Collider>)>()
            .iter(&world)
            .map(|(parent, transform)| (parent.parent(), transform.translation.y))
            .collect();
        assert_eq!(segments.len(), 3);
        assert!(segments.iter().all(|(parent, _)| *parent == root));
        let mut bottoms: Vec<f32> = segments.iter().map(|(_, y)| *y).collect();
        bottoms.sort_by(f32::total_cmp);
        assert_eq!(
            bottoms,
            [0.0, 3.0, 6.0],
            "segment bottoms at the scaled prefix sums"
        );
        assert_eq!(world.query::<&TreeView>().iter(&world).count(), 0);
    }

    /// A tilted instance is refused: the cut and the resting pose both read an upright trunk.
    #[test]
    #[should_panic(expected = "is tilted")]
    fn a_tilted_instance_is_refused() {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        spawn(
            &mut commands,
            &fixture_kind(),
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 0,
            },
            // Half a degree: a lean the old loose gate (up.y > 0.9999, about 0.8°) let through.
            Transform::from_rotation(Quat::from_rotation_x(0.5_f32.to_radians())),
            None,
        );
        queue.apply(&mut world);
    }

    /// An 11 × 11 fixture grid over `FIXTURE_EXTENT` (100 m cells) sampled from `height`.
    fn fixture_grid(height: impl Fn(f32, f32) -> f32) -> HeightGrid {
        let size = 11_u32;
        let half = FIXTURE_EXTENT.half_extent();
        let step = FIXTURE_EXTENT.world_size_m / (size - 1) as f32;
        let height = &height;
        let samples: Vec<f32> = (0..size)
            .flat_map(|j| {
                (0..size).map(move |i| height(-half + i as f32 * step, -half + j as f32 * step))
            })
            .collect();
        HeightGrid::new(samples.into(), size, FIXTURE_EXTENT)
    }

    /// THE RESTING TILT IS PINNED TO THE BIT (A9): the fixture trunk on flat ground, on a 10 %
    /// slope and at the grid's edge (where the oracle clamps), at every cut and three bins, must
    /// bisect to these exact f32 bits on every platform — the lying collider is sim state every
    /// peer derives alone, so a peer that differs by one ulp has a different world. Any mutant of
    /// the search reds this: a step fewer or more, the midpoint taken the other way, the sample's
    /// radius offset dropped, platform trig in place of libm's, or the 100° fallback returning
    /// anything but 90°.
    #[test]
    fn the_resting_tilt_is_pinned_to_the_bit() {
        let body = TreeBody::new(fixture_kind(), 1.0);
        let slope = |x: f32, _: f32| x / 10.0;
        let cases = [
            ("flat", fixture_grid(|_, _| 0.0), Vec3::ZERO),
            ("slope", fixture_grid(slope), Vec3::new(100.0, 10.0, 0.0)),
            ("edge", fixture_grid(slope), Vec3::new(499.0, 49.9, 0.0)),
        ];
        let mut rows = Vec::new();
        for (label, grid, base) in &cases {
            for cut in [0_u8, 1, 2] {
                for dir in [0_u8, 64, 200] {
                    let tilt = resting_tilt(&body, *base, cut, dir, |x, z| grid.height_at(x, z));
                    rows.push((*label, cut, dir, tilt.to_bits()));
                }
            }
        }
        const PINNED: [(&str, u8, u8, u32); 27] = [
            ("flat", 0, 0, 0x3fbc3d92),
            ("flat", 0, 64, 0x3fbc3d92),
            ("flat", 0, 200, 0x3fbc3d92),
            ("flat", 1, 0, 0x3fc90fdb),
            ("flat", 1, 64, 0x3fc90fdb),
            ("flat", 1, 200, 0x3fc90fdb),
            ("flat", 2, 0, 0x3fc90fdb),
            ("flat", 2, 64, 0x3fc90fdb),
            ("flat", 2, 200, 0x3fc90fdb),
            ("slope", 0, 0, 0x3faf8c01),
            ("slope", 0, 64, 0x3fbc3d91),
            ("slope", 0, 200, 0x3fb9bf04),
            ("slope", 1, 0, 0x3fc90fdb),
            ("slope", 1, 64, 0x3fc90fdb),
            ("slope", 1, 200, 0x3fc90fdb),
            ("slope", 2, 0, 0x3fc90fdb),
            ("slope", 2, 64, 0x3fc90fdb),
            ("slope", 2, 200, 0x3fc90fdb),
            ("edge", 0, 0, 0x3fb5ca04),
            ("edge", 0, 64, 0x3fbc3d97),
            ("edge", 0, 200, 0x3fb9bf0a),
            ("edge", 1, 0, 0x3fc90fdb),
            ("edge", 1, 64, 0x3fc90fdb),
            ("edge", 1, 200, 0x3fc90fdb),
            ("edge", 2, 0, 0x3fc90fdb),
            ("edge", 2, 64, 0x3fc90fdb),
            ("edge", 2, 200, 0x3fc90fdb),
        ];
        let listing = rows
            .iter()
            .map(|(label, cut, dir, bits)| {
                format!("            ({label:?}, {cut}, {dir}, {bits:#010x}),")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            rows, PINNED,
            "the resting tilt moved; the rows now read:\n{listing}"
        );
        // Flat ground, whole trunk: it lies just short of the horizontal, the tip's radius holding
        // it off the surface. A piece cut above the base pivots at the cut's height and reaches
        // nothing by 100°, so it takes the 90° fallback.
        for row in rows.iter().filter(|row| row.0 == "flat") {
            let tilt = f32::from_bits(row.3);
            if row.1 == 0 {
                assert!(tilt > 80.0_f32.to_radians() && tilt < FRAC_PI_2, "{row:?}");
            } else {
                assert_eq!(tilt, FRAC_PI_2, "{row:?}");
            }
        }
    }

    /// EVERY PEER DERIVES THE FALLEN BODY FROM THE STATE (ADR-0016, A2): once a tree is `Down`,
    /// its stump segments keep their place and their `[Terrain, Armor]` layers, the lying piece's
    /// segments leave the vertical onto `[Armor]` with no filters, the resting pose is recorded and
    /// the pool is gone. A derive that forgets the layer swap, moves the stump, or leaves the pool
    /// reds this.
    #[test]
    fn a_down_state_derives_the_lying_piece_and_drops_the_pool() {
        let mut app = App::new();
        app.insert_resource(fixture_grid(|_, _| 0.0));
        app.add_systems(Update, derive_state);
        let kind = fixture_kind();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, app.world());
        spawn(
            &mut commands,
            &kind,
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 0,
            },
            Transform::from_xyz(5.0, 0.0, 5.0),
            None,
        );
        queue.apply(app.world_mut());
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<TreeBody>>()
            .single(app.world())
            .expect("one root");
        app.update();
        assert!(
            app.world().get::<ComponentHealth>(root).is_some(),
            "standing: the pool stays"
        );
        *app.world_mut().get_mut::<TreeState>(root).unwrap() = TreeState::Down { cut: 1, dir: 0 };
        app.update();
        assert!(
            app.world().get::<ComponentHealth>(root).is_none(),
            "down: no pool"
        );
        let resting = app
            .world()
            .get::<Resting>(root)
            .expect("the resting pose is recorded");
        assert!(resting.tilt > 1.0 && (resting.dx, resting.dz) == (1.0, 0.0));
        let mut segments: Vec<(Transform, CollisionLayers)> = app
            .world_mut()
            .query_filtered::<(&Transform, &CollisionLayers), With<TreeSegment>>()
            .iter(app.world())
            .map(|(transform, layers)| (*transform, *layers))
            .collect();
        segments.sort_by(|a, b| a.0.translation.y.total_cmp(&b.0.translation.y));
        let standing = CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL);
        let lying = CollisionLayers::new([Layer::Armor], LayerMask::NONE);
        assert_eq!(
            segments[0].0,
            Transform::from_xyz(0.0, 0.0, 0.0),
            "the stump stays"
        );
        assert_eq!(segments[0].1, standing);
        assert_eq!(
            segments[1].0.translation,
            Vec3::new(0.0, 2.0, 0.0),
            "the pivot segment turns in place"
        );
        assert_eq!(segments[1].1, lying);
        let top = segments[2].0.translation;
        assert!(
            top.x > 1.9 && top.y < 2.2 && top.z.abs() < 1e-5,
            "the top segment lies toward +x: {top}"
        );
        assert_eq!(segments[2].1, lying);
        assert!(
            (segments[2].0.rotation * Vec3::Y).x > 0.99,
            "the lying segment's axis points along the fall"
        );
    }
}
