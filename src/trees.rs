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
//! `fell_k × dbh³` — and every peer derives the rest from it: the body here, the glb and the
//! fall's presentation in [`view`], which the sim never waits on.
//!
//! THE BODY IN THE PHYSICS WORLD. The body is derived and placed in avian's world — collider
//! poses, bounds and the trees a spatial query walks — in the frame its state changes
//! ([`derive_systems`]), so the first march after that frame, the next fixed tick's, reads the
//! derived body. The authority derives after the tick's physics step: a transition a march makes
//! is read by the march of the next tick, never by later shells of the same tick.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::{FRAC_PI_2, PI};
use std::path::Path;
use std::sync::Arc;

use avian3d::collider_tree::{ColliderTrees, update_moved_collider_aabbs};
use avian3d::prelude::{
    Collider, ColliderTransform, CollisionLayers, ContactGraph, ContactPair, LayerMask, Position,
    RigidBody, Rotation,
};
use avian3d::schedule::PhysicsSystems;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

use crate::Layer;
use crate::ballistics::{BallisticVolume, ComponentHealth, PoolEmptied, VolumeMaterial};
use crate::provenance::Sourced;
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;
use crate::tank::Tank;
use crate::terrain_grid::HeightGrid;
use crate::{ClientReplica, ShotClock};

pub(crate) mod view;
pub(crate) use view::FallClock;

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

/// The authority tick a tree's state last changed at (`ShotClock` units, the wire's `tick`).
/// Inserted beside the `Down` state by whoever sets it — the authority's laws here, the ledger
/// apply on a replica — so the presentation fuses the fall to the tick it happened at; removed
/// by [`derive_state`] when the state stands, as every derived marker is.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fell {
    pub(crate) tick: u32,
}

/// The set the authority's fall laws run in; the ledger publish and the body derive order after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TreeFallSet;

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

/// The collision filter of a standing segment: hulls contact it and rounds walk it.
fn standing_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL)
}

/// The collision filter of a lying segment: a hard body for the walk, no longer an obstacle for
/// a hull (A2).
fn lying_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::Armor], LayerMask::NONE)
}

/// Every peer derives the whole body from the state on every change of it (ADR-0016, A10),
/// never from the body before: standing, every segment stands at its spawn height on
/// [`standing_layers`] with the pool present and no resting pose or transition tick; down, the
/// stump's segments
/// stand the same and every segment from the cut up lies in the resting pose on
/// [`lying_layers`], the pose is recorded and the pool is gone — what is down takes no more
/// dose. A tree back to standing (a replaced ledger) or down at another cut derives exactly as
/// one that never held the old state.
///
/// The fall turns the piece rigidly about the pivot — the cut's bottom — so each lying segment
/// keeps the instance's yaw under the tilt: its world rotation is `tilt × yaw`, its world offset
/// from the base the pivot plus its height above the cut turned by the tilt alone (the pivot and
/// the trunk's axis are vertical, which the yaw fixes).
fn derive_state(
    trees: Query<
        (
            Entity,
            &TreeState,
            &TreeBody,
            &Transform,
            &Children,
            Has<ComponentHealth>,
        ),
        (Changed<TreeState>, Without<TreeSegment>),
    >,
    mut segments: Query<(&mut Transform, &CollisionLayers), With<TreeSegment>>,
    grid: Option<Res<HeightGrid>>,
    mut commands: Commands,
) {
    // Trees are placed by the map whose grid this is; a composition without the grid (the flat
    // slab) has no trees to derive.
    let Some(grid) = grid else {
        return;
    };
    for (root, state, body, transform, children, has_pool) in &trees {
        let fallen = match *state {
            TreeState::Standing => None,
            TreeState::Down { cut, dir } => {
                let tilt = resting_tilt(body, transform.translation, cut, dir, |x, z| {
                    grid.height_at(x, z)
                });
                let (dx, dz) = azimuth(dir);
                Some((usize::from(cut), Resting { tilt, dx, dz }))
            }
        };
        let root_inv = transform.rotation.inverse();
        let mut k = 0;
        for &child in children {
            let Ok((mut local, layers)) = segments.get_mut(child) else {
                continue;
            };
            let (pose, filter) = match fallen {
                Some((cut, resting)) if k >= cut => {
                    let tilt = tilt_rotation(&resting);
                    let pivot = Vec3::Y * body.prefix[cut];
                    let along = tilt * (Vec3::Y * (body.prefix[k] - body.prefix[cut]));
                    let pose = Transform {
                        translation: root_inv * (pivot + along),
                        rotation: root_inv * tilt * transform.rotation,
                        scale: Vec3::ONE,
                    };
                    (pose, lying_layers())
                }
                _ => (
                    Transform::from_xyz(0.0, body.prefix[k], 0.0),
                    standing_layers(),
                ),
            };
            local.set_if_neq(pose);
            // An immutable component: replaced only when the state moves it.
            if *layers != filter {
                commands.entity(child).insert(filter);
            }
            k += 1;
        }
        match fallen {
            Some((_, resting)) => {
                commands
                    .entity(root)
                    .insert(resting)
                    .remove::<ComponentHealth>();
            }
            None => {
                commands.entity(root).remove::<(Resting, Fell)>();
                if !has_pool {
                    commands.entity(root).insert(ComponentHealth {
                        current: body.kind.hp,
                        max: body.kind.hp,
                    });
                }
            }
        }
    }
}

/// Every peer places a re-derived tree's segment colliders in the physics world in the frame the
/// state changes: each segment's `ColliderTransform`, `Position` and `Rotation` take the values
/// avian's propagation (`propagate_collider_transforms`) and child update
/// (`update_child_collider_position`) compute from the derived transforms — the same expressions
/// on the same inputs, so the next physics step rewrites the same bits. The `ColliderTransform`
/// is written with them because lightyear rewrites every child collider's pose from it after
/// each fixed loop (`LightyearAvianPlugin::update_child_collider_position`).
fn place_segment_colliders(
    trees: Query<
        (&Transform, &Position, &Rotation, &Children),
        (Changed<TreeState>, Without<TreeSegment>),
    >,
    mut segments: Query<
        (
            &Transform,
            &mut ColliderTransform,
            &mut Position,
            &mut Rotation,
        ),
        With<TreeSegment>,
    >,
) {
    for (root, body_position, body_rotation, children) in &trees {
        let parent = ColliderTransform::from(*root);
        for &child in children {
            let Ok((local, mut collider, mut position, mut rotation)) = segments.get_mut(child)
            else {
                continue;
            };
            let local = ColliderTransform::from(*local);
            let placed = ColliderTransform {
                translation: parent.scale * local.translation,
                rotation: local.rotation,
                scale: parent.scale * local.scale,
            };
            collider.set_if_neq(placed);
            position.set_if_neq(Position(
                body_position.0 + body_rotation * placed.translation,
            ));
            rotation.set_if_neq((body_rotation.0 * placed.rotation.0).normalize().into());
        }
    }
}

/// The body derive as every peer schedules it: [`derive_state`], then
/// [`place_segment_colliders`], then avian's own refit of moved colliders
/// ([`update_moved_collider_aabbs`]: their bounds and the collider trees a spatial query walks)
/// in any frame a tree's state changed. A march runs in `FixedUpdate`, before the physics step,
/// so the march of the first fixed tick after the derive reads the derived body; without the
/// placement it would read the pose of the last step, one tick behind the state.
pub(crate) fn derive_systems() -> ScheduleConfigs<ScheduleSystem> {
    (
        derive_state,
        place_segment_colliders,
        update_moved_collider_aabbs::<Collider>
            .run_if(resource_exists::<ColliderTrees>)
            .run_if(any_match_filter::<(Changed<TreeState>, Without<TreeSegment>)>),
    )
        .chain()
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
            standing_layers(),
            TreeSegment,
        ));
    }
    if let Some(assets) = view {
        view::attach(commands, root, kind, scale, assets);
    }
}

/// A standing tree whose pool a dose emptied falls: at the dose's entry height (the segment that
/// height lies in) and along the round's travel; a field (no direction) fells it whole, away from
/// the field's centre. A tree already down ignores it.
pub(crate) fn fell_on_pool_emptied(
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

/// The ram law's reading of one tick's contacts: for every ACTIVE touching pair between a trunk
/// segment and a tank's body, the solver's normal impulse (N·s) at its lever above the tree's base
/// and its horizontal push into the tree, grouped by standing tree. Sleeping pairs are never read
/// — their retained impulse is not this tick's — and only a tank pushes. A pair names its
/// colliders' BODIES, and a tank is one body: its root, marked [`Tank`], under which the hull and
/// turret nodes hang with their proxies (`Hull` marks a node, never a body). A body that is no
/// tank — a launched turret — rolls off a tree. The manifold normal runs from the pair's first
/// collider to its second, so it points into the tree exactly when the tree's segment is the
/// second.
fn ram_contacts<'a>(
    pairs: impl Iterator<Item = &'a ContactPair>,
    segment_root: impl Fn(Entity) -> Option<Entity>,
    is_tank: impl Fn(Entity) -> bool,
    standing_base_y: impl Fn(Entity) -> Option<f32>,
) -> BTreeMap<Entity, Vec<RamContact>> {
    let mut per_tree: BTreeMap<Entity, Vec<RamContact>> = BTreeMap::new();
    for pair in pairs {
        let (tree_first, root, other) =
            match (segment_root(pair.collider1), segment_root(pair.collider2)) {
                (Some(root), None) => (true, root, pair.body2),
                (None, Some(root)) => (false, root, pair.body1),
                _ => continue,
            };
        if !other.is_some_and(&is_tank) {
            continue;
        }
        let Some(base_y) = standing_base_y(root) else {
            continue;
        };
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
    per_tree
}

/// The authority's ram rule, read after the solver stored this tick's impulses: a standing tree
/// whose trunk took its [`TreeBody::fell_torque`] of a tank's torque in one tick falls whole,
/// along the push ([`ram_contacts`] reads the contacts, [`ram_torque`] sums them).
fn fell_on_ram(
    graph: Res<ContactGraph>,
    segments: Query<&ChildOf, With<TreeSegment>>,
    tanks: Query<(), With<Tank>>,
    mut trees: Query<(&mut TreeState, &TreeBody, &Transform)>,
    clock: Option<Res<ShotClock>>,
    mut commands: Commands,
) {
    let per_tree = ram_contacts(
        graph.iter_active_touching(),
        |collider| segments.get(collider).ok().map(ChildOf::parent),
        |body| tanks.contains(body),
        |root| {
            trees
                .get(root)
                .ok()
                .filter(|(state, _, _)| **state == TreeState::Standing)
                .map(|(_, _, transform)| transform.translation.y)
        },
    );
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

/// The sim half: the authority's state transitions and the body derived from them. The emptying
/// observer is mounted in every composition (a replica's march empties no pool, so it never
/// fires on a client); the ram rule and the derive after it run only where this process is the
/// authority — a replica derives in the frame its ledger lands (`net::destruction`).
pub(crate) fn sim_plugin(app: &mut App) {
    app.add_observer(fell_on_pool_emptied)
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
                derive_systems()
                    .after(TreeFallSet)
                    .run_if(not(resource_exists::<ClientReplica>)),
            ),
        );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terrain_grid::FIXTURE_EXTENT;
    use avian3d::prelude::{ColliderAabb, SimpleCollider};

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

    /// The fixture trunk as a kind of `substance` with an HP pool of `hp` — fixture data for the
    /// march's tests, which need a trunk a round goes through and a pool a field alone empties.
    pub(crate) fn fixture_kind_of(substance: &str, hp: u32) -> Arc<TreeKind> {
        let table = BTreeMap::from([(
            "fixture".to_owned(),
            Species {
                substance: substance.to_owned(),
                fell_k: Sourced(1.0e5),
                hp: Sourced(hp),
            },
        )]);
        Arc::new(
            TreeKind::resolve(
                "fixture",
                fixture_asset(),
                &table,
                SubstanceRegistry::shipped_static(),
            )
            .expect("the fixture trunk holds"),
        )
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

    /// EVERY A6 REFUSAL IS A REFUSAL, NOT A CLAMP: an empty trunk, a non-finite or non-positive
    /// dimension, an unknown species, a non-positive pool or ram threshold, a trunk too short to
    /// read a breast height, and more segments than the wire word's cut byte can name are each
    /// refused by name. Any one of these laxed to a default — a NaN height carried, a zero HP
    /// pool spawned, a 256th segment accepted — reds its row.
    #[test]
    fn every_malformed_asset_or_species_is_refused_by_name() {
        let refusal = |asset: TreeAsset, table: BTreeMap<String, Species>| {
            TreeKind::resolve(
                "fixture",
                asset,
                &table,
                SubstanceRegistry::shipped_static(),
            )
            .expect_err("refused")
        };
        let mut asset = fixture_asset();
        asset.segments.clear();
        assert!(refusal(asset, fixture_table()).contains("segments"));
        let mut asset = fixture_asset();
        asset.segments[0].height_m = f32::NAN;
        assert!(refusal(asset, fixture_table()).contains("dimension"));
        let mut asset = fixture_asset();
        asset.segments[2].r_top_m = 0.0;
        assert!(refusal(asset, fixture_table()).contains("dimension"));
        let mut asset = fixture_asset();
        asset.species = "no_such_species".to_owned();
        assert!(refusal(asset, fixture_table()).contains("species"));
        let mut table = fixture_table();
        table.get_mut("fixture").unwrap().hp = Sourced(0);
        assert!(refusal(fixture_asset(), table).contains("hp"));
        let mut table = fixture_table();
        table.get_mut("fixture").unwrap().fell_k = Sourced(-1.0);
        assert!(refusal(fixture_asset(), table).contains("fell_k"));
        let mut asset = fixture_asset();
        for segment in &mut asset.segments {
            segment.height_m = 0.4;
        }
        assert!(refusal(asset, fixture_table()).contains("breast"));
        let mut asset = fixture_asset();
        let stub = asset.segments[2];
        asset.segments = vec![stub; MAX_SEGMENTS + 1];
        assert!(refusal(asset, fixture_table()).contains("segments"));
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

    /// A cut the word's high byte cannot carry (`cut + 1` past a byte) has no encoding: the
    /// authority never sets one — resolve bounds the segment count — so encoding it is a broken
    /// invariant, not a wrapped word.
    #[test]
    #[should_panic(expected = "has no word")]
    fn a_cut_past_the_word_has_no_encoding() {
        let _ = TreeState::Down { cut: 255, dir: 0 }.encode();
    }

    /// THE RAM LAW READS A TANK BODY'S CONTACTS ON A STANDING TREE AND NOTHING ELSE: a pair whose
    /// other body is no tank (a launched turret), a pair with no trunk segment, and any pair on
    /// a tree no longer standing contribute nothing; a tank's point reads its impulse at its
    /// height above the base, pushing along the manifold normal into the tree. Dropping the tank
    /// check, the standing check or the normal's flip for a tree-first pair reds this.
    #[test]
    fn ram_contacts_read_tank_body_pairs_on_standing_trees_only() {
        use avian3d::collision::contact_types::ContactId;
        use avian3d::prelude::{ContactManifold, ContactPoint};
        let mut world = World::new();
        let mut entity = || world.spawn_empty().id();
        let (tree, segment) = (entity(), entity());
        let (tank, tank_collider) = (entity(), entity());
        let (turret, turret_collider) = (entity(), entity());
        let (other, other_collider) = (entity(), entity());
        let pair = |c1, c2, b1, b2| {
            let mut point =
                ContactPoint::new(Vec3::ZERO, Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0), 0.0);
            point.normal_impulse = 10.0;
            let mut pair = ContactPair::new(c1, c2, ContactId(0));
            pair.body1 = Some(b1);
            pair.body2 = Some(b2);
            pair.manifolds = vec![ContactManifold::new([point], Vec3::X)];
            pair
        };
        let pairs = [
            pair(segment, tank_collider, tree, tank),
            pair(turret_collider, segment, turret, tree),
            pair(tank_collider, other_collider, tank, other),
        ];
        let segment_root = |collider| (collider == segment).then_some(tree);
        let is_tank = |body| body == tank;
        let read = ram_contacts(pairs.iter(), segment_root, is_tank, |root| {
            (root == tree).then_some(1.0)
        });
        assert_eq!(read.len(), 1, "one tree pushed: {read:?}");
        assert_eq!(
            read[&tree],
            vec![RamContact {
                impulse: 10.0,
                lever_m: 2.0,
                push: Vec2::new(-1.0, 0.0),
            }],
            "the tank's point, 2 m above the base, pushing against the normal (tree first)",
        );
        let down = ram_contacts(pairs.iter(), segment_root, is_tank, |_| None);
        assert!(down.is_empty(), "a tree not standing takes no push");
    }

    /// THE RAM READS THE TANK'S ROOT BODY, THE WAY AVIAN NAMES IT: a tank is one dynamic body —
    /// its root, carrying [`Tank`] — whose `Hull` node and that node's collision proxy are
    /// children with no body of their own, so a contact pair names the root and never the `Hull`
    /// node. In a physics world with the real schedule, a proxy driven into a standing trunk
    /// fells it along the push; a launched turret — a dynamic body of its own, no tank — driven
    /// the same way does not. A law reading `Hull` off the pair's body fells neither.
    /// DERIVED: 20 t stopped from 4 m/s is 80 kN·s; at the cube's half-metre of lever or more
    /// that is four times the fixture's 10.4 kN·s·m threshold.
    #[test]
    fn a_tanks_proxies_ram_through_the_root_body_and_a_launched_turret_rolls_off() {
        use crate::tank::Hull;
        use avian3d::prelude::{Gravity, LinearVelocity, Mass, PhysicsPlugins};
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity::ZERO)
        // One fixed tick per update at the fixed clock's default 64 Hz.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let kind = fixture_kind();
        let tree_at = |app: &mut App, z: f32, ordinal: u16| -> Entity {
            let world = app.world_mut();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            spawn(
                &mut commands,
                &kind,
                MapInstance {
                    id: format!("fir_{ordinal}"),
                    ordinal,
                },
                Transform::from_xyz(0.0, 0.0, z),
                None,
            );
            queue.apply(world);
            world
                .query::<(Entity, &MapInstance)>()
                .iter(world)
                .find(|(_, identity)| identity.ordinal == ordinal)
                .map(|(entity, _)| entity)
                .expect("the tree just spawned")
        };
        let rammed = tree_at(&mut app, 0.0, 0);
        let nudged = tree_at(&mut app, 20.0, 1);
        let body = |app: &mut App, z: f32| -> Entity {
            app.world_mut()
                .spawn((
                    Transform::from_xyz(-1.2, 1.0, z),
                    RigidBody::Dynamic,
                    Mass(20_000.0),
                    LinearVelocity(Vec3::X * 4.0),
                ))
                .id()
        };
        let proxy = (
            Collider::cuboid(1.0, 1.0, 1.0),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
        );
        // The tank: root body → `Hull` node → proxy, as `tank::spawn` builds it.
        let tank = body(&mut app, 0.0);
        app.world_mut().entity_mut(tank).insert(Tank);
        let hull = app
            .world_mut()
            .spawn((ChildOf(tank), Transform::IDENTITY, Hull))
            .id();
        app.world_mut()
            .spawn((ChildOf(hull), Transform::IDENTITY, proxy.clone()));
        // The launched turret: a free body carrying its collider, no tank above it.
        let turret = body(&mut app, 20.0);
        app.world_mut().entity_mut(turret).insert(proxy);
        for _ in 0..20 {
            app.update();
        }
        assert_eq!(
            app.world().get::<TreeState>(rammed),
            Some(&TreeState::Down { cut: 0, dir: 0 }),
            "the tank's proxy fells the trunk along +x"
        );
        assert!(
            app.world().get::<Fell>(rammed).is_some(),
            "the transition carries its tick"
        );
        assert_eq!(
            app.world().get::<TreeState>(nudged),
            Some(&TreeState::Standing),
            "a launched turret rolls off"
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
        assert_eq!(
            transform.rotation, pose.rotation,
            "the yaw is the instance's"
        );
        let standing = CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL);
        let mut segments: Vec<(Entity, f32, ColliderAabb)> = world
            .query_filtered::<
                (&ChildOf, &Transform, &Collider, &CollisionLayers),
                With<TreeSegment>,
            >()
            .iter(&world)
            .map(|(parent, transform, collider, layers)| {
                assert_eq!(*layers, standing, "a standing segment stops hulls and rounds");
                (
                    parent.parent(),
                    transform.translation.y,
                    collider.aabb(Vec3::ZERO, Quat::IDENTITY),
                )
            })
            .collect();
        assert_eq!(segments.len(), 3);
        assert!(segments.iter().all(|(parent, _, _)| *parent == root));
        segments.sort_by(|a, b| a.1.total_cmp(&b.1));
        let bottoms: Vec<f32> = segments.iter().map(|(_, y, _)| *y).collect();
        assert_eq!(
            bottoms,
            [0.0, 3.0, 6.0],
            "segment bottoms at the scaled prefix sums"
        );
        // Each hull spans its frustum's wider ring and its scaled height from the segment's own
        // origin: the ring table holds the four axis points, so the bounds are exact.
        for ((_, _, aabb), segment) in segments.iter().zip(&kind.segments) {
            let r = segment.r_bottom_m.max(segment.r_top_m) * 1.5;
            let h = segment.height_m * 1.5;
            assert!(
                aabb.min.abs_diff_eq(Vec3::new(-r, 0.0, -r), 1e-6)
                    && aabb.max.abs_diff_eq(Vec3::new(r, h, r), 1e-6),
                "the hull is the scaled frustum: {aabb:?} for {segment:?}"
            );
        }
        assert_eq!(world.query::<&view::TreeView>().iter(&world).count(), 0);
    }

    /// A non-uniform scale is refused: one scalar multiplies every height and radius, so an
    /// instance that stretches one axis has no body this module can build.
    #[test]
    #[should_panic(expected = "scales uniformly")]
    fn a_non_uniform_scale_is_refused() {
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
            Transform::from_scale(Vec3::new(1.0, 1.5, 1.0)),
            None,
        );
        queue.apply(&mut world);
    }

    /// A negative scale is refused: it is uniform, but no trunk is built inside out.
    #[test]
    #[should_panic(expected = "scales uniformly")]
    fn a_negative_scale_is_refused() {
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
            Transform::from_scale(Vec3::splat(-1.0)),
            None,
        );
        queue.apply(&mut world);
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
            // Half a degree of lean: its up falls 3.8e-5 short of 1 (DERIVED: 1 − cos 0.5°),
            // 38 times `UPRIGHT_EPSILON`.
            Transform::from_rotation(Quat::from_rotation_x(0.5_f32.to_radians())),
            None,
        );
        queue.apply(&mut world);
    }

    /// An 11 × 11 fixture grid over `FIXTURE_EXTENT` (100 m cells) sampled from `height`.
    pub(crate) fn fixture_grid(height: impl Fn(f32, f32) -> f32) -> HeightGrid {
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
    /// peer derives alone, so a peer that differs by one ulp has a different world. The nine
    /// cut-0 rows reach the terrain and pin the search itself: a step fewer or more, the midpoint
    /// taken the other way, the sample's radius offset dropped or platform trig in place of
    /// libm's reds them. The eighteen rows above the base reach nothing by 100° and pin only the
    /// fallback's 90°; a reordering of the samples inside `any` is not observable here.
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
        // Flat ground, whole trunk: it lies just short of the horizontal, held off the surface by
        // the first boundary above the cut — the widest sample, 0.2 m at 2 m, touching at 84.3°
        // (DERIVED: cos tilt = r / h) before the narrower ones. A piece cut above the base pivots
        // at the cut's height and reaches nothing by 100°, so it takes the 90° fallback.
        for row in rows.iter().filter(|row| row.0 == "flat") {
            let tilt = f32::from_bits(row.3);
            if row.1 == 0 {
                assert!(tilt > 80.0_f32.to_radians() && tilt < FRAC_PI_2, "{row:?}");
            } else {
                assert_eq!(tilt, FRAC_PI_2, "{row:?}");
            }
        }
    }

    /// A derive bed: the fixture tree spawned at `pose` on `grid`, the derive in `Update`.
    fn derive_bed(grid: HeightGrid, pose: Transform) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(grid);
        app.add_systems(Update, derive_state);
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, app.world());
        spawn(
            &mut commands,
            &fixture_kind(),
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 0,
            },
            pose,
            None,
        );
        queue.apply(app.world_mut());
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<TreeBody>>()
            .single(app.world())
            .expect("one root");
        (app, root)
    }

    /// The segments' local transforms and filters, in segment order.
    fn segments_of(app: &App, root: Entity) -> Vec<(Transform, CollisionLayers)> {
        let world = app.world();
        world
            .get::<Children>(root)
            .expect("segments")
            .iter()
            .filter_map(|child| {
                Some((
                    *world.get::<Transform>(child)?,
                    *world.get::<CollisionLayers>(child)?,
                ))
            })
            .collect()
    }

    /// EVERY PEER DERIVES THE WHOLE BODY FROM THE STATE, IN EVERY ORDER (ADR-0016, A2, A10):
    /// down, the stump's segments keep their place and their standing filter, every segment from
    /// the cut up leaves the vertical onto the lying filter, the resting pose is recorded, the
    /// setter's transition tick stays and the pool is gone; back to standing, every segment
    /// stands at its spawn height on the standing filter, the pose and the transition tick are
    /// gone and the pool is back; down at another cut, the segments the new cut spares stand
    /// again and the rest lie. A derive that skips `Standing`, touches only the segments at and
    /// above the new cut, or leaves the pool, the pose or the tick from the state before reds
    /// this.
    #[test]
    fn every_state_derives_the_whole_body_from_the_state() {
        let (mut app, root) =
            derive_bed(fixture_grid(|_, _| 0.0), Transform::from_xyz(5.0, 0.0, 5.0));
        let spawned = segments_of(&app, root);
        assert_eq!(spawned.len(), 3);
        app.update();
        assert!(
            app.world().get::<ComponentHealth>(root).is_some()
                && app.world().get::<Resting>(root).is_none(),
            "standing at spawn: the pool stays, no pose"
        );
        assert_eq!(
            segments_of(&app, root),
            spawned,
            "standing derives the spawn"
        );

        // As every setter does: a `Down` state lands with its transition tick.
        let set = |app: &mut App, state: TreeState| {
            *app.world_mut().get_mut::<TreeState>(root).unwrap() = state;
            if let TreeState::Down { .. } = state {
                app.world_mut().entity_mut(root).insert(Fell { tick: 7 });
            }
            app.update();
        };
        set(&mut app, TreeState::Down { cut: 1, dir: 0 });
        assert!(
            app.world().get::<ComponentHealth>(root).is_none(),
            "down: no pool"
        );
        assert_eq!(
            app.world().get::<Fell>(root),
            Some(&Fell { tick: 7 }),
            "down: the transition tick stays"
        );
        let resting = app
            .world()
            .get::<Resting>(root)
            .expect("the resting pose is recorded");
        assert!(resting.tilt > 1.0 && (resting.dx, resting.dz) == (1.0, 0.0));
        let down_1 = segments_of(&app, root);
        assert_eq!(down_1[0], spawned[0], "the stump stays");
        assert_eq!(
            down_1[1].0.translation,
            Vec3::new(0.0, 2.0, 0.0),
            "the pivot segment turns in place"
        );
        assert_eq!(down_1[1].1, lying_layers());
        let top = down_1[2].0.translation;
        assert!(
            top.x > 1.9 && top.y < 2.2 && top.z.abs() < 1e-5,
            "the top segment lies toward +x: {top}"
        );
        assert_eq!(down_1[2].1, lying_layers());
        assert!(
            (down_1[2].0.rotation * Vec3::Y).x > 0.99,
            "the lying segment's axis points along the fall"
        );

        set(&mut app, TreeState::Standing);
        assert_eq!(
            segments_of(&app, root),
            spawned,
            "back to standing: every segment at its spawn pose and filter"
        );
        let pool = app
            .world()
            .get::<ComponentHealth>(root)
            .expect("back to standing: the pool is back");
        assert_eq!((pool.current, pool.max), (100, 100), "full");
        assert!(
            app.world().get::<Resting>(root).is_none() && app.world().get::<Fell>(root).is_none(),
            "no pose and no transition tick standing"
        );

        set(&mut app, TreeState::Down { cut: 2, dir: 64 });
        let down_2 = segments_of(&app, root);
        assert_eq!(&down_2[..2], &spawned[..2], "the two spared segments stand");
        let tip = down_2[2].0.translation;
        assert!(
            tip.z > 0.0 || (down_2[2].0.rotation * Vec3::Y).z > 0.99,
            "the tip lies toward +z: {:?}",
            down_2[2].0
        );
        assert_eq!(down_2[2].1, lying_layers());

        set(&mut app, TreeState::Down { cut: 0, dir: 128 });
        let down_0 = segments_of(&app, root);
        assert!(
            down_0.iter().all(|(_, layers)| *layers == lying_layers()),
            "cut 0: every segment lies"
        );
        assert!(
            down_0
                .iter()
                .all(|(local, _)| (local.rotation * Vec3::Y).x < -0.99),
            "cut 0: every segment's axis points toward -x: {down_0:?}"
        );
        assert_ne!(
            down_0[1], down_2[1],
            "the once-spared segment no longer stands"
        );
    }

    /// THE LYING PIECE KEEPS THE INSTANCE'S YAW (A1, P2d): the fall turns the stack rigidly, so a
    /// lying segment's world rotation is the tilt composed over the yaw — its ring's own axes
    /// turned by the yaw, then laid down — and its world offset from the base is the pivot plus
    /// its height above the cut, tilted, which the yaw leaves alone. A derive composing the tilt
    /// alone lays every ring down unturned, which the 12-gon makes observable.
    #[test]
    fn the_lying_piece_keeps_the_instances_yaw() {
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        let base = Vec3::new(5.0, 0.0, 5.0);
        let (mut app, root) = derive_bed(
            fixture_grid(|_, _| 0.0),
            Transform {
                translation: base,
                rotation: yaw,
                scale: Vec3::ONE,
            },
        );
        *app.world_mut().get_mut::<TreeState>(root).unwrap() = TreeState::Down { cut: 1, dir: 0 };
        app.update();
        let resting = *app.world().get::<Resting>(root).expect("the pose");
        let tilt = tilt_rotation(&resting);
        let body = app.world().get::<TreeBody>(root).expect("the body").clone();
        for (k, (local, _)) in segments_of(&app, root).into_iter().enumerate().skip(1) {
            let world_rotation = yaw * local.rotation;
            let world_offset = yaw * local.translation;
            let h = body.prefix[k] - body.prefix[1];
            let expected_offset = Vec3::Y * body.prefix[1] + tilt * (Vec3::Y * h);
            assert!(
                world_offset.abs_diff_eq(expected_offset, 1e-5),
                "segment {k}: offset {world_offset} vs {expected_offset}"
            );
            let expected_x = tilt * (yaw * Vec3::X);
            assert!(
                (world_rotation * Vec3::X).abs_diff_eq(expected_x, 1e-5),
                "segment {k}: ring axis {} vs {expected_x}",
                world_rotation * Vec3::X
            );
        }
    }

    /// THE LYING POSE IS PINNED TO THE BIT (A9): for every row of the tilt pins that touches the
    /// terrain — the whole trunk, cut 0, on flat ground, on the slope and at the grid's edge, in
    /// three bins — a yawed instance's three lying segments derive to these exact local transform
    /// bits (translation and rotation, folded in segment order), on every platform: glam runs
    /// scalar-math and the trig is libm's, so a peer that differs by one ulp has a different
    /// world. Any change to the composition of the pose — the yaw dropped, the pivot moved, a
    /// factor reordered — reds this; a tilt change reds the tilt pins first.
    #[test]
    fn the_lying_pose_is_pinned_to_the_bit() {
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        let slope = |x: f32, _: f32| x / 10.0;
        let cases: [(&str, fn(f32, f32) -> f32, Vec3); 3] = [
            ("flat", |_, _| 0.0, Vec3::ZERO),
            ("slope", slope, Vec3::new(100.0, 10.0, 0.0)),
            ("edge", slope, Vec3::new(499.0, 49.9, 0.0)),
        ];
        let fold =
            |acc: u64, bits: u32| (acc ^ u64::from(bits)).wrapping_mul(0x0000_0100_0000_01b3);
        let mut rows = Vec::new();
        for (label, height, base) in cases {
            for dir in [0_u8, 64, 200] {
                let (mut app, root) = derive_bed(
                    fixture_grid(height),
                    Transform {
                        translation: base,
                        rotation: yaw,
                        scale: Vec3::ONE,
                    },
                );
                *app.world_mut().get_mut::<TreeState>(root).unwrap() =
                    TreeState::Down { cut: 0, dir };
                app.update();
                let segments = segments_of(&app, root);
                assert_eq!(segments.len(), 3);
                let mut digest = 0xcbf2_9ce4_8422_2325_u64;
                for (local, _) in &segments {
                    for value in local
                        .translation
                        .to_array()
                        .into_iter()
                        .chain(local.rotation.to_array())
                    {
                        digest = fold(digest, value.to_bits());
                    }
                }
                rows.push((label, dir, digest));
            }
        }
        const PINNED: [(&str, u8, u64); 9] = [
            ("flat", 0, 0xac0b18b7fefd6b24),
            ("flat", 64, 0x08f5ee0d3d3a8a71),
            ("flat", 200, 0x460aa21305fa361e),
            ("slope", 0, 0xc08c70ac23803686),
            ("slope", 64, 0xf9181f0d41a59396),
            ("slope", 200, 0x232514d1a22c32ab),
            ("edge", 0, 0x5e58ff7a20e38e37),
            ("edge", 64, 0x4a4511140845b207),
            ("edge", 200, 0xd9ddf29af434f66b),
        ];
        let listing = rows
            .iter()
            .map(|(label, dir, digest)| format!("            ({label:?}, {dir}, {digest:#018x}),"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            rows, PINNED,
            "the lying pose moved; the rows now read:\n{listing}"
        );
    }
}
