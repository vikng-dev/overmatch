//! Trees in the game's damage model: a hard body with an HP pool and a state. Game rounds dose it,
//! tanks push it, and it falls at a segment.
//!
//! A TREE IS DATA (ADR-0034). `assets/trees/<id>/<id>.tree.ron` ([`TreeAsset`]) is the trunk as a
//! chain of tapered segments, each boundary's radius stored once — every segment's base radius,
//! then the tip's — so adjacent segments meet by construction and a segment's top radius is read
//! through [`TreeKind::r_top_m`]. TREES ARE JUST WOOD: `assets/trees/wood.ron` ([`Wood`]) is what
//! the sim reads of every tree — the wood substance, the HP per metre of breast-height diameter and
//! the ram threshold — and `assets/trees/species.ron` only names the species an asset may claim
//! (the generator's look and ranges; the sim reads no value of a row). All three are read once at
//! startup into the map's [`TreeLibrary`] and validated there; every number the sim uses comes from
//! them, the glb is the view (ADR-0014).
//!
//! THE BODY. One static root per tree — the [`BallisticVolume`] of the wood and the HP pool its
//! diameter gives ([`TreeBody::hp`]), the way a module's pool is charged — and one child per
//! segment with its own convex frustum collider (two 12-gon rings) on `[Terrain, Armor]` with
//! filters `ALL`: hulls contact it and the union walk slows the round by chord × factor and doses
//! the pool the energy it lost. Never a compound: the walk's convex collector reads one solid per
//! collider. The instance's uniform scale is applied to the dimensions at spawn and the root's own
//! scale is one.
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

/// Directory under the asset root holding the species table, the wood row and one folder per tree
/// asset.
const TREES_DIR: &str = "trees";

/// The species table's file name inside [`TREES_DIR`].
const SPECIES_FILE: &str = "species.ron";

/// The wood row's file name inside [`TREES_DIR`].
const WOOD_FILE: &str = "wood.ron";

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

/// One trunk segment: a frustum `height_m` tall with `r_bottom_m` at its base, metres at asset
/// scale 1. Its top radius is the next segment's `r_bottom_m`, the last segment's the asset's
/// `r_tip_m` ([`TreeKind::r_top_m`]).
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Segment {
    pub(crate) height_m: f32,
    pub(crate) r_bottom_m: f32,
}

/// `<id>.tree.ron`: the trunk, base to tip, the radius at its tip, and the species it belongs to.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TreeAsset {
    pub(crate) species: String,
    pub(crate) segments: Vec<Segment>,
    /// The last segment's top radius, metres at asset scale 1.
    pub(crate) r_tip_m: f32,
}

/// The species table as the sim reads it: the species an asset may name. A row's values are the
/// generator's (look and ranges) and are not read here.
type SpeciesTable = BTreeMap<String, serde::de::IgnoredAny>;

/// `wood.ron`: what the sim reads of every tree, whatever its species.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Wood {
    /// The trunk's substance, a hard key of the substance registry.
    substance: String,
    /// The trunk's HP pool per metre of breast-height diameter.
    hp_per_m: Sourced<f32>,
    /// The ram threshold's constant, N·s/m²: a tick's `Σ impulse × lever` fells the trunk at
    /// `fell_k × dbh³`.
    fell_k: Sourced<f32>,
}

/// One tree asset resolved against the species table, the wood row and the substance registry,
/// validated: what every instance of it spawns from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TreeKind {
    pub(crate) id: String,
    pub(crate) segments: Vec<Segment>,
    /// The last segment's top radius at asset scale 1; read a segment's top through
    /// [`Self::r_top_m`].
    r_tip_m: f32,
    pub(crate) material: VolumeMaterial,
    pub(crate) substance: String,
    pub(crate) fell_k: f32,
    /// HP points per metre of breast-height diameter ([`TreeBody::hp`]).
    pub(crate) hp_per_m: f32,
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
        table: &SpeciesTable,
        wood: &Wood,
        registry: &SubstanceRegistry,
    ) -> Result<Self, String> {
        let TreeAsset {
            species,
            segments,
            r_tip_m,
        } = asset;
        if segments.is_empty() || segments.len() > MAX_SEGMENTS {
            return Err(format!(
                "{} segments; a trunk has 1 to {MAX_SEGMENTS}",
                segments.len()
            ));
        }
        let positive = |v: f32| v.is_finite() && v > 0.0;
        for (k, segment) in segments.iter().enumerate() {
            if !(positive(segment.height_m) && positive(segment.r_bottom_m)) {
                return Err(format!(
                    "segment {k} has a non-positive dimension: {segment:?}"
                ));
            }
        }
        if !positive(r_tip_m) {
            return Err(format!("r_tip_m {r_tip_m} is not a positive finite radius"));
        }
        if !table.contains_key(&species) {
            return Err(format!("species {species:?} is not in {SPECIES_FILE}"));
        }
        let material = VolumeMaterial::of(registry, &wood.substance)
            .map_err(|err| format!("{WOOD_FILE} substance: {err}"))?;
        let (fell_k, hp_per_m) = (wood.fell_k.0, wood.hp_per_m.0);
        for (name, value) in [("fell_k", fell_k), ("hp_per_m", hp_per_m)] {
            if !positive(value) {
                return Err(format!("{WOOD_FILE} {name} {value} is not positive"));
            }
        }
        let mut kind = Self {
            id: id.to_owned(),
            segments,
            r_tip_m,
            material,
            substance: wood.substance.clone(),
            fell_k,
            hp_per_m,
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

    /// Segment `k`'s top radius at asset scale 1: the boundary it shares with segment `k + 1`,
    /// whose `r_bottom_m` stores it, or the tip's `r_tip_m` above the last segment. The one
    /// reading of a top radius, so a segment's top and its upper neighbour's base are one float.
    pub(crate) fn r_top_m(&self, k: usize) -> f32 {
        self.segments
            .get(k + 1)
            .map_or(self.r_tip_m, |next| next.r_bottom_m)
    }

    /// The trunk's radius `height` metres above the base along the authored taper, at asset scale
    /// 1; the base radius below the base, the tip radius above the tip.
    pub(crate) fn radius_at(&self, height: f32) -> f32 {
        let mut bottom = 0.0;
        for (k, segment) in self.segments.iter().enumerate() {
            let top = bottom + segment.height_m;
            if height < top {
                let t = ((height - bottom) / segment.height_m).clamp(0.0, 1.0);
                return segment.r_bottom_m + (self.r_top_m(k) - segment.r_bottom_m) * t;
            }
            bottom = top;
        }
        self.r_tip_m
    }
}

/// Every tree asset the selected map places, resolved and validated, keyed by asset id.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct TreeLibrary {
    kinds: BTreeMap<String, Arc<TreeKind>>,
    /// The texts the kinds were read from — the species table, the wood row, then each asset in id
    /// order — for the map's content digest.
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
        let table: SpeciesTable = ron::from_str(&species_text).unwrap_or_else(|err| {
            panic!("trees: {} failed to parse: {err}", species_path.display())
        });
        let wood_path = root.join(TREES_DIR).join(WOOD_FILE);
        let wood_text = read(&wood_path);
        let wood: Wood = ron::from_str(&wood_text)
            .unwrap_or_else(|err| panic!("trees: {} failed to parse: {err}", wood_path.display()));
        let registry = SubstanceRegistry::shipped_static();
        let mut texts = vec![species_text, wood_text];
        let mut kinds = BTreeMap::new();
        for id in ids {
            let path = root.join(TREES_DIR).join(id).join(format!("{id}.tree.ron"));
            let text = read(&path);
            let asset: TreeAsset = ron::from_str(&text)
                .unwrap_or_else(|err| panic!("trees: {} failed to parse: {err}", path.display()));
            let kind = TreeKind::resolve(id, asset, &table, &wood, registry)
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

    /// The trunk's HP pool: the wood's HP per metre of diameter times this instance's breast-height
    /// diameter, rounded to the nearest point (one f32 multiply and `roundf`, the same on every
    /// target), and at least one point: a pool of none would never empty.
    pub(crate) fn hp(&self) -> u32 {
        (libm::roundf(self.kind.hp_per_m * self.dbh_m) as u32).max(1)
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

/// The set the authority's fall laws run in; the ledger publish and the body derive order after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TreeFallSet;

/// A fallen tree's resting pose, derived once from its state and the terrain: the piece above
/// `cut` lies tilted by `tilt` radians from upright toward the horizontal unit direction
/// `(dx, dz)`, turned about `seat` — the resting centre of its butt (the cut boundary), an
/// offset from the tree's base in world axes. `hinge` is the tilt about the cut's bottom at
/// which the piece first met the terrain, the first stage of the view's fall. A cut at the base
/// keeps its butt at the base: `seat` is zero and `hinge == tilt`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct Resting {
    pub(crate) hinge: f32,
    pub(crate) tilt: f32,
    pub(crate) seat: Vec3,
    pub(crate) dx: f32,
    pub(crate) dz: f32,
}

/// The largest tilt the hinge is searched over: past the horizontal by 10°, for a piece hanging
/// over a downhill edge.
const MAX_TILT: f32 = 100.0 * PI / 180.0;

/// The largest tilt a lying piece is searched over about its seat: a lying piece follows a slope
/// of up to 45° downhill (DERIVED: 90° + 45°); past that the ground is an edge and the piece
/// lies level over it — the scripted law does not slide, the rigid body will.
const LYING_MAX_TILT: f32 = 135.0 * PI / 180.0;

/// The step the tilt range is scanned at for the first contact. The scan can step over a
/// terrain feature narrower than `step × distance` at a sample, and the bisection inside a
/// bracket can still land one bracket late on one, so the step is set so the feature a piece
/// could visibly rest on cannot be skipped at the tip of the longest piece the species table
/// allows: 40 m (MEASURED: the tallest `height_m` bound in `assets/trees/species.ron`). 0.25°
/// sweeps 0.17 m at 40 m (DERIVED: 40 × 0.25° in radians); the shipped grid's cell is 0.366 m
/// (MEASURED: 4096 samples across 1500 m, `assets/maps/kalinovo/`), so a one-cell feature at
/// that tip is wider than a step.
const TILT_SCAN_STEP: f32 = 0.25 * PI / 180.0;

/// The halvings that take a step-wide bracket below one f32 ulp at the angles in range (DERIVED:
/// 0.25° is 4.4e-3 rad; 2^16 halvings reach 6.7e-8, under the 1.2e-7 ulp of an angle near
/// 1 rad). Past the ulp a halving is a no-op, so the bisection stops at no progress.
const TILT_STEPS: u32 = 16;

/// The horizontal unit direction of azimuth bin `dir`, libm's trig so every peer gets the same
/// bits.
pub(crate) fn azimuth(dir: u8) -> (f32, f32) {
    let theta = f32::from(dir) / AZIMUTH_BINS as f32 * std::f32::consts::TAU;
    (libm::cosf(theta), libm::sinf(theta))
}

/// The smallest tilt in `[0, max]` at which any sample of a piece turned about `pivot` toward
/// `(dx, dz)` reaches the terrain: a sample `(h, r)` lies `h` along the piece's axis, lowered by
/// `r`. Contact is not monotone in the tilt — a sample can touch a ridge at one tilt and clear
/// it at a larger one, its height still dropping as it moves past the ridge onto lower ground —
/// so the range is scanned at multiples of [`TILT_SCAN_STEP`] in increasing order (the range's
/// end is a whole number of steps: 400 for [`MAX_TILT`], 540 for [`LYING_MAX_TILT`], DERIVED)
/// and the first touching step brackets the contact, which [`TILT_STEPS`] halvings resolve to
/// the bracket's touching end. Touching at zero is zero; touching at no step is `None`.
fn first_contact(
    pivot: Vec3,
    samples: &[(f32, f32)],
    (dx, dz): (f32, f32),
    max: f32,
    height_at: &impl Fn(f32, f32) -> f32,
) -> Option<f32> {
    let touching = |tilt: f32| {
        let (sin, cos) = (libm::sinf(tilt), libm::cosf(tilt));
        samples.iter().any(|&(h, r)| {
            let x = pivot.x + dx * h * sin;
            let y = pivot.y + h * cos - r;
            let z = pivot.z + dz * h * sin;
            y <= height_at(x, z)
        })
    };
    let last = libm::roundf(max / TILT_SCAN_STEP) as u32;
    let k = (0..=last).find(|&k| touching(k as f32 * TILT_SCAN_STEP))?;
    if k == 0 {
        return Some(0.0);
    }
    let (mut lo, mut hi) = ((k - 1) as f32 * TILT_SCAN_STEP, k as f32 * TILT_SCAN_STEP);
    for _ in 0..TILT_STEPS {
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            break;
        }
        if touching(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

/// The pose a fallen piece rests in, from the body, the world base, the decoded state and the
/// terrain oracle. Pure in those inputs, libm-only in its trig and in one fixed order of
/// evaluation, so every peer and CI derive the same bits. The samples are each segment boundary
/// above the cut and the tip, each lowered by its local radius; the cut's own boundary is never
/// one, and a sample's `xz` goes to the oracle as is (it clamps at the map's edge).
///
/// The hinge first: the piece turns about the cut's bottom on the root's axis until a sample
/// first meets the terrain ([`first_contact`] over [`MAX_TILT`]; nothing reached by 100° lies
/// level, at 90°). A cut at the base rests there, its butt at the base. For a cut above the base
/// the butt slides off the stump and seats beside it on the fall's side, its edge against the
/// stump's edge — `2r` along `(dx, dz)` for the cut's radius `r`, the stump's top and the butt
/// being the same boundary (DERIVED) — with its centre `r` above the terrain there, and the
/// piece turns about the seat until a sample first meets the terrain, over [`LYING_MAX_TILT`]
/// (nothing reached lies level). On flat ground a sample `(h, r_s)` touches when
/// `cos tilt = (r_s − r) / h` (DERIVED), so a tapered piece lies just past the horizontal.
pub(crate) fn resting_pose(
    body: &TreeBody,
    base: Vec3,
    cut: u8,
    dir: u8,
    height_at: impl Fn(f32, f32) -> f32,
) -> Resting {
    let cut = usize::from(cut);
    let (dx, dz) = azimuth(dir);
    let samples: Vec<(f32, f32)> = (cut + 1..body.prefix.len())
        .map(|j| {
            (
                body.prefix[j] - body.prefix[cut],
                body.radius_at(body.prefix[j]),
            )
        })
        .collect();
    let hinge = first_contact(
        base + Vec3::Y * body.prefix[cut],
        &samples,
        (dx, dz),
        MAX_TILT,
        &height_at,
    )
    .unwrap_or(FRAC_PI_2);
    if cut == 0 {
        return Resting {
            hinge,
            tilt: hinge,
            seat: Vec3::ZERO,
            dx,
            dz,
        };
    }
    let r = body.radius_at(body.prefix[cut]);
    let (sx, sz) = (dx * (2.0 * r), dz * (2.0 * r));
    let sy = height_at(base.x + sx, base.z + sz) + r - base.y;
    let seat = Vec3::new(sx, sy, sz);
    let tilt = first_contact(base + seat, &samples, (dx, dz), LYING_MAX_TILT, &height_at)
        .unwrap_or(FRAC_PI_2);
    Resting {
        hinge,
        tilt,
        seat,
        dx,
        dz,
    }
}

/// The rotation that tilts the piece above a cut by `angle` toward `(dx, dz)` of `resting`, in
/// the frame of a tree root turned by `root`: about the horizontal world axis `(dz, 0, -dx)`
/// carried into that frame, libm's half-angle trig. Under the root it is the world tilt composed
/// over the root's yaw, so a lying piece keeps the instance's yaw. The ONE source of a fallen
/// piece's rotation: the sim's lying segments take it at the resting tilt ([`derive_state`]),
/// the view's cut trunk node at each stage of the fall and at the resting tilt when it lands
/// (`view::present_fall`), both under the root's `Transform` rotation with no rotation between
/// (a segment is the root's child; the view's chain carries none, A5), so the two compose under
/// the root (`Transform::mul_transform`) to the same rotation, bit for bit. Past the transforms
/// the engines differ: avian's collider `Rotation` is the root's `Rotation` — derived from the
/// spawn transform's affine — times this, normalized, and the renderer composes affines.
pub(crate) fn fall_rotation(resting: &Resting, root: Quat, angle: f32) -> Quat {
    let axis = root.inverse() * Vec3::new(resting.dz, 0.0, -resting.dx);
    let (s, c) = (libm::sinf(0.5 * angle), libm::cosf(0.5 * angle));
    Quat::from_xyzw(axis.x * s, axis.y * s, axis.z * s, c)
}

/// The seat of `resting` in the frame of a tree root turned by `root`, world metres: the pivot
/// the sim's lying segments ([`derive_state`]) and the view's cut trunk node
/// (`view::present_fall`) both turn about, taken from here by both so they compose the same
/// bits. Zero for a zero seat.
pub(crate) fn seat_in_root(resting: &Resting, root: Quat) -> Vec3 {
    root.inverse() * resting.seat
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
/// stump's segments stand the same and every segment from the cut up lies in the resting pose
/// on [`lying_layers`], the pose is recorded and the pool is gone — what is down takes no more
/// dose. A tree back to standing (a replaced ledger) or down at another cut derives exactly as
/// one that never held the old state.
///
/// The lying piece is turned rigidly about the seat — the butt's resting centre, which is the
/// cut's bottom only for a cut at the base — so in the root's frame each lying segment is turned
/// by [`fall_rotation`] at the resting tilt and sits at the seat ([`seat_in_root`]) plus its
/// height above the cut, turned the same.
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
                let resting = resting_pose(body, transform.translation, cut, dir, |x, z| {
                    grid.height_at(x, z)
                });
                let turn = fall_rotation(&resting, transform.rotation, resting.tilt);
                let seat = seat_in_root(&resting, transform.rotation);
                Some((usize::from(cut), resting, turn, seat))
            }
        };
        let mut k = 0;
        for &child in children {
            let Ok((mut local, layers)) = segments.get_mut(child) else {
                continue;
            };
            let (pose, filter) = match fallen {
                Some((cut, _, turn, seat)) if k >= cut => {
                    let pose = Transform {
                        translation: seat + turn * (Vec3::Y * (body.prefix[k] - body.prefix[cut])),
                        rotation: turn,
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
            Some((_, resting, _, _)) => {
                commands
                    .entity(root)
                    .insert(resting)
                    .remove::<ComponentHealth>();
            }
            None => {
                commands.entity(root).remove::<(Resting, Fell)>();
                if !has_pool {
                    commands.entity(root).insert(ComponentHealth {
                        current: body.hp(),
                        max: body.hp(),
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

/// Segment `k`'s collider in its own frame: base ring at y = 0, top ring at the segment's height.
fn segment_collider(kind: &TreeKind, k: usize, scale: f32) -> Collider {
    let segment = &kind.segments[k];
    let (height, r_bottom, r_top) = (
        segment.height_m * scale,
        segment.r_bottom_m * scale,
        kind.r_top_m(k) * scale,
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
/// the resting pose and the cut both read the trunk as upright. Returns the root; the body is
/// whole without a view, which a composition that draws attaches under the root itself.
pub(crate) fn spawn(
    commands: &mut Commands,
    kind: &Arc<TreeKind>,
    identity: MapInstance,
    pose: Transform,
) -> Entity {
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
    let hp = body.hp();
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
                current: hp,
                max: hp,
            },
            TreeState::Standing,
            identity,
            body,
        ))
        .id();
    for (k, &bottom) in prefix[..kind.segments.len()].iter().enumerate() {
        commands.spawn((
            ChildOf(root),
            Transform::from_xyz(0.0, bottom, 0.0),
            segment_collider(kind, k, scale),
            standing_layers(),
            TreeSegment,
        ));
    }
    root
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
    use avian3d::prelude::SimpleCollider;

    /// A three-segment fixture trunk (fixture data): 2 m segments, radii 0.3 → 0.2 → 0.1 → 0.05.
    pub(crate) fn fixture_asset() -> TreeAsset {
        TreeAsset {
            species: "fixture".to_owned(),
            segments: vec![
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.3,
                },
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.2,
                },
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.1,
                },
            ],
            r_tip_m: 0.05,
        }
    }

    fn fixture_table() -> SpeciesTable {
        BTreeMap::from([("fixture".to_owned(), serde::de::IgnoredAny)])
    }

    /// The fixture wood (fixture data): RHA, 200 HP per metre of diameter, so the fixture trunk's
    /// 0.47 m holds 94 points at scale 1.
    fn fixture_wood() -> Wood {
        Wood {
            substance: "RHA".to_owned(),
            hp_per_m: Sourced(200.0),
            fell_k: Sourced(1.0e5),
        }
    }

    fn resolve(asset: TreeAsset, table: &SpeciesTable, wood: &Wood) -> Result<TreeKind, String> {
        TreeKind::resolve(
            "fixture",
            asset,
            table,
            wood,
            SubstanceRegistry::shipped_static(),
        )
    }

    /// The fixture trunk of `substance` whose pool holds `hp` points at scale 1 — fixture data for
    /// the march's tests, which need a trunk a round goes through and a pool a field alone empties.
    /// Its HP per metre is `hp` over the fixture's diameter, so [`TreeBody::hp`] reads `hp` back.
    pub(crate) fn fixture_kind_of(substance: &str, hp: u32) -> Arc<TreeKind> {
        let wood = Wood {
            substance: substance.to_owned(),
            ..fixture_wood()
        };
        let mut kind =
            resolve(fixture_asset(), &fixture_table(), &wood).expect("the fixture trunk holds");
        kind.hp_per_m = hp as f32 / kind.dbh_m;
        let kind = Arc::new(kind);
        assert_eq!(
            TreeBody::new(kind.clone(), 1.0).hp(),
            hp,
            "the fixture pool"
        );
        kind
    }

    pub(crate) fn fixture_kind() -> Arc<TreeKind> {
        Arc::new(
            resolve(fixture_asset(), &fixture_table(), &fixture_wood())
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

    /// EVERY A6 REFUSAL IS A REFUSAL, NOT A CLAMP: an empty trunk, a non-finite or non-positive
    /// segment dimension, a missing, non-positive or non-finite tip radius, an unknown species, a
    /// wood row whose HP per metre or ram threshold is not positive, a trunk too short to read a
    /// breast height, and more segments than the wire word's cut byte can name are each refused by
    /// name. Any one of these laxed to a default — a NaN height carried, a zero or infinite tip
    /// spawned, a pool of no HP per metre spawned, a 256th segment accepted — reds its row.
    #[test]
    fn every_malformed_asset_or_wood_row_is_refused_by_name() {
        let refusal = |asset: TreeAsset, wood: Wood| {
            resolve(asset, &fixture_table(), &wood).expect_err("refused")
        };
        let mut asset = fixture_asset();
        asset.segments.clear();
        assert!(refusal(asset, fixture_wood()).contains("segments"));
        let mut asset = fixture_asset();
        asset.segments[0].height_m = f32::NAN;
        assert!(refusal(asset, fixture_wood()).contains("dimension"));
        let mut asset = fixture_asset();
        asset.segments[2].r_bottom_m = 0.0;
        assert!(refusal(asset, fixture_wood()).contains("dimension"));
        for r_tip_m in [0.0, f32::INFINITY] {
            let mut asset = fixture_asset();
            asset.r_tip_m = r_tip_m;
            assert!(refusal(asset, fixture_wood()).contains("r_tip_m"));
        }
        let missing = ron::from_str::<TreeAsset>(
            r#"TreeAsset(species: "fixture", segments: [Segment(height_m: 2.0, r_bottom_m: 0.3)])"#,
        )
        .expect_err("a trunk without its tip radius")
        .to_string();
        assert!(missing.contains("r_tip_m"), "{missing}");
        let mut asset = fixture_asset();
        asset.species = "no_such_species".to_owned();
        assert!(refusal(asset, fixture_wood()).contains("species"));
        for hp_per_m in [0.0, -1.0, f32::NAN] {
            let wood = Wood {
                hp_per_m: Sourced(hp_per_m),
                ..fixture_wood()
            };
            assert!(refusal(fixture_asset(), wood).contains("hp_per_m"));
        }
        let wood = Wood {
            fell_k: Sourced(-1.0),
            ..fixture_wood()
        };
        assert!(refusal(fixture_asset(), wood).contains("fell_k"));
        let mut asset = fixture_asset();
        for segment in &mut asset.segments {
            segment.height_m = 0.4;
        }
        assert!(refusal(asset, fixture_wood()).contains("breast"));
        let mut asset = fixture_asset();
        let stub = asset.segments[2];
        asset.segments = vec![stub; MAX_SEGMENTS + 1];
        assert!(refusal(asset, fixture_wood()).contains("segments"));
    }

    /// A wood row whose substance the registry does not carry, or carries as a witness, cannot be
    /// a layer of the walk: refused at load.
    #[test]
    fn an_unknown_or_witness_substance_is_refused() {
        for substance in ["Balsa", "Flesh"] {
            let wood = Wood {
                substance: substance.to_owned(),
                ..fixture_wood()
            };
            resolve(fixture_asset(), &fixture_table(), &wood).expect_err(substance);
        }
    }

    /// THE TRUNK'S POOL FOLLOWS ITS THICKNESS: the wood's HP per metre times the instance's
    /// breast-height diameter, rounded to the nearest point, and never below one. The fixture
    /// trunk's 0.47 m at 200 HP per metre holds 94 points at scale 1 and 188 at scale 2; a mutant
    /// reading the asset's diameter instead of the instance's holds 94 at both, one keeping a fixed
    /// pool holds the same at every scale, and one truncating reads 93 at scale 0.9975 (93.77). A
    /// pool of no points would never empty, so a sliver of a trunk still holds one.
    #[test]
    fn the_trunk_pool_is_the_wood_rate_times_the_diameter() {
        let hp = |scale: f32| TreeBody::new(fixture_kind(), scale).hp();
        assert_eq!(hp(1.0), 94);
        assert_eq!(hp(2.0), 188);
        assert_eq!(hp(0.9975), 94, "rounded to the nearest point");
        assert_eq!(hp(1.0e-4), 1, "never below one point");
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

    /// The threshold is the wood's constant times the CUBE of the breast-height diameter at the
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
    /// the same way does not, though it touches the trunk and its contacts, read as the law
    /// reads a tank's, carry the fell torque in one tick: only the body's identity spares the
    /// tree. A law reading `Hull` off the pair's body fells neither; a turret that never reaches
    /// its trunk leaves the tree standing for no reason the law gives, and reds this.
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
        let nudged_segments: Vec<Entity> = app
            .world()
            .get::<Children>(nudged)
            .expect("segments")
            .iter()
            .collect();
        let base_y = app
            .world()
            .get::<Transform>(nudged)
            .expect("the root")
            .translation
            .y;
        // Each tick: whether the turret touches a segment of its tree with a positive normal
        // impulse, and the torque those contacts carry as the ram law reads a tank's.
        let (mut touched, mut peak) = (false, 0.0_f32);
        for _ in 0..20 {
            app.update();
            let graph = app.world().resource::<ContactGraph>();
            let pairs: Vec<&ContactPair> = nudged_segments
                .iter()
                .filter_map(|segment| graph.get(turret, *segment))
                .map(|(_, pair)| pair)
                .filter(|pair| pair.is_touching())
                .collect();
            touched |= pairs
                .iter()
                .any(|pair| pair.total_normal_impulse_magnitude() > 0.0);
            let contacts = ram_contacts(
                pairs.into_iter(),
                |collider| nudged_segments.contains(&collider).then_some(nudged),
                |_| true,
                |_| Some(base_y),
            );
            if let Some(contacts) = contacts.get(&nudged) {
                peak = peak.max(ram_torque(contacts).0);
            }
        }
        let threshold = app
            .world()
            .get::<TreeBody>(nudged)
            .expect("the body")
            .fell_torque();
        assert!(
            touched,
            "the turret touches its tree with a positive normal impulse"
        );
        assert!(
            peak >= threshold,
            "the turret's push carries the fell torque in one tick: {peak} N·s·m of {threshold}"
        );
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

    /// The spawn builds the body the module doc describes: one root with the pool and the kind's
    /// volume — its material and substance — one frustum collider per segment on
    /// `[Terrain, Armor]`, nothing on the root's scale, and no view — the sim spawns none.
    /// A root carrying another substance's volume, or a segment hulled as a cylinder at either
    /// ring's radius, reds this.
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
        assert_eq!(
            (health.current, health.max),
            (141, 141),
            "the wood's 200 HP per metre at the instance's 0.705 m (DERIVED, fixture data)"
        );
        let volume = world
            .get::<BallisticVolume>(root)
            .expect("the root's volume");
        assert_eq!(
            (volume.material, volume.substance.as_str()),
            (kind.material, kind.substance.as_str()),
            "the volume is the kind's"
        );
        assert_eq!(transform.scale, Vec3::ONE);
        assert_eq!(transform.translation, pose.translation);
        assert_eq!(
            transform.rotation, pose.rotation,
            "the yaw is the instance's"
        );
        let standing = CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL);
        let mut segments: Vec<(Entity, f32, Collider)> = world
            .query_filtered::<
                (&ChildOf, &Transform, &Collider, &CollisionLayers),
                With<TreeSegment>,
            >()
            .iter(&world)
            .map(|(parent, transform, collider, layers)| {
                assert_eq!(*layers, standing, "a standing segment stops hulls and rounds");
                (parent.parent(), transform.translation.y, collider.clone())
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
        // origin: the ring table holds the four axis points, so the bounds are exact. And each
        // end has its own ring's radius: a tenth of a millimetre inside either end, a ray along Z
        // past the 12-gon's +X vertex meets the hull a millimetre inside that ring's scaled
        // radius and misses it a millimetre outside. DERIVED margin: that far in, the
        // cross-section's radius is off the end ring's by `|r_bottom - r_top| × 1e-4 / height`,
        // under 10 µm on this fixture.
        const PROBE: f32 = 1.0e-3;
        const INSIDE: f32 = 1.0e-4;
        for (k, ((_, _, collider), segment)) in segments.iter().zip(&kind.segments).enumerate() {
            let r_top_m = kind.r_top_m(k);
            let aabb = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
            let r = segment.r_bottom_m.max(r_top_m) * 1.5;
            let h = segment.height_m * 1.5;
            assert!(
                aabb.min.abs_diff_eq(Vec3::new(-r, 0.0, -r), 1e-6)
                    && aabb.max.abs_diff_eq(Vec3::new(r, h, r), 1e-6),
                "the hull is the scaled frustum: {aabb:?} for {segment:?}"
            );
            for (y, ring) in [
                (INSIDE, segment.r_bottom_m * 1.5),
                (h - INSIDE, r_top_m * 1.5),
            ] {
                let meets = |x: f32| {
                    collider
                        .cast_ray(
                            Vec3::ZERO,
                            Quat::IDENTITY,
                            Vec3::new(x, y, -1.0),
                            Vec3::Z,
                            2.0,
                            true,
                        )
                        .is_some()
                };
                assert!(
                    meets(ring - PROBE) && !meets(ring + PROBE),
                    "at {y} m the hull's radius is the ring's {ring}: {segment:?}"
                );
            }
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

    /// THE RESTING POSE IS PINNED TO THE BIT (A9): the fixture trunk on flat ground, on a 10 %
    /// slope and at the grid's edge (where the oracle clamps), at every cut and three bins, must
    /// derive to these exact f32 bits of the hinge, the tilt and the seat on every platform —
    /// the lying collider is sim state every peer derives alone, so a peer that differs by one
    /// ulp has a different world. The rows pin the pose's bits. A tilt is the touching end of
    /// the bracket the scan finds, bisected to adjacent floats: a halving past that is a no-op,
    /// and where contact is monotone it is the float a bisection of the whole range reaches too,
    /// so the cut-0 rows carry that bisection's bits and the ridge test, not these rows, tells
    /// the scan from it. What moves a row: the midpoint taken the other way (the bracket's far
    /// end comes back), the sample's radius offset dropped, the platform's trig in place of
    /// libm's, the oracle's clamp at the edge, and the seat formula — `2r` beside the stump
    /// along the bin, `r` above the terrain THERE (a seat read at the base's height moves every
    /// slope and edge row above the base). The cut-0 rows reach the terrain about the cut and
    /// pin the hinge search; the rows above the base reach nothing by 100° about the cut and pin
    /// the fallback's 90° as the hinge, then the seat and the search about it. A reordering of
    /// the samples inside `any` is not observable here. The pin is a platform-drift tripwire:
    /// CI's Linux bits must match these macOS bits.
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
                    let pose = resting_pose(&body, *base, cut, dir, |x, z| grid.height_at(x, z));
                    rows.push((
                        *label,
                        cut,
                        dir,
                        pose.hinge.to_bits(),
                        pose.tilt.to_bits(),
                        pose.seat.to_array().map(f32::to_bits),
                    ));
                }
            }
        }
        #[rustfmt::skip]
        const PINNED: [(&str, u8, u8, u32, u32, [u32; 3]); 27] = [
            ("flat", 0, 0, 0x3fbc3d92, 0x3fbc3d92, [0x00000000, 0x00000000, 0x00000000]),
            ("flat", 0, 64, 0x3fbc3d92, 0x3fbc3d92, [0x00000000, 0x00000000, 0x00000000]),
            ("flat", 0, 200, 0x3fbc3d92, 0x3fbc3d92, [0x00000000, 0x00000000, 0x00000000]),
            ("flat", 1, 0, 0x3fc90fdb, 0x3fcddcf2, [0x3ecccccd, 0x3e4ccccd, 0x00000000]),
            ("flat", 1, 64, 0x3fc90fdb, 0x3fcddcf2, [0xb29630f2, 0x3e4ccccd, 0x3ecccccd]),
            ("flat", 1, 200, 0x3fc90fdb, 0x3fcddcf2, [0x3d9fd16d, 0x3e4ccccd, 0xbec8dd65]),
            ("flat", 2, 0, 0x3fc90fdb, 0x3fcc4324, [0x3e4ccccd, 0x3dcccccd, 0x00000000]),
            ("flat", 2, 64, 0x3fc90fdb, 0x3fcc4324, [0xb21630f2, 0x3dcccccd, 0x3e4ccccd]),
            ("flat", 2, 200, 0x3fc90fdb, 0x3fcc4324, [0x3d1fd16d, 0x3dcccccd, 0xbe48dd65]),
            ("slope", 0, 0, 0x3faf8c01, 0x3faf8c01, [0x00000000, 0x00000000, 0x00000000]),
            ("slope", 0, 64, 0x3fbc3d91, 0x3fbc3d91, [0x00000000, 0x00000000, 0x00000000]),
            ("slope", 0, 200, 0x3fb9bf04, 0x3fb9bf04, [0x00000000, 0x00000000, 0x00000000]),
            ("slope", 1, 0, 0x3fc90fdb, 0x3fc114e5, [0x3ecccccd, 0x3e75c300, 0x00000000]),
            ("slope", 1, 64, 0x3fc90fdb, 0x3fcddcf0, [0xb29630f2, 0x3e4cccc0, 0x3ecccccd]),
            ("slope", 1, 200, 0x3fc90fdb, 0x3fcb5d80, [0x3d9fd16d, 0x3e54c9c0, 0xbec8dd65]),
            ("slope", 2, 0, 0x3fc90fdb, 0x3fbf7d30, [0x3e4ccccd, 0x3df5c200, 0x00000000]),
            ("slope", 2, 64, 0x3fc90fdb, 0x3fcc4323, [0xb21630f2, 0x3dcccd00, 0x3e4ccccd]),
            ("slope", 2, 200, 0x3fc90fdb, 0x3fc9c3d5, [0x3d1fd16d, 0x3dd4ca00, 0xbe48dd65]),
            ("edge", 0, 0, 0x3fb5ca04, 0x3fb5ca04, [0x00000000, 0x00000000, 0x00000000]),
            ("edge", 0, 64, 0x3fbc3d97, 0x3fbc3d97, [0x00000000, 0x00000000, 0x00000000]),
            ("edge", 0, 200, 0x3fb9bf0a, 0x3fb9bf0a, [0x00000000, 0x00000000, 0x00000000]),
            ("edge", 1, 0, 0x3fc90fdb, 0x3fcb9f4e, [0x3ecccccd, 0x3e75c300, 0x00000000]),
            ("edge", 1, 64, 0x3fc90fdb, 0x3fcddcf1, [0xb29630f2, 0x3e4ccc00, 0x3ecccccd]),
            ("edge", 1, 200, 0x3fc90fdb, 0x3fcb5d77, [0x3d9fd16d, 0x3e54c900, 0xbec8dd65]),
            ("edge", 2, 0, 0x3fc90fdb, 0x3fc7243e, [0x3e4ccccd, 0x3df5c000, 0x00000000]),
            ("edge", 2, 64, 0x3fc90fdb, 0x3fcc4319, [0xb21630f2, 0x3dccca00, 0x3e4ccccd]),
            ("edge", 2, 200, 0x3fc90fdb, 0x3fc9c3c3, [0x3d1fd16d, 0x3dd4cc00, 0xbe48dd65]),
        ];
        let listing = rows
            .iter()
            .map(|(label, cut, dir, hinge, tilt, [sx, sy, sz])| {
                format!(
                    "            ({label:?}, {cut}, {dir}, {hinge:#010x}, {tilt:#010x}, \
                     [{sx:#010x}, {sy:#010x}, {sz:#010x}]),"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            rows, PINNED,
            "the resting pose moved; the rows now read:\n{listing}"
        );
        // Flat ground, whole trunk: it lies just short of the horizontal, held off the surface by
        // the first boundary above the cut — the widest sample, 0.2 m at 2 m, touching at 84.3°
        // (DERIVED: cos tilt = r / h) before the narrower ones — and rests where it hinged, its
        // butt at the base. A piece cut above the base hinges at the cut's height and reaches
        // nothing by 100°, so its hinge is the 90° fallback; seated on the ground it lies just
        // past the horizontal, its tip down (DERIVED: cos tilt = (r_tip − r) / h < 0).
        for row in rows.iter().filter(|row| row.0 == "flat") {
            let (hinge, tilt) = (f32::from_bits(row.3), f32::from_bits(row.4));
            let seat = Vec3::from_array(row.5.map(f32::from_bits));
            if row.1 == 0 {
                assert!(tilt > 80.0_f32.to_radians() && tilt < FRAC_PI_2, "{row:?}");
                assert!(hinge == tilt && seat == Vec3::ZERO, "{row:?}");
            } else {
                assert_eq!(hinge, FRAC_PI_2, "{row:?}");
                assert!(tilt > FRAC_PI_2 && tilt < 95.0_f32.to_radians(), "{row:?}");
                assert!(seat.y > 0.0 && seat.xz().length() > 0.0, "{row:?}");
            }
        }
    }

    /// The height a sample of the piece above `cut` rests above the terrain, per sample, in
    /// the resting pose about the seat: negative is below the ground.
    fn clearances(
        body: &TreeBody,
        base: Vec3,
        cut: usize,
        resting: &Resting,
        height_at: impl Fn(f32, f32) -> f32,
    ) -> Vec<f32> {
        let pivot = base + resting.seat;
        let (sin, cos) = (libm::sinf(resting.tilt), libm::cosf(resting.tilt));
        (cut + 1..body.prefix.len())
            .map(|j| {
                let h = body.prefix[j] - body.prefix[cut];
                let r = body.radius_at(body.prefix[j]);
                let x = pivot.x + resting.dx * h * sin;
                let y = pivot.y + h * cos - r;
                let z = pivot.z + resting.dz * h * sin;
                y - height_at(x, z)
            })
            .collect()
    }

    /// THE FIRST CONTACT IS THE ONE THE PIECE RESTS ON, not a later one: contact is not
    /// monotone in the tilt, so a search that assumes it is lays the piece through a ridge. The
    /// fixture trunk at cut 0 toward +X over flat ground with a 2.9 m shelf at `x` in
    /// (2.5, 3.0): its 4 m boundary (r 0.1) passes over the shelf for tilts in (38.7°, 48.6°)
    /// (DERIVED: `4 sin θ` in (2.5, 3.0)) and is on it from 41.4° (DERIVED:
    /// `4 cos θ − 0.1 ≤ 2.9`); the flat contact is at 84.3°. The pose must rest in the shelf's
    /// touching interval. A bisection of the whole range answers the flat contact and reds
    /// this; so does a scan step coarser than the shelf's interval. One primitive serves the
    /// hinge and the seat, so a cut at the base — whose tilt is its hinge — exercises both.
    #[test]
    fn the_first_contact_is_bracketed_not_bisected_through_a_ridge() {
        let body = TreeBody::new(fixture_kind(), 1.0);
        let shelf = |x: f32, _: f32| if x > 2.5 && x < 3.0 { 2.9 } else { 0.0 };
        let pose = resting_pose(&body, Vec3::ZERO, 0, 0, shelf);
        let lower = libm::acosf((2.9 + 0.1) / 4.0);
        let upper = libm::asinf(3.0 / 4.0);
        assert!(
            pose.tilt >= lower - 1e-5 && pose.tilt < upper,
            "ridge: tilt {}° ({:#010x}), the shelf's touching interval is [{}°, {}°)",
            pose.tilt.to_degrees(),
            pose.tilt.to_bits(),
            lower.to_degrees(),
            upper.to_degrees()
        );
        assert!(pose.hinge == pose.tilt && pose.seat == Vec3::ZERO);
        let clearances = clearances(&body, Vec3::ZERO, 0, &pose, shelf);
        assert!(
            clearances[1].abs() < 1e-5 && clearances[2] > 1.0,
            "the 4 m boundary rests on the shelf, the tip hangs past it: {clearances:?}"
        );
    }

    /// THE BUTT SEATS BESIDE THE STUMP, ON THE GROUND: for a cut above the base on flat ground,
    /// in three bins, the seat is exactly `(dx × 2r, r, dz × 2r)` for the cut's radius `r` (the
    /// butt's edge against the stump's edge, its centre a radius above the terrain), and the
    /// piece lies about it just past the horizontal: on flat ground a sample `(h, r_s)` touches
    /// at `cos tilt = (r_s − r) / h` and the smallest such tilt wins — the tip's, 92.15° at
    /// cut 1 (h 4, r 0.05 against r 0.2) before the 2 m boundary's 92.87°, and 91.43° at cut 2
    /// (h 2, r 0.05 against r 0.1) (DERIVED). Generic on every case: no sample lies below the
    /// terrain and one lies on it, within the search's resolution — the piece rests ON the
    /// ground, never in it (DERIVED resolution: the bracket ends under one ulp of the angle,
    /// 2.4e-7 rad at 2 rad, times the 4 m lever is 1e-6 m; 1e-5 is that with margin). A seat
    /// at `r` beside the stump, or at the stump's height, or a tilt that stops at the hinge's
    /// 90°, reds this.
    #[test]
    fn the_butt_seats_beside_the_stump_on_the_ground() {
        let body = TreeBody::new(fixture_kind(), 1.0);
        let flat = |_: f32, _: f32| 0.0;
        let base = Vec3::new(5.0, 0.0, -3.0);
        for (cut, derived_deg) in [(1_u8, 92.149_1_f32), (2, 91.432_5)] {
            let r = body.radius_at(body.prefix[usize::from(cut)]);
            for dir in [0_u8, 64, 200] {
                let pose = resting_pose(&body, base, cut, dir, flat);
                let (dx, dz) = azimuth(dir);
                assert_eq!(
                    pose.seat.to_array().map(f32::to_bits),
                    [dx * (2.0 * r), r, dz * (2.0 * r)].map(f32::to_bits),
                    "cut {cut} dir {dir}: the seat {:?}",
                    pose.seat
                );
                assert!(
                    (pose.tilt.to_degrees() - derived_deg).abs() < 1e-3,
                    "cut {cut} dir {dir}: tilt {}° for {derived_deg}°",
                    pose.tilt.to_degrees()
                );
                assert_eq!(pose.hinge, FRAC_PI_2, "nothing reached about the cut");
                let clearances = clearances(&body, base, usize::from(cut), &pose, flat);
                let lowest = clearances.iter().copied().fold(f32::INFINITY, f32::min);
                assert!(
                    lowest.abs() < 1e-5,
                    "cut {cut} dir {dir}: the lowest sample rests on the ground: {clearances:?}"
                );
            }
        }
    }

    /// THE HINGE IS THE FIRST CONTACT ABOUT THE CUT, THE SEAT BESIDE IT: a cut above the base
    /// whose tip reaches a 45° slope rising along the fall (`height = x`, dir 0, cut 1) hinges
    /// at 65.2° (DERIVED: the tip, h 4 lowered 0.05 from the cut's 2 m, meets `y = x` where
    /// `2 + 4 cos θ − 0.05 = 4 sin θ`; the 2 m boundary's contact is later, at 87.2°), and the
    /// seat beside the stump sits on the slope, `2r` out and `2r + r` up; on flat ground at
    /// cut 1 nothing is reached by 100° and the hinge is exactly the 90° fallback. A seat on a
    /// ledge higher than the stump's top (a 3 m step beside the tree) is above the cut's bottom,
    /// so the butt drops nothing: the view's second stage takes its floor duration from that. A
    /// hinge searched about the seat, or a seat read at the base's height, reds this.
    #[test]
    fn the_hinge_is_the_first_contact_about_the_cut() {
        let body = TreeBody::new(fixture_kind(), 1.0);
        let slope = |x: f32, _: f32| x;
        let uphill = resting_pose(&body, Vec3::ZERO, 1, 0, slope);
        assert!(
            (uphill.hinge.to_degrees() - 65.164).abs() < 1e-2,
            "the hinge on the slope: {}°",
            uphill.hinge.to_degrees()
        );
        let r = body.radius_at(body.prefix[1]);
        assert_eq!(
            uphill.seat,
            Vec3::new(2.0 * r, 2.0 * r + r, 0.0),
            "the seat on the slope"
        );
        assert!(
            uphill.tilt < uphill.hinge && uphill.tilt > 45.0_f32.to_radians(),
            "seated, the piece lies up the slope, steeper than 45°: {}°",
            uphill.tilt.to_degrees()
        );
        let flat = resting_pose(&body, Vec3::ZERO, 1, 0, |_, _| 0.0);
        assert_eq!(flat.hinge, FRAC_PI_2, "nothing reached by 100°");
        let ledge = resting_pose(
            &body,
            Vec3::ZERO,
            1,
            0,
            |x, _| if x > 0.1 { 3.0 } else { 0.0 },
        );
        assert!(
            ledge.seat.y >= body.prefix[1],
            "the seat on the ledge, {}, is no lower than the stump's top, {}",
            ledge.seat.y,
            body.prefix[1]
        );
        assert!(
            ledge.hinge < FRAC_PI_2 && ledge.tilt > FRAC_PI_2,
            "hinged on the ledge's top at {}°, lying on it at {}°",
            ledge.hinge.to_degrees(),
            ledge.tilt.to_degrees()
        );
    }

    /// A derive bed: the fixture tree spawned at `pose` on `grid`, the derive in `Update`.
    pub(crate) fn derive_bed(grid: HeightGrid, pose: Transform) -> (App, Entity) {
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
    pub(crate) fn segments_of(app: &App, root: Entity) -> Vec<(Transform, CollisionLayers)> {
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
    /// the cut up leaves the vertical onto the lying filter — the cut's segment at the recorded
    /// seat beside the stump, the ones above it along the fall — the resting pose is recorded,
    /// the setter's transition tick stays and the pool is gone; back to standing, every segment
    /// stands at its spawn height on the standing filter, the pose and the transition tick are
    /// gone and the pool is back; down at another cut, the segments the new cut spares stand
    /// again and the rest lie. A derive that skips `Standing`, touches only the segments at and
    /// above the new cut, turns the piece about the stump's top instead of the seat, or leaves
    /// the pool, the pose or the tick from the state before reds this.
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
            down_1[1].0.translation, resting.seat,
            "the cut's segment turns about the seat (the root is unturned)"
        );
        assert!(
            resting.seat.x > 0.0 && resting.seat.y > 0.0 && resting.seat.y < 2.0,
            "the seat is beside the stump, on the ground, below the stump's top: {}",
            resting.seat
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
        assert_eq!(
            (pool.current, pool.max),
            (94, 94),
            "full: the fixture wood's 200 HP per metre at 0.47 m"
        );
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

    /// THE FALL ROTATION IS THE WORLD TILT OVER THE YAW (A1): under a yawed root, the fall
    /// rotation carries the root's up axis to the tilted direction `(dx sin a, cos a, dz sin a)`
    /// and the root's ring axis `yaw × X` to that axis tilted the same — the ring's own axes
    /// turned by the yaw, then laid down — so a lying piece keeps the instance's yaw. A rotation
    /// built about the world axis without carrying it into the root's frame lays every ring down
    /// unturned under the yaw, which the 12-gon makes observable. DERIVED bound: each compared
    /// component is under a hundred roundings of unit-scale f32 values, each at most 2⁻²⁴ ≈ 6e-8,
    /// so the two computations differ below 6e-6; 1e-5 is that with margin.
    #[test]
    fn the_fall_rotation_is_the_world_tilt_over_the_yaw() {
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        for dir in [0_u8, 64, 200] {
            for angle in [0.3_f32, FRAC_PI_2, 1.7] {
                let (dx, dz) = azimuth(dir);
                let resting = Resting {
                    hinge: angle,
                    tilt: angle,
                    seat: Vec3::ZERO,
                    dx,
                    dz,
                };
                let world = yaw * fall_rotation(&resting, yaw, angle);
                let (s, c) = (libm::sinf(0.5 * angle), libm::cosf(0.5 * angle));
                let tilt = Quat::from_xyzw(dz * s, 0.0, -dx * s, c);
                let up = Vec3::new(
                    dx * libm::sinf(angle),
                    libm::cosf(angle),
                    dz * libm::sinf(angle),
                );
                assert!(
                    (world * Vec3::Y).abs_diff_eq(up, 1e-5),
                    "dir {dir}, {angle} rad: up goes to {} for {up}",
                    world * Vec3::Y
                );
                let ring = tilt * (yaw * Vec3::X);
                assert!(
                    (world * Vec3::X).abs_diff_eq(ring, 1e-5),
                    "dir {dir}, {angle} rad: the ring axis goes to {} for {ring}",
                    world * Vec3::X
                );
            }
        }
    }

    /// THE LYING POSE IS PINNED TO THE BIT (A9): on flat ground, on the slope and at the grid's
    /// edge, in three bins, a yawed instance's lying segments derive to these exact local
    /// transform bits (translation and rotation, folded in segment order) — the whole trunk at
    /// cut 0, turned about the base, and the piece above cut 1, turned about its seat carried
    /// into the root's frame — on every platform: glam runs scalar-math and the trig is libm's,
    /// so a peer that differs by one ulp has a different world. Any change to the composition
    /// of the pose — the yaw dropped, the seat not carried under the yaw, the pivot put back
    /// on the stump, a factor reordered — reds this; a change to the search or the seat reds
    /// the pose pins first.
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
            for cut in [0_u8, 1] {
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
                        TreeState::Down { cut, dir };
                    app.update();
                    let segments = segments_of(&app, root);
                    assert_eq!(segments.len(), 3);
                    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
                    for (local, _) in &segments[usize::from(cut)..] {
                        for value in local
                            .translation
                            .to_array()
                            .into_iter()
                            .chain(local.rotation.to_array())
                        {
                            digest = fold(digest, value.to_bits());
                        }
                    }
                    rows.push((label, cut, dir, digest));
                }
            }
        }
        const PINNED: [(&str, u8, u8, u64); 18] = [
            ("flat", 0, 0, 0xac6fffa0efcf2646),
            ("flat", 0, 64, 0xb54e527901b36078),
            ("flat", 0, 200, 0x883a6b667889c39a),
            ("flat", 1, 0, 0x10aaf2ccf734387e),
            ("flat", 1, 64, 0xd05a4cd6beae4604),
            ("flat", 1, 200, 0x3a448554fb539f86),
            ("slope", 0, 0, 0x6bb3658585200064),
            ("slope", 0, 64, 0xc093756d96f81e08),
            ("slope", 0, 200, 0x5dc081d11110a614),
            ("slope", 1, 0, 0x6a8f163c5f1ff04a),
            ("slope", 1, 64, 0x41d246947c6254c9),
            ("slope", 1, 200, 0xaf65f1b33313a071),
            ("edge", 0, 0, 0x364f8ea69e370d27),
            ("edge", 0, 64, 0x5cd06b3c1f68bc21),
            ("edge", 0, 200, 0xd4fb70177b475810),
            ("edge", 1, 0, 0x38dcd8bf2606a450),
            ("edge", 1, 64, 0x9c8e6c0aeafa2b67),
            ("edge", 1, 200, 0x768f4fd11abf373f),
        ];
        let listing = rows
            .iter()
            .map(|(label, cut, dir, digest)| {
                format!("            ({label:?}, {cut}, {dir}, {digest:#018x}),")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            rows, PINNED,
            "the lying pose moved; the rows now read:\n{listing}"
        );
    }
}
