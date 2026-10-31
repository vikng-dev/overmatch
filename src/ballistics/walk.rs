//! The §13 union field walk — the pure core.
//!
//! The live march and the §13.6 gate run it through `super::resolve::decide`. Everything here is a
//! pure function over pre-collected data: no ECS, no spatial queries, no commands, no wall clock,
//! no RNG. Those live in the Avian adapter (`super::resolve`, `super::collect`), which is what makes
//! the laws testable to the bit.
//!
//! # What it replaces
//!
//! The serial resolver charges one volume at a time and probes ahead *restricted to the struck
//! entity*, so an overlapping or exactly-abutting neighbour is entered from inside, its exit probe
//! finds nothing, and it is crossed at ZERO cost (§13.1's table). §13.2's answer is not a special
//! case for seams but a different law: along the ray the volumes form a **material-factor field**,
//! cost is `∫ max(factor) dt` over it, and every boundary law fires where the field *steps*. `max`
//! is forced by the invariants (idempotent, monotone, commutative — §13.2), not chosen.
//!
//! # The three mechanisms, on three orthogonal axes (§13)
//!
//! - **union** (§13.2) — shared space is charged once. Implemented as the canonical span stream.
//! - **ε-weld** (§13.4) — a longitudinal micro-gap deletes phantom *faces*, never creates phantom
//!   *steel*.
//! - **disc sampling** (§13.5) — the shell meets the world as a caliber-wide body: k sample rays,
//!   aggregated into `(η, n̄, cost)` per crossing event.
//!
//! # Pairing is a fact, not an inference
//!
//! Every crossing arrives naming the closed shell it came from ([`ShellKey`], from the manifold
//! gate's own edge-connected components) AND the welded feature the ray met — a triangle interior,
//! a welded edge, or a welded vertex. Occupancy is tracked per shell, and that is the whole of the
//! topology law. Without it the interface is genuinely ambiguous: two shells of one primitive
//! closing at one `t` and one shell's exit claimed by both triangles of its shared edge present the
//! same `t`, the same winding sign and the same normal.
//!
//! IDENTITY IS THE ONLY DEDUP. Claims are one crossing when they name one contact of one shell,
//! never because their `t` are close: there is no coincidence window here, no proximity test and no
//! signed net. Two contacts are two crossings however close they land, so a shell thinner than any
//! window is an ordinary traversal — charged, with its own entrance and exit. The parameter is
//! carried at `f64` from the collector's exact projection through to the cost integral, so two
//! contacts keep their order and their chargeable chord even where their public `f32` values
//! coincide.
//!
//! # Staged, because normalization bends the axis
//!
//! Entry faces are sampled along the INCOMING axis, but the transit happens along the bent one (the
//! serial resolver bends at `ballistics.rs`'s `bend_toward` call before probing the exit). One
//! pre-collected hit list therefore cannot describe both. So the core is a two-stage continuation:
//! [`begin`] consumes the entrance disc and returns either a ricochet plan or a
//! [`TransitRequest`] naming the axis and frame the caller must collect the transit corridor along;
//! [`finish`] consumes that corridor, advancing the round's mechanism (`super::mechanism`) through
//! the crossing's ordered [`Encounter`]s, and returns the [`ResolutionPlan`]. Fixture tests answer
//! the request from fixtures; the runtime answers it with real casts.
//!
//! Each sample hands off between the stages on the face of the piece it crossed
//! ([`DiscWalk::hand_off`]), and each stage's crossing is its first piece plus every piece that
//! begins inside it ([`DiscWalk::first_crossing`]). The rule, and why no plane is
//! shared across the disc, is the design doc's §13.5, "The hand-off"
//! (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! # Fail loud, never repair
//!
//! Unpairable topology returns a structured [`WalkError`] naming the sample, volume, primitive and
//! violated invariant. The core NEVER synthesizes a missing exit, infers "we must have started
//! inside", or charges zero and moves on — silent-zero armour is the exact defect class §13.1
//! exists to kill. Deciding what to do with the error (bake/CI: hard failure; production authority:
//! deterministic fail-closed) is the slice-2 adapter's job, not this module's.
//!
//! # Three tolerance domains, deliberately separate
//!
//! [`WalkLaws::topology_abs`]/[`WalkLaws::topology_rel`] relate a corridor RESTART to the face it
//! hands off on, and have no part in pairing; [`WalkLaws::weld_perp`] is the §13.4 physical
//! event-topology knob (~2 mm
//! perpendicular); [`WalkLaws::event_plane_tolerance`] is the lateral surface-patch relation the
//! disc uses to associate samples. They are three orders of magnitude apart and must never be
//! collapsed into one "epsilon" — a march/query offset in particular must never reach this module,
//! since the old marcher's 1 mm nudge alone would eat half the weld budget.

use bevy::prelude::*;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use super::mechanism::{
    Control, Dose, DoseSource, Effect, Geometry, Limits, Mechanism, OvermatchProjection,
    overlap_factor,
};
use crate::substances::{SubstanceError, SubstanceId, SubstanceRegistry};

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------------------------
// Input vocabulary
// ---------------------------------------------------------------------------------------------

/// One ballistic mesh primitive — the grain presence is REPORTED at.
///
/// The primitive is an `Entity` rather than an index because the bind already gives every glb mesh
/// primitive its own collider entity (`tank::spawn::insert_ballistic_volumes`), so the identity the
/// walk needs already exists and is stable for the frame — an index would be a second, weaker name
/// for the same thing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PrimitiveKey {
    /// The `BallisticVolume` node — where the factor, the HP and the damage address live.
    pub volume: Entity,
    /// The collider entity carrying this primitive's geometry.
    pub primitive: Entity,
}

/// One closed shell of one primitive — the grain presence and pairing are TRACKED at.
///
/// SURFACE IDENTITY, and the walk's whole basis for pairing. §13.7 authors a mixed-substance part
/// as several closed islands in one primitive, so a primitive names a SET of surfaces — and at that
/// grain "two shells close at one `t`" and "one shell's exit is claimed by both triangles of its
/// shared edge" are the same numbers: same `t`, same winding sign, same normal. One means the depth
/// drops by two, the other by one, and no rule over `t`, normals or triangle valence can tell them
/// apart. Every such rule that shipped erased armour on some legal mesh.
///
/// The identity is not invented here. `bake::manifold_gate` builds the edge-connected components to
/// prove closure and outward winding per island; the shell id is those components, published
/// instead of discarded. Edge-connected, deliberately: two legal shells may touch at one welded
/// VERTEX (§13.7's islands), and vertex components would merge them back into one ambiguous name.
///
/// EPHEMERAL, AND DELIBERATELY NOT A WIRE IDENTITY. Two of its three fields are Bevy entities,
/// allocated per world, and the third is dense in the bake's own triangle order — so an exporter
/// that reorders faces renumbers it. It carries no `Serialize`, no `Reflect` and no derive that
/// could put it on a wire, and nothing in `net` names it: the walk runs on the authority and
/// publishes outcomes, never topology. A future consumer that needs shell identity across a save,
/// a replay or the wire must state it as (volume slot, primitive slot, shell index) plus the bake's
/// own hash, and refuse a mismatched bake — never these bits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ShellKey {
    /// The `BallisticVolume` node — where the factor, the HP and the damage address live.
    pub volume: Entity,
    /// The collider entity carrying this primitive's geometry.
    pub primitive: Entity,
    /// Dense index of the closed shell inside that primitive (`bake::MeshGeometry::shells`).
    pub shell: u32,
}

impl ShellKey {
    /// The primitive this shell belongs to.
    pub fn primitive_key(&self) -> PrimitiveKey {
        PrimitiveKey {
            volume: self.volume,
            primitive: self.primitive,
        }
    }
}

/// The welded feature of a shell the ray actually met — the identity that decides which claims are
/// the same crossing.
///
/// EXACT, and free. The sheared edge test is antisymmetric between the two triangles incident on an
/// edge, so a claim's barycentric areas cancel to EXACTLY zero on that edge for both of them, and on
/// two edges at a shared vertex for the whole fan around it. That zero pattern is the contact, and
/// it is the same pattern in every triangle that names the feature, whatever the mesh's own index
/// buffer calls it (the ids are the bake's WELDED vertices). A ray that is merely NEAR a feature
/// cancels nothing and is claimed by exactly one triangle's interior.
///
/// So dedup is by identity, not by proximity: two claims are one crossing when they name one
/// contact of one shell, and never because their `t` happen to be close.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Contact {
    /// The interior of one triangle, named by its index. Claimed by that triangle alone.
    Face(u32),
    /// A welded edge, named by its two welded vertex ids in ascending order.
    Edge(u32, u32),
    /// A welded vertex — the whole fan around it names this.
    Vertex(u32),
}

/// One triangle crossing along a sample ray, as the spatial adapter reports it.
///
/// `true_normal` is the face's OUTWARD normal recovered from triangle winding — NOT parry's
/// reported normal, which is flipped to oppose the ray, so a backface read from inside is
/// indistinguishable from a head-on entry (§13.1). Recovering the true orientation is the adapter's
/// contract, and [`entering`](Self::entering) is that orientation decided; this module classifies
/// entry/exit from it and would silently invert the whole walk if handed the flipped one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceHit {
    pub volume: Entity,
    pub primitive: Entity,
    /// Which closed shell of that primitive this face belongs to — see [`ShellKey`].
    pub shell: u32,
    /// The welded feature this claim is ABOUT. Claims sharing one contact of one shell are one
    /// crossing; claims of different contacts never are.
    pub contact: Contact,
    /// Diagnostic identity only. It never participates in the reduction below (which is topological,
    /// so a re-triangulated mesh cannot change the answer); it exists so a [`WalkError`] can name
    /// the offending faces.
    pub triangle: u32,
    /// Distance along the sample ray from the corridor origin. CANONICAL PER CONTACT: every claim
    /// of one welded feature reports the same bits, because the collector computes it from the
    /// feature rather than from each triangle's own plane.
    ///
    /// f64, and that is a law rather than a precision preference. It is the quantity crossings are
    /// ORDERED by and chords are MEASURED with, and both fail at f32 width: two distinct contacts of
    /// one shell can land a fraction of an f32 ULP apart, where f32 puts an exit before its own entry
    /// and collapses a real thickness to a zero chord that is then crossed free. Everything the walk
    /// publishes as a POSITION is still f32 — a metre-scale coordinate needs no more — but the
    /// parameter itself stays wide from the collector's exact projection through to the cost integral.
    pub t: f64,
    pub true_normal: Vec3,
    /// Whether the ray crosses this face INTO its shell (against the outward normal). Decided by
    /// the collector with the predicate that admitted the face — the sign of the exact crossing
    /// determinant — so every admitted face has a direction. `true_normal` is the face's geometry
    /// and never re-decides it: its f32 dot with the axis can cancel to zero, or flip, on a crossing
    /// whose exact determinant does not.
    pub entering: bool,
}

impl FaceHit {
    fn key(&self) -> ShellKey {
        ShellKey {
            volume: self.volume,
            primitive: self.primitive,
            shell: self.shell,
        }
    }
}

/// What a volume is made of, as the walk and the mechanisms read it: the substance's identity, its
/// declared resistance, reference-mm per metre of chord
/// (`crate::substances::HardSubstance::factor`), and the field law's ratio of the spall cone an exit through it throws
/// (`crate::substances::HardSubstance::debris_ratio`). Hard matter only: a witness (ADR-0040)
/// states no resistance and is never a walk volume.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VolumeMaterial {
    pub substance: SubstanceId,
    pub factor: f32,
    pub debris_ratio: f32,
}

/// The debris ratio of every fixture material ([`VolumeMaterial::fixture`]): a 30° cone. Fixture
/// data.
#[cfg(test)]
pub(crate) const FIXTURE_DEBRIS_RATIO: f32 = 2.0;

impl VolumeMaterial {
    /// The hard substance named `name` in `registry`. Hard error on an unknown name or a witness.
    pub fn of(registry: &SubstanceRegistry, name: &str) -> Result<Self, SubstanceError> {
        let hard = registry.hard(name)?;
        Ok(Self {
            substance: registry.id(name)?,
            factor: hard.factor(),
            debris_ratio: hard.debris_ratio,
        })
    }

    /// A fixture material of `factor` reference-mm per metre, its substance one per factor
    /// ([`SubstanceId::fixture`]), its debris ratio [`FIXTURE_DEBRIS_RATIO`].
    #[cfg(test)]
    pub(crate) fn fixture(factor: f32) -> Self {
        Self {
            substance: SubstanceId::fixture(factor),
            factor,
            debris_ratio: FIXTURE_DEBRIS_RATIO,
        }
    }
}

/// The material of every volume the corridor can meet, supplied ONCE per volume.
///
/// Not per hit: a volume has one substance (§13.7), and repeating it on every face invites two
/// faces of one plate disagreeing — an unrepresentable state that the union would then have to
/// arbitrate.
#[derive(Clone, Debug, Default)]
pub struct VolumeTable {
    materials: BTreeMap<Entity, VolumeMaterial>,
}

impl VolumeTable {
    /// Build the table, rejecting factors that cannot participate in `max` or in a deterministic
    /// sum. A `NaN` poisons both the field maximum and the sort; a negative factor breaks
    /// monotonicity (§13.6). Zero is canonicalized so `-0.0` cannot produce a second bit pattern
    /// for "no material" and defeat the bit-equality coalescing in [`walk_ray`].
    ///
    /// One volume, one entry; one substance, one factor. Interfaces compare substance identity
    /// while mechanisms read factors, so a substance carrying two factors would change resistance
    /// where no interface is reported ([`WalkError::SubstanceFactorConflict`]), and a repeated
    /// volume would have one of its materials silently dropped ([`WalkError::RepeatedVolume`]).
    pub fn new(
        entries: impl IntoIterator<Item = (Entity, VolumeMaterial)>,
    ) -> Result<Self, WalkError> {
        let mut materials = BTreeMap::new();
        let mut factors: BTreeMap<SubstanceId, f32> = BTreeMap::new();
        for (volume, material) in entries {
            let factor = material.factor;
            if !factor.is_finite() || factor < 0.0 {
                return Err(WalkError::BadFactor { volume, factor });
            }
            let factor = if factor == 0.0 { 0.0 } else { factor };
            let declared = *factors.entry(material.substance).or_insert(factor);
            if declared.to_bits() != factor.to_bits() {
                return Err(WalkError::SubstanceFactorConflict {
                    substance: material.substance,
                    first: declared,
                    second: factor,
                });
            }
            if materials
                .insert(volume, VolumeMaterial { factor, ..material })
                .is_some()
            {
                return Err(WalkError::RepeatedVolume { volume });
            }
        }
        Ok(Self { materials })
    }

    /// Every `(volume, material)` the table carries, in entity order.
    ///
    /// Exists for the §13.6 idempotence/monotonicity gate ([`super::fuzz`]), which re-walks a real
    /// corridor with one volume DUPLICATED and therefore has to state the same table plus one row.
    /// Reconstructing it from the world instead would prove the property about a table the walk was
    /// never handed.
    pub fn entries(&self) -> impl Iterator<Item = (Entity, VolumeMaterial)> + '_ {
        self.materials
            .iter()
            .map(|(volume, material)| (*volume, *material))
    }

    fn material(&self, volume: Entity) -> Result<VolumeMaterial, WalkError> {
        self.materials
            .get(&volume)
            .copied()
            .ok_or(WalkError::UnknownVolume { volume })
    }

    fn factor(&self, volume: Entity) -> Result<f32, WalkError> {
        Ok(self.material(volume)?.factor)
    }
}

/// One sample ray's pre-collected corridor.
///
/// The interval is HALF-OPEN, `[0, length)`. Parry reports a hit exactly at its maximum TOI, so two
/// adjacent closed corridors would both claim a boundary sitting on their shared end; half-open
/// gives that boundary exactly one owner. Hits at or past `length` are therefore not this
/// corridor's, and a primitive left open at the end is [`WalkError::IncompleteCorridor`] — never a
/// synthesized exit.
#[derive(Clone, Debug, Default)]
pub struct RayCorridor {
    /// WORLD position this corridor's local frame hangs off. Every other position here — including
    /// [`origin`](Self::origin) — is measured FROM it, and stays at corridor scale.
    ///
    /// f32 near the edge of a 2.5 km map resolves to 0.24 mm, so a world position and anything
    /// derived from it are quantised at that step INDEPENDENTLY: two exact-equal quantities computed
    /// by different routes come back up to a quarter millimetre apart. That is what put a transit
    /// handoff 0.38 mm off the very face it was computed from (MEASURED at
    /// `(2499.9, 924.963, 1524.939)`, 38° incidence) and let a real entry face be pruned in-envelope.
    /// Anchoring kills it at the source: the world-scale subtraction happens ONCE, on the anchor, and
    /// every relationship the walk cares about is then arithmetic on small numbers.
    #[expect(
        dead_code,
        reason = "the frame `origin` is measured from; the walk reads the disc corridor's copy, which every ray corridor inherits unchanged"
    )]
    pub anchor: Vec3,
    /// Corridor start, RELATIVE to [`anchor`](Self::anchor).
    pub origin: Vec3,
    /// Unit travel direction.
    pub axis: Vec3,
    pub length: f32,
    /// Shells the corridor origin is ALREADY inside, declared by the caller.
    ///
    /// Never inferred. "The first hit is an exit, so we must have started inside" silently converts
    /// a dropped entry face into legitimate topology — which is how a hole in a mesh becomes free
    /// armour. An exit with neither declared presence nor a prior entry is an error.
    ///
    /// Per SHELL, not per primitive, and that is load-bearing: a corridor restarting inside two
    /// shells of one primitive that seeds one presence meets two exits and underflows. Identity has
    /// to survive the restart or the restart re-introduces the ambiguity it was built to remove.
    pub initial_presence: Vec<ShellKey>,
    pub hits: Vec<FaceHit>,
}

// ---------------------------------------------------------------------------------------------
// Laws / knobs
// ---------------------------------------------------------------------------------------------

/// Every tunable the walk consumes. Sandbox knobs (§4: "the structure is the design; the magnitudes
/// are live knobs"), gathered so no law reads a hidden constant.
#[derive(Clone, Copy, Debug)]
pub struct WalkLaws {
    /// SEED domain — absolute floor (m) under which two `t` name the same boundary at a restart.
    ///
    /// It relates a RESTART to a face, and nothing else: a corridor restarts at a `t` read on one
    /// ray (a sample's hand-off, a perforation's resume point) while the faces near it are
    /// re-measured from the new corridor's own origin, so the two agree only to rounding. It decides
    /// which side of the restart owns a face ([`RayWalk::inside_at`], `collect::admit`). It has no
    /// part in pairing, which is topological.
    pub topology_abs: f32,
    /// SEED domain — relative term, so a corridor anchored far downrange (where f32 spacing is
    /// coarser) still recognises its own restart.
    pub topology_rel: f32,
    /// WELD domain (§13.4) — maximum PERPENDICULAR gap that merges event topology (~2 mm: far above
    /// export jitter, two orders below real spaced armour).
    pub weld_perp: f32,
    /// Weld face compatibility: the exit face and the next entry face must describe approximately
    /// opposing sides of one gap, `n_exit · n_entry <= -weld_face_cos`. Without it the grazing
    /// formula lets a near-tangent SIDE face weld to unrelated geometry arbitrarily far downrange —
    /// `|axis · n| → 0` makes any gap "perpendicular-small".
    pub weld_face_cos: f32,
    /// Weld lookahead ceiling (m) along the ray. Guarantees corridor termination and bounds the
    /// same runaway from the other side. Deliberately far below real spaced armour.
    pub weld_max_lookahead: f32,
    /// Cumulative omitted perpendicular gap allowed across a CHAIN of welds in one run.
    ///
    /// RULED 2026-08-07 (§13.4): chaining is transitive but budgeted, and the cap IS the tolerance —
    /// the same 2 mm knob, deliberately, because one constant bounds both extremes at once. A single
    /// two-plate sandwich welds across up to 2 mm of air; an arbitrarily long picket fence can never
    /// collapse into one run with one terminal exit by chaining.
    pub weld_run_gap_budget: f32,
    /// LATERAL domain — two samples' entrance patches lie on one surface when their normals agree
    /// to this cosine …
    pub event_plane_cos: f32,
    /// … and their entry points are within this perpendicular distance (m) of the shared plane.
    /// Longitudinal overlap alone cannot associate samples: on a thin oblique slab the ring shifts
    /// by `r·tan(incidence)`, so one sample can exit before another enters and one ordinary plate
    /// would split into several crossing events.
    pub event_plane_tolerance: f32,
    /// Along-ray slop (m) allowed between where a sample's run STARTS and where one shared surface
    /// predicts it should. The longitudinal sibling of [`event_plane_tolerance`](Self::event_plane_tolerance),
    /// and deliberately not derived from it: a perpendicular tolerance divided by `cos(incidence)`
    /// grows without bound as the ray goes parallel to the surface, which is the runaway the
    /// longitudinal conjunct exists to stop.
    ///
    /// It bounds a RESIDUAL, so it can be tight without shattering anything. Every sample of one
    /// plane has residual zero BY CONSTRUCTION however far apart along the ray they land, and real
    /// faceting is carried by transitivity — adjacent ring samples are a quarter radius apart, so
    /// their shared surface is locally flat even where the plate is not.
    pub event_residual_tolerance: f32,
    /// Ceiling on `sec(incidence)` — dimensionless, and the only knob left in the association
    /// geometry.
    ///
    /// DIMENSIONS, EXPLICITLY, because a cap read in the wrong unit mispredicts the geometry. The
    /// predicted separation of two samples is `−(d·n̄)/(axis·n̄)`, whose worst case over a disc is
    /// `|d| = 2r` with `d` up the plane's steepest line: `2·r·sin(i)·sec(i) = 2·r·tan(i)`. Capping
    /// the SECANT at `C` therefore caps the reach at `2·r·C·sin(i) ≤ 2·r·C` — that is `C` DIAMETERS,
    /// not `C` radii. The previous formulation capped `2·tan` itself, which made the same `10` mean
    /// ten RADII, five diameters, and split a valid ring-only same-plane contact in two.
    ///
    /// GENEROUS, deliberately. Its only job is numerical: the division blows up as the ray goes
    /// parallel to the surface. Below it the relation is EXACT, and a cap that engages anywhere a
    /// valid crossing can happen does not bound the geometry, it mispredicts it. A hundred engages
    /// past 89.43°, an order beyond any incidence at
    /// which a round both declines to ricochet and still presents a resolvable chord. The reach it
    /// implies is never the operative bound — the residual is.
    pub event_secant_cap: f32,
    /// Past this incidence (rad, from the surface normal) an un-overmatched round ricochets. The
    /// speed a bounce keeps is no knob: it is the striking speed's share along the surface
    /// ([`Begin::Ricochet`]).
    pub ricochet_angle: f32,
    /// Share of the incidence angle the round straightens toward the normal as it bites into the
    /// reference substance; [`begin`] scales it by the entrance substance's share of
    /// [`rha_reference`](Self::rha_reference).
    pub normalization: f32,
    /// Overmatch when caliber ≥ this × the crossing's factor-weighted perpendicular thickness.
    pub overmatch_ratio: f32,
    /// Reference-mm per metre of the reference substance (RHA), the divisor turning a cost back
    /// into steel-equivalent METRES so it can be compared against a caliber, and a factor into the
    /// entry bend's resistance share.
    pub rha_reference: f32,
    // NO `significant_step` knob. §13's "meaningful factor step" is still an open tab, and a
    // declared-but-unread knob is a false affordance — it advertises a lever whose default is
    // correct only by accident. The pathology it was meant to gate (a thick soft volume deflecting
    // a main-gun round) is already dead by a different mechanism: factor-weighted overmatch
    // (§13.3), which is exercised by `an_arm_does_not_ricochet_an_88`. Add the knob back only with
    // a reader and a test.
}

impl Default for WalkLaws {
    fn default() -> Self {
        Self {
            // 1 µm: three orders below the weld tolerance and below any authored feature.
            topology_abs: 1.0e-6,
            // ~3 f32 ULP. Deliberately tiny: the corridor is ORIGIN-RELATIVE (the driver anchors it
            // where the march segment started, and a continuation keeps that anchor within a step and
            // is re-anchored between steps, so it sits at most the step's reach plus `resolve::MAX_OVERRUN` behind any corridor), so
            // `t` stays within tens of metres of the anchor and this term only has to cover intersection
            // rounding — a relative term large enough to matter at world scale would swallow the
            // millimetre distinction the weld tolerance is written in.
            topology_rel: 4.0e-7,
            weld_perp: 2.0e-3,
            // ~60°: parallel plates read -1; anything less opposed is not two sides of one gap.
            weld_face_cos: 0.5,
            weld_max_lookahead: 5.0e-2,
            weld_run_gap_budget: 2.0e-3,
            // ~25°.
            event_plane_cos: 0.9,
            event_plane_tolerance: 2.0e-3,
            event_residual_tolerance: 1.0e-2,
            event_secant_cap: 100.0,
            // Carried verbatim from the serial resolver so slice 2 is a resolution change, not a
            // retune.
            ricochet_angle: 1.221,
            normalization: 0.2,
            overmatch_ratio: 3.0,
            rha_reference: 1000.0,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// A violated invariant, named precisely enough to fix the mesh or the adapter that produced it.
#[derive(Clone, Debug, PartialEq)]
pub enum WalkError {
    /// A factor that cannot participate in `max` or in a deterministic sum.
    BadFactor { volume: Entity, factor: f32 },
    /// One substance declared at two factors in one volume table.
    SubstanceFactorConflict {
        substance: SubstanceId,
        first: f32,
        second: f32,
    },
    /// One volume supplied twice to a volume table.
    RepeatedVolume { volume: Entity },
    /// A hit named a volume the table does not carry — a bind gap, never a defaultable factor.
    UnknownVolume { volume: Entity },
    /// The corridor itself is malformed (non-unit axis, non-finite length, a hit behind the origin).
    BadCorridor { sample: usize, reason: &'static str },
    /// An exit face for a shell the walk is not inside: a dropped entry, an inverted winding, or
    /// an undeclared start-inside. NOT repaired by inventing an interval from the corridor origin.
    UnexpectedExit {
        sample: usize,
        key: ShellKey,
        t: f64,
        triangles: Vec<u32>,
    },
    /// An entry face for a shell the walk is already inside — a self-overlapping island, which
    /// means the mesh is not the closed positively-oriented shell the bake gate promises.
    UnexpectedEntry {
        sample: usize,
        key: ShellKey,
        t: f64,
        triangles: Vec<u32>,
    },
    /// The corridor ran out with material still open. The caller must extend the corridor until the
    /// crossing closes (§13.4's atomic resolution corridor); the core will not synthesize the exit.
    IncompleteCorridor {
        sample: usize,
        open: Vec<ShellKey>,
        length: f32,
    },
    /// Two distinct certified contacts of ONE shell whose EXACT ray parameters are bit-equal.
    ///
    /// Not the sub-f32 chord — that one is ordinary material, ordered and charged on the f64
    /// parameter like any other traversal. This is the residue: two features of one shell the ray
    /// meets at one indistinguishable parameter, which is tangent grade and measure zero. There is no
    /// order between them and no thickness to charge, and reading the pair as a zero-length touch
    /// would be a silent free crossing of whatever they actually bound — the defect class §13.1
    /// exists to kill. So it is named instead.
    UnrepresentableChord {
        sample: usize,
        key: ShellKey,
        t: f64,
    },
    /// A stretch of positive union factor that no shell's presence interval covers.
    ///
    /// The field and the intervals are two readings of one walk, so they cannot disagree — and the
    /// disagreement is not repairable, because a run with no geometry behind it has no entrance
    /// surface, no owning body and no exit to spall from. Attributing it to a stand-in entity is
    /// how a numerical artefact spends a round's capability and marks a penetration.
    UnattributedRun { sample: usize, start: f64, end: f64 },
    /// The spatial collector could not probe a candidate it was handed. Never softened into "that
    /// volume contributed nothing" — an unprobed volume and an absent one are the same silence.
    CollectorFailed {
        volume: Entity,
        reason: &'static str,
    },
    /// One corridor produced more face crossings than the collector will hold. A ceiling, not a
    /// budget: the alternative is a truncated hit list, which is unpairable topology wearing the
    /// costume of valid geometry.
    CorridorOverflow {
        volume: Entity,
        collected: usize,
        limit: usize,
    },
    /// The corridor handed to [`finish`] is not the one [`begin`] asked for. Accepting it silently
    /// would resolve one contact's geometry against another contact's entrance decision — an
    /// overmatch verdict, a normalization bend and a ricochet test all belonging to a surface the
    /// round never met.
    CorridorMismatch { reason: Mismatch },
    /// The covered samples' entry normals cancelled, so no aggregate `n̄` exists.
    ///
    /// Defensive: a face is an ENTRY only when `axis · n < 0`, so every contributing normal leans
    /// against the axis and their sum cannot vanish — `n̄` is well-defined by construction, which is
    /// precisely §13.5's repair of the point model's degenerate normal.
    DegenerateEntryNormal { coverage: f32 },
    /// A fresh start's membership could not be established against a candidate volume: its shape is
    /// neither a certified trimesh nor convex, so no containment probe can be trusted on it, or the
    /// probe through the candidates' bounds would exceed the corridor ceiling. The start is never
    /// declared outside instead.
    UncertifiedStart {
        volume: Entity,
        reason: &'static str,
    },
    /// A mechanism asked the crossing driver for a control it does not carry out (a redirect or a
    /// schedule). Never approximated by stopping or continuing.
    UnsupportedControl,
    /// The driver's step ran out of a limit: more crossings than `resolve::MAX_STEP_CROSSINGS`, or
    /// travel more than `resolve::MAX_OVERRUN` past where the step legitimately ends
    /// (`resolve::Budget::reach`). Each crossing was bounded; their succession was not, and a step
    /// that never yields applies consequences ever further downrange. `crossings` counts the
    /// refused one; `overrun` is metres past that end it would have reached.
    DrainExhausted { crossings: usize, overrun: f32 },
}

impl WalkError {
    /// The stable name of the invariant this error breaks: one per variant, and one per
    /// [`Mismatch`] for a corridor mismatch. It is the key the runtime's safe stop logs, so reports
    /// from real matches group by it; a name is never reworded once shipped. A variant carrying a
    /// `reason` names the variant, and its reason travels beside it as context.
    pub fn kind(&self) -> &'static str {
        match self {
            WalkError::BadFactor { .. } => "bad_factor",
            WalkError::SubstanceFactorConflict { .. } => "substance_factor_conflict",
            WalkError::RepeatedVolume { .. } => "repeated_volume",
            WalkError::UnknownVolume { .. } => "unknown_volume",
            WalkError::BadCorridor { .. } => "bad_corridor",
            WalkError::UnexpectedExit { .. } => "unexpected_exit",
            WalkError::UnexpectedEntry { .. } => "unexpected_entry",
            WalkError::IncompleteCorridor { .. } => "incomplete_corridor",
            WalkError::UnrepresentableChord { .. } => "unrepresentable_chord",
            WalkError::UnattributedRun { .. } => "unattributed_run",
            WalkError::CollectorFailed { .. } => "collector_failed",
            WalkError::CorridorOverflow { .. } => "corridor_overflow",
            WalkError::CorridorMismatch { reason } => reason.kind(),
            WalkError::DegenerateEntryNormal { .. } => "degenerate_entry_normal",
            WalkError::UncertifiedStart { .. } => "uncertified_start",
            WalkError::UnsupportedControl => "unsupported_control",
            WalkError::DrainExhausted { .. } => "drain_exhausted",
        }
    }
}

/// How the corridor handed to [`finish`] departs from the one [`begin`] asked for
/// ([`WalkError::CorridorMismatch`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// Collected along a different axis than the bend asked for.
    Axis,
    /// Collected against a different world anchor.
    Anchor,
    /// Starts somewhere other than the axis sample's hand-off.
    Origin,
    /// The ring frame was re-derived instead of transported.
    Frame,
    /// Samples a different disc radius.
    Radius,
    /// Samples a different number of rays than the entrance disc.
    SampleCount,
    /// A sample ray resumes somewhere other than where the hand-off put it.
    SampleOffset,
    /// No crossing begins at the hand-off.
    NotAtHandOff,
    /// The crossing at the hand-off is neither made of the entrance's geometry nor on its surface.
    NotTheEntrance,
}

impl Mismatch {
    /// The stable name [`WalkError::kind`] reports for this mismatch.
    pub fn kind(self) -> &'static str {
        match self {
            Mismatch::Axis => "corridor_mismatch.axis",
            Mismatch::Anchor => "corridor_mismatch.anchor",
            Mismatch::Origin => "corridor_mismatch.origin",
            Mismatch::Frame => "corridor_mismatch.frame",
            Mismatch::Radius => "corridor_mismatch.radius",
            Mismatch::SampleCount => "corridor_mismatch.sample_count",
            Mismatch::SampleOffset => "corridor_mismatch.sample_offset",
            Mismatch::NotAtHandOff => "corridor_mismatch.not_at_hand_off",
            Mismatch::NotTheEntrance => "corridor_mismatch.not_the_entrance",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Single-ray output
// ---------------------------------------------------------------------------------------------

/// One maximal stretch of CONSTANT union factor. The canonical cost representation.
///
/// Spans are cut only where `max(factor)` actually changes — never where the presence *set* changes
/// underneath an unchanged maximum. That is what makes seam invisibility (§13.6) bit-exact rather
/// than merely approximate: `(b−a)·F` is not bit-equal to `(s−a)·F + (b−s)·F`, so a plate split at
/// `s` would otherwise cost differently from one thick plate, and a low-factor volume buried inside
/// steel would change the cost bits purely by subdividing the interval.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub factor: f32,
}

impl Span {
    fn len(&self) -> f64 {
        self.end - self.start
    }

    fn cost(&self) -> f64 {
        self.len() * self.factor as f64
    }
}

/// One constant-factor stretch of MATERIAL inside a welded run, with any welded voids removed.
///
/// `start`/`end` are the outer extent (they may straddle a deleted micro-gap); `material` is the
/// metres actually charged. Welding deletes faces, never creates steel (§13.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSegment {
    pub start: f64,
    pub end: f64,
    pub factor: f32,
    pub material: f64,
    pub cost: f32,
}

/// A contiguous crossing after ε-welding: one entrance, one exit, and the material steps between.
#[derive(Clone, Debug, PartialEq)]
pub struct WeldedRun {
    pub start: f64,
    pub end: f64,
    pub cost: f32,
    /// Outward normal of the outermost entry face — the surface the entrance laws read.
    pub entry_normal: Vec3,
    /// Outward normal of the innermost exit face.
    pub exit_normal: Vec3,
    pub segments: Vec<MaterialSegment>,
    /// The volume whose primitive OPENS the run — the entrance surface, physically the first thing
    /// touched. §13.5's ratified impulse rule routes the whole momentum exchange to its rigid body
    /// (2026-08-07): applying `m·Δv` to every present body would duplicate momentum, and picking by
    /// factor would reintroduce the ownership tie-break §13.2 abolished. Ties at an exact abutment
    /// are broken by the lowest entity, which is arbitrary but deterministic — at an abutment either
    /// face IS the entrance surface.
    pub entry_volume: Entity,
    /// How many micro-gaps this run swallowed.
    pub joints: u32,
    /// Everything present anywhere in the run — the "shares a primitive" half of disc event
    /// association.
    pub primitives: Vec<PrimitiveKey>,
}

/// Where the field steps, and what the step means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryKind {
    /// Air → material at the outermost face: incidence, ricochet, normalization and overmatch read
    /// here, exactly once per run (§13.4).
    Entrance,
    /// A material → material step inside a run.
    Step,
    /// Material → air at the innermost face.
    Exit,
}

/// One field transition along a sample ray. Spall sources are not read from these: they are body
/// exits ([`BodyExit`]), found from the sample's substance layers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundaryEvent {
    pub kind: BoundaryKind,
    pub t: f64,
    pub position: Vec3,
    pub normal: Vec3,
    pub factor_before: f32,
    pub factor_after: f32,
    /// True when this step only exists because a micro-gap was welded away.
    pub welded: bool,
    pub run: usize,
}

/// What one volume of the tank presented to one sample ray.
///
/// Presence is the UNION of that entity's primitive intervals — never their sum. Two interpenetrating
/// islands of one mesh must deposit once, or a shell rewards sloppy modelling with double damage.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityPresence {
    pub entity: Entity,
    pub factor: f32,
    pub chord: f32,
    pub cost: f32,
    pub spans: Vec<(f64, f64)>,
}

/// One volume present over a [`Layer`]: its identity (the layer a dose is addressed to) and its
/// material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presence {
    pub volume: Entity,
    pub material: VolumeMaterial,
}

/// One stretch of a sample ray over which the set of present volumes is constant — an ORDERED
/// ENCOUNTER, cut wherever any volume's presence opens or closes, with nothing reduced: an overlap
/// of two substances lists both. A gap (air between plates, a welded void) is a layer with nothing
/// present. A sample's layers tile its corridor `[0, length)` in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub start: f64,
    pub end: f64,
    /// In volume order.
    pub present: Vec<Presence>,
}

impl Layer {
    /// The substances present, in id order, each once.
    pub fn substances(&self) -> Vec<SubstanceId> {
        let mut out: Vec<SubstanceId> = self
            .present
            .iter()
            .map(|presence| presence.material.substance)
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// One shell's presence intervals along a sample ray.
///
/// Kept alongside the per-ENTITY union because the staged handoff needs surface identity: after
/// normalization bends the axis and transports the ring, a transit sample can begin INSIDE material,
/// and [`RayCorridor::initial_presence`] must be told which shells — inference is forbidden (that is
/// how a dropped entry face becomes free armour).
#[derive(Clone, Debug, PartialEq)]
pub struct ShellPresence {
    pub key: ShellKey,
    /// Disjoint, ordered, half-open `[open, close)`. A span the ray BEGAN inside (declared
    /// [`RayCorridor::initial_presence`]) opens at `−∞`: its entry was owned by the seed that
    /// declared it, behind every hand-off this walk can make, so [`RayWalk::inside_at`] hands it on
    /// even at `t = 0`.
    pub spans: Vec<(f64, f64)>,
}

/// One sample ray's resolved walk.
#[derive(Clone, Debug, PartialEq)]
pub struct RayWalk {
    /// Canonical constant-factor spans covering `[0, length)`, air included.
    pub spans: Vec<Span>,
    pub runs: Vec<WeldedRun>,
    /// `∫ max(factor) dt`, accumulated in f64 over the canonical spans and cast ONCE.
    pub cost: f32,
    pub presence: Vec<EntityPresence>,
    /// Per-shell presence — the seed state the staged handoff exports.
    pub shells: Vec<ShellPresence>,
    pub events: Vec<BoundaryEvent>,
    /// The ordered encounters along this sample ([`Layer`]).
    pub layers: Vec<Layer>,
}

impl RayWalk {
    /// Which shells a corridor RESTARTED at progress `t` would have to declare as initial
    /// presence.
    ///
    /// The interval is `(open, close]` — the entry face must be STRICTLY behind the restart. A face
    /// sitting at the restart arrives in the new corridor's own hit list at `t = 0` and is
    /// processed there, so seeding it as well would double-count the entry
    /// ([`WalkError::UnexpectedEntry`]). An exit sitting at the restart, conversely, must be seeded
    /// or it has nothing to pair with.
    ///
    /// "AT the restart" is a TOLERANCE, not an equality, and that is the whole reason this takes
    /// laws. A hand-off on a face reads its seed at exactly that face's `t`, and the transit
    /// corridor re-measures the face along the bend at `t ≈ 0`; but a second boundary beside that
    /// face, or any boundary beside a restart read off no boundary at all (a perforation's resume
    /// point, a hand-off in air), agrees with the restart only to within rounding. Read as an exact
    /// comparison, that hair decides between "the seed owns this entry" and "the corridor does", and
    /// getting it wrong is `UnexpectedEntry` and a round stopped dead on a plate it should have
    /// crossed. [`coincident`] is the module's existing answer to "do these two `t` name one
    /// boundary", so it is the one used here.
    pub fn inside_at(&self, t: f64, laws: &WalkLaws) -> Vec<ShellKey> {
        self.shells
            .iter()
            .filter(|presence| {
                presence.spans.iter().any(|(open, close)| {
                    // A declared span (`open = −∞`) was entered before any `t`; `coincident`
                    // would read `∞ ≤ ∞` as the same boundary.
                    let entered =
                        *open == f64::NEG_INFINITY || (*open < t && !coincident(*open, t, laws));
                    let still_inside = t <= *close || coincident(*close, t, laws);
                    entered && still_inside
                })
            })
            .map(|presence| presence.key)
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Stage 1 — topology reduction and the union field
// ---------------------------------------------------------------------------------------------

/// Whether two `t` values name the same topological boundary. Scale-aware, because f32 spacing
/// coarsens with distance and a face diagonal at 400 m must still reduce to one crossing.
pub(crate) fn coincident(anchor: f64, t: f64, laws: &WalkLaws) -> bool {
    (t - anchor).abs() <= topology_band(anchor.abs().max(t.abs()), laws)
}

/// The width of "the same boundary" for parameters of magnitude `scale` (m) — [`coincident`]'s
/// tolerance, exposed so the collector's hand-off clamp (`collect::sitting_reach`) is stated in the
/// same band [`RayWalk::inside_at`] judges the seed with.
pub(crate) fn topology_band(scale: f64, laws: &WalkLaws) -> f64 {
    laws.topology_abs as f64 + laws.topology_rel as f64 * scale
}

/// One certified crossing of one shell: where the ray goes in or out, and which faces said so.
struct Crossing {
    key: ShellKey,
    contact: Contact,
    t: f64,
    /// `+1` inward, `−1` outward.
    sign: i32,
    /// Sum of the contributing faces' outward normals — the boundary aggregate, unnormalized so a
    /// fan's several faces average rather than the last one winning.
    normal: Vec3,
    triangles: Vec<u32>,
}

/// Canonicalize the raw face claims into the crossings they name.
///
/// ONE CONTACT OF ONE SHELL IS ONE CROSSING, and nothing else is. `collect::cross_triangle`
/// deliberately lets every triangle incident on a welded feature claim a ray running through it — a
/// duplicate is recoverable and a dropped crossing is not — and it names the feature exactly, so the
/// fan arrives already identified. There is no window here and no proximity test: two claims are one
/// crossing when they name one contact, and two contacts are two crossings however close their `t`.
///
/// A FAN THAT DISAGREES ABOUT DIRECTION IS A TANGENT. The ray meets the feature and leaves on the
/// same side, so it bounds no material and crosses no surface: no interval, no cost, no event. A fan
/// that agrees is transverse and emits exactly one oriented crossing. That is the whole rule, and it
/// needs neither a normal tolerance nor a minimum thickness — a shell thinner than any window is two
/// DIFFERENT contacts and stays two crossings.
fn canonical_crossings(order: &[&FaceHit]) -> Vec<Crossing> {
    struct Fan {
        t: f64,
        sign: i32,
        normal: Vec3,
        triangles: Vec<u32>,
        mixed: bool,
    }
    let mut fans: BTreeMap<(ShellKey, Contact), Fan> = BTreeMap::new();
    for hit in order {
        // EXACT ZERO IS THE ONLY TANGENT, and the collector has already refused it: a face whose
        // plane contains the ray projects to a degenerate outline and claims nothing. So every hit
        // has a direction, read from the collector's exact predicate rather than re-derived from
        // `true_normal`, with no angular threshold deciding what counts as a face. A fan whose
        // claims disagree is a tangent brush, dropped below.
        let sign = if hit.entering { 1 } else { -1 };
        match fans.entry((hit.key(), hit.contact)) {
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let fan = slot.get_mut();
                fan.mixed |= fan.sign != sign;
                fan.normal += hit.true_normal;
                fan.triangles.push(hit.triangle);
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(Fan {
                    t: hit.t,
                    sign,
                    normal: hit.true_normal,
                    triangles: vec![hit.triangle],
                    mixed: false,
                });
            }
        }
    }
    fans.into_iter()
        .filter(|(_, fan)| !fan.mixed)
        .map(|((key, contact), fan)| Crossing {
            key,
            contact,
            t: fan.t,
            sign: fan.sign,
            normal: fan.normal,
            triangles: fan.triangles,
        })
        .collect()
}

/// The live union field: which shells are open, and therefore what `max(factor)` is.
///
/// Openness is a BOOLEAN, not a depth. A closed, outward-wound, EMBEDDED shell crossed transversely
/// at certified contacts has crossings that alternate in from out; a shell that passes through
/// itself does not, and reading the difference as depth is how a real branch gets erased. So the
/// alternation is required rather than assumed, and the first crossing that disproves it is named
/// ([`WalkError::UnexpectedEntry`]) instead of counted.
///
/// The bake certifies embedding over the surface (`bake::embedding`), so alternation is a
/// redundant runtime backstop rather than the only guard: a shell that could disprove it never
/// reaches the world. It is kept fail-closed because a live collider is not the buffer the bake
/// judged — a pose, a scale or a collider swap can still put a ray somewhere the certificate does
/// not cover.
struct Field {
    /// Per open shell: where its presence opened.
    open: BTreeMap<ShellKey, f64>,
    /// Per entity with a shell open: how many. An entity whose count reaches zero is removed, so
    /// the field's maximum reads only what is open.
    per_entity: BTreeMap<Entity, u32>,
}

impl Field {
    fn max_factor(&self, volumes: &VolumeTable) -> Result<f32, WalkError> {
        let mut max = 0.0f32;
        for (&entity, &count) in &self.per_entity {
            if count > 0 {
                max = max.max(volumes.factor(entity)?);
            }
        }
        Ok(max)
    }
}

/// One step of the union field's maximum factor, at a batch of crossings sharing one exact `t`:
/// the maximum before and after the batch, and the summed normals of its entries and of its exits
/// (`None` where the batch held none).
struct Transition {
    t: f64,
    before: f32,
    after: f32,
    entry_normal: Option<Vec3>,
    exit_normal: Option<Vec3>,
}

/// Walk one sample ray: reduce topology, build presence, integrate the union field, ε-weld the runs
/// and emit the boundary events.
pub fn walk_ray(
    sample: usize,
    corridor: &RayCorridor,
    volumes: &VolumeTable,
    laws: &WalkLaws,
) -> Result<RayWalk, WalkError> {
    if !corridor.length.is_finite() || corridor.length < 0.0 {
        return Err(WalkError::BadCorridor {
            sample,
            reason: "corridor length must be finite and non-negative",
        });
    }
    if (corridor.axis.length_squared() - 1.0).abs() > 1.0e-3 {
        return Err(WalkError::BadCorridor {
            sample,
            reason: "corridor axis must be unit length",
        });
    }

    // Deterministic total order. Ties on `t` are BATCHED (below), not broken — neither entry-first
    // nor exit-first is correct at an exact abutment, since either invents a transient air region or
    // a transient overlap and with it a phantom factor step (§13.6 seam invisibility).
    let mut order: Vec<&FaceHit> = corridor
        .hits
        .iter()
        .filter(|hit| hit.t < corridor.length as f64)
        .collect();
    if order.iter().any(|hit| !hit.t.is_finite() || hit.t < 0.0) {
        return Err(WalkError::BadCorridor {
            sample,
            reason: "a face hit is behind the corridor origin or non-finite",
        });
    }
    order.sort_by(|a, b| {
        a.t.total_cmp(&b.t)
            .then(a.volume.cmp(&b.volume))
            .then(a.primitive.cmp(&b.primitive))
            .then(a.shell.cmp(&b.shell))
            .then(a.triangle.cmp(&b.triangle))
    });

    // PAIRING IS PER SHELL, AND ONLY PER SHELL. Every claim already names the surface and the
    // welded feature it is about, so the reduction below never has to decide whether two nearby
    // faces are one surface or two — that is a fact by the time it arrives.
    //
    // ORDER COMES FROM THE EXACT PARAMETER. Each contact's `t` is the collector's f64 projection, so
    // two contacts a fraction of an f32 ULP apart still sort the way the ray actually meets them.
    // Ordering the same pair on their rounded f32 values reverses them about half the time, and a
    // reversed pair is an exit before its own entry: a structured error over geometry that is
    // perfectly ordinary. An exit that STILL precedes its entry after this sort is a genuine
    // alternation violation, and stays one.
    let mut crossings = canonical_crossings(&order);
    crossings.sort_by(|a, b| {
        a.t.total_cmp(&b.t)
            .then(a.key.cmp(&b.key))
            .then(a.contact.cmp(&b.contact))
    });

    let (intervals, transitions) = integrate_field(sample, corridor, &crossings, volumes)?;
    let (spans, cost) = canonical_spans(corridor, volumes, &transitions)?;
    let raw_runs = raw_runs(&spans);

    // Boundary normals, looked up by the transition that opens/closes each run.
    let normal_at = |t: f64, entry: bool| -> Vec3 {
        transitions
            .iter()
            .filter(|transition| transition.t == t)
            .find_map(|transition| {
                if entry {
                    transition.entry_normal
                } else {
                    transition.exit_normal
                }
            })
            .map(unit_or_zero)
            // A run that opens at the corridor origin (declared initial presence) has no entry face
            // in this corridor; the incoming axis is the honest stand-in — the same fallback the
            // serial resolver uses for a degenerate normal.
            .unwrap_or(if entry { -corridor.axis } else { corridor.axis })
    };

    let runs = weld_runs(
        sample, corridor, &spans, &raw_runs, &intervals, &normal_at, laws,
    )?;
    let events = boundary_events(corridor, &runs);
    let presence = entity_presence(&intervals, volumes)?;
    let layers = layers(corridor.length as f64, &presence, volumes)?;

    let mut by_shell: BTreeMap<ShellKey, Vec<(f64, f64)>> = BTreeMap::new();
    for (key, open, close) in &intervals {
        by_shell.entry(*key).or_default().push((*open, *close));
    }
    let shells = by_shell
        .into_iter()
        .map(|(key, mut spans)| {
            spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            // The field opened a declared shell at the corridor origin, which is where its CHORD
            // starts; its ENTRY is the seed's, and lies behind the origin.
            if corridor.initial_presence.contains(&key)
                && let Some(first) = spans.first_mut()
            {
                first.0 = f64::NEG_INFINITY;
            }
            ShellPresence { key, spans }
        })
        .collect();

    Ok(RayWalk {
        spans,
        runs,
        cost,
        presence,
        shells,
        events,
        layers,
    })
}

/// Integrate the union field along `crossings` (sorted by exact `t`), opened on the corridor's
/// declared initial presence: the per-shell presence intervals, in close order, and the
/// transitions where the field's maximum factor steps. Every shell the field opens closes inside
/// the corridor, or the walk fails.
fn integrate_field(
    sample: usize,
    corridor: &RayCorridor,
    crossings: &[Crossing],
    volumes: &VolumeTable,
) -> Result<(Vec<(ShellKey, f64, f64)>, Vec<Transition>), WalkError> {
    let mut field = Field {
        open: BTreeMap::new(),
        per_entity: BTreeMap::new(),
    };
    for key in &corridor.initial_presence {
        if field.open.insert(*key, 0.0).is_none() {
            *field.per_entity.entry(key.volume).or_insert(0) += 1;
        }
        volumes.factor(key.volume)?;
    }

    // Per-shell presence intervals, in close order.
    let mut intervals: Vec<(ShellKey, f64, f64)> = Vec::new();
    let mut transitions: Vec<Transition> = Vec::new();

    let mut i = 0usize;
    while i < crossings.len() {
        // ONE BIT-EQUAL EXACT `t` IS ONE ATOMIC BATCH. Crossings that share the exact parameter have
        // no order, so the field is read once, every crossing is applied, and the field is read
        // again. That is what keeps seam invisibility bit-exact at an exact abutment: the outgoing
        // plate's exit and the incoming plate's entry cancel inside one batch, no transition is
        // emitted, and one span covers both plates (§13.6).
        let t = crossings[i].t;
        let mut j = i + 1;
        while j < crossings.len() && crossings[j].t.to_bits() == t.to_bits() {
            j += 1;
        }
        let batch = &crossings[i..j];
        i = j;

        // ONE SHELL MAY NOT APPEAR TWICE IN ONE BATCH. A batch is deliberately unordered, so two
        // contacts of one shell inside it have no order and no thickness between them — tangent
        // grade, and measure zero now that ordinary sub-f32 chords are carried exactly. Applying
        // both would charge whatever they bound as nothing.
        let mut seen: BTreeSet<ShellKey> = BTreeSet::new();
        for crossing in batch {
            if !seen.insert(crossing.key) {
                return Err(WalkError::UnrepresentableChord {
                    sample,
                    key: crossing.key,
                    t,
                });
            }
        }

        let before = field.max_factor(volumes)?;
        let mut entry_normal = Vec3::ZERO;
        let mut exit_normal = Vec3::ZERO;
        let mut any_entry = false;
        let mut any_exit = false;

        for crossing in batch {
            let key = crossing.key;
            if crossing.sign > 0 {
                volumes.factor(key.volume)?;
                // A CERTIFIED SHELL ALTERNATES. Entering one the ray is already inside is a broken
                // certificate — a self-intersecting shell, or a crossing the collector invented —
                // and it is refused rather than counted as depth.
                if field.open.contains_key(&key) {
                    return Err(WalkError::UnexpectedEntry {
                        sample,
                        key,
                        t,
                        triangles: crossing.triangles.clone(),
                    });
                }
                field.open.insert(key, t);
                *field.per_entity.entry(key.volume).or_insert(0) += 1;
                entry_normal += crossing.normal;
                any_entry = true;
            } else {
                // An exit with no shell open is a dropped entry, an inverted winding or a hole in
                // the mesh — never repaired by inventing an interval from the corridor origin.
                let Some(open_at) = field.open.remove(&key) else {
                    return Err(WalkError::UnexpectedExit {
                        sample,
                        key,
                        t,
                        triangles: crossing.triangles.clone(),
                    });
                };
                if let Some(count) = field.per_entity.get_mut(&key.volume) {
                    *count -= 1;
                    if *count == 0 {
                        field.per_entity.remove(&key.volume);
                    }
                }
                intervals.push((key, open_at, t));
                exit_normal += crossing.normal;
                any_exit = true;
            }
        }

        let after = field.max_factor(volumes)?;
        if before.to_bits() != after.to_bits() {
            transitions.push(Transition {
                t,
                before,
                after,
                entry_normal: any_entry.then_some(entry_normal),
                exit_normal: any_exit.then_some(exit_normal),
            });
        }
    }

    if !field.open.is_empty() {
        return Err(WalkError::IncompleteCorridor {
            sample,
            open: field.open.keys().copied().collect(),
            length: corridor.length,
        });
    }

    Ok((intervals, transitions))
}

/// The canonical spans over `[0, length)`, and their summed cost. Cut ONLY at factor changes, so
/// equal-factor abutment and one thick plate produce the identical span list and therefore the
/// identical cost bits.
fn canonical_spans(
    corridor: &RayCorridor,
    volumes: &VolumeTable,
    transitions: &[Transition],
) -> Result<(Vec<Span>, f32), WalkError> {
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor = 0.0f64;
    let mut factor = if transitions.is_empty() {
        // No steps at all: either empty air, or the corridor lies wholly inside declared presence.
        field_start_factor(corridor, volumes)?
    } else {
        transitions[0].before
    };
    for transition in transitions {
        if transition.t > cursor {
            spans.push(Span {
                start: cursor,
                end: transition.t,
                factor,
            });
            cursor = transition.t;
        }
        factor = transition.after;
    }
    if corridor.length as f64 > cursor {
        spans.push(Span {
            start: cursor,
            end: corridor.length as f64,
            factor,
        });
    }

    let mut cost_acc = 0.0f64;
    for span in &spans {
        if span.factor > 0.0 {
            cost_acc += span.cost();
        }
    }
    let cost = cost_acc as f32;
    Ok((spans, cost))
}

/// The runs of `spans`: maximal stretches of factor > 0, as index ranges into `spans`.
fn raw_runs(spans: &[Span]) -> Vec<(usize, usize)> {
    let mut raw_runs: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    for (index, span) in spans.iter().enumerate() {
        if span.factor > 0.0 {
            open.get_or_insert(index);
        } else if let Some(start) = open.take() {
            raw_runs.push((start, index));
        }
    }
    if let Some(start) = open.take() {
        raw_runs.push((start, spans.len()));
    }
    raw_runs
}

/// The ordered encounters over `[0, length)`: cut at every presence boundary, each listing the
/// volumes whose (unioned) presence covers it.
fn layers(
    length: f64,
    presence: &[EntityPresence],
    volumes: &VolumeTable,
) -> Result<Vec<Layer>, WalkError> {
    let mut cuts: Vec<f64> = vec![0.0, length];
    for entity in presence {
        for (open, close) in &entity.spans {
            cuts.push(*open);
            cuts.push(*close);
        }
    }
    cuts.retain(|t| (0.0..=length).contains(t));
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let materials = presence
        .iter()
        .map(|entity| volumes.material(entity.entity))
        .collect::<Result<Vec<_>, _>>()?;
    // A sweep over the span ends in order. Every span end inside the corridor is a cut, so a span
    // covers the window opening at `start` exactly when `open <= start < close`.
    let mut opens: Vec<(f64, usize)> = Vec::new();
    let mut closes: Vec<(f64, usize)> = Vec::new();
    for (index, entity) in presence.iter().enumerate() {
        for (open, close) in &entity.spans {
            opens.push((*open, index));
            closes.push((*close, index));
        }
    }
    opens.sort_by(|a, b| a.0.total_cmp(&b.0));
    closes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut next_open, mut next_close) = (0, 0);
    // Per presence index (volume order), how many of its spans cover the window.
    let mut active: BTreeMap<usize, u32> = BTreeMap::new();
    let mut out = Vec::with_capacity(cuts.len());
    for window in cuts.windows(2) {
        let (start, end) = (window[0], window[1]);
        while next_open < opens.len() && opens[next_open].0 <= start {
            *active.entry(opens[next_open].1).or_insert(0) += 1;
            next_open += 1;
        }
        while next_close < closes.len() && closes[next_close].0 <= start {
            let index = closes[next_close].1;
            if let Some(count) = active.get_mut(&index) {
                *count -= 1;
                if *count == 0 {
                    active.remove(&index);
                }
            }
            next_close += 1;
        }
        out.push(Layer {
            start,
            end,
            present: active
                .keys()
                .map(|&index| Presence {
                    volume: presence[index].entity,
                    material: materials[index],
                })
                .collect(),
        });
    }
    Ok(out)
}

/// The union factor at `t = 0`, from declared initial presence alone.
fn field_start_factor(corridor: &RayCorridor, volumes: &VolumeTable) -> Result<f32, WalkError> {
    let mut max = 0.0f32;
    for key in &corridor.initial_presence {
        max = max.max(volumes.factor(key.volume)?);
    }
    Ok(max)
}

fn unit_or_zero(v: Vec3) -> Vec3 {
    let len = v.length();
    if len > 1.0e-6 { v / len } else { Vec3::ZERO }
}

/// ε-weld the raw runs (§13.4) and build each welded run's material segments.
fn weld_runs(
    sample: usize,
    corridor: &RayCorridor,
    spans: &[Span],
    raw_runs: &[(usize, usize)],
    intervals: &[(ShellKey, f64, f64)],
    normal_at: &dyn Fn(f64, bool) -> Vec3,
    laws: &WalkLaws,
) -> Result<Vec<WeldedRun>, WalkError> {
    let mut welded: Vec<WeldedRun> = Vec::new();
    let mut group_start = 0usize;
    let mut gap_budget = 0.0f64;

    let flush = |from: usize, to: usize| -> Result<WeldedRun, WalkError> {
        let start = spans[raw_runs[from].0].start;
        let end = spans[raw_runs[to].1 - 1].end;
        let mut segments: Vec<MaterialSegment> = Vec::new();
        let mut cost_acc = 0.0f64;
        for run in &raw_runs[from..=to] {
            for span in &spans[run.0..run.1] {
                cost_acc += span.cost();
                match segments.last_mut() {
                    // Coalesce across a welded void so an equal-factor sandwich reads as ONE
                    // stretch of the field.
                    Some(last) if last.factor.to_bits() == span.factor.to_bits() => {
                        last.end = span.end;
                        last.material += span.end - span.start;
                        last.cost = (last.cost as f64 + span.cost()) as f32;
                    }
                    _ => segments.push(MaterialSegment {
                        start: span.start,
                        end: span.end,
                        factor: span.factor,
                        material: span.end - span.start,
                        cost: span.cost() as f32,
                    }),
                }
            }
        }
        let mut primitives: Vec<PrimitiveKey> = intervals
            .iter()
            .filter(|(_, open, close)| *close > start && *open < end)
            .map(|(key, _, _)| key.primitive_key())
            .collect();
        primitives.sort();
        primitives.dedup();
        // A run IS its geometry: positive union factor means some shell is open there, and an open
        // shell has an interval covering it. The two readings cannot disagree, and a run with no
        // geometry behind it is not attributable to anything, so it is refused rather than handed a
        // stand-in entity to spend a round's capability on.
        let entry_volume = intervals
            .iter()
            .filter(|(_, open, _)| *open == start)
            .map(|(key, _, _)| key.volume)
            .min()
            .or_else(|| primitives.first().map(|key| key.volume))
            .ok_or(WalkError::UnattributedRun { sample, start, end })?;
        Ok(WeldedRun {
            start,
            end,
            cost: cost_acc as f32,
            entry_normal: normal_at(start, true),
            exit_normal: normal_at(end, false),
            entry_volume,
            segments,
            joints: (to - from) as u32,
            primitives,
        })
    };

    for index in 0..raw_runs.len() {
        if index + 1 == raw_runs.len() {
            welded.push(flush(group_start, index)?);
            break;
        }
        let this_end = spans[raw_runs[index].1 - 1].end;
        let next_start = spans[raw_runs[index + 1].0].start;
        let gap_along = next_start - this_end;
        let n_exit = normal_at(this_end, false);
        let n_entry = normal_at(next_start, true);

        // Perpendicular, not along the ray (§13.4) — else grazing incidence un-welds exactly when it
        // matters most. Both faces are consulted and the LARGER projection wins: for the parallel
        // plates the rule is written for they agree, and where they disagree the conservative read
        // welds less.
        let projection = corridor
            .axis
            .dot(n_exit)
            .abs()
            .max(corridor.axis.dot(n_entry).abs());
        let gap_perp = gap_along * projection as f64;
        let opposing = n_exit.dot(n_entry) <= -laws.weld_face_cos;
        let weld = opposing
            && gap_along <= laws.weld_max_lookahead as f64
            && gap_perp <= laws.weld_perp as f64
            && gap_budget + gap_perp <= laws.weld_run_gap_budget as f64;

        if weld {
            gap_budget += gap_perp;
        } else {
            welded.push(flush(group_start, index)?);
            group_start = index + 1;
            gap_budget = 0.0;
        }
    }

    Ok(welded)
}

/// Turn each welded run's material segments into its ordered field transitions: one entrance, every
/// factor step inside, one exit. Welding deletes only a void's exit/entry face pair; the material
/// step it exposes is still a step, and a void has no event of its own.
fn boundary_events(corridor: &RayCorridor, runs: &[WeldedRun]) -> Vec<BoundaryEvent> {
    let mut events = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let at = |t: f64| corridor.origin + corridor.axis * t as f32;
        let Some(first) = run.segments.first() else {
            continue;
        };
        events.push(BoundaryEvent {
            kind: BoundaryKind::Entrance,
            t: run.start,
            position: at(run.start),
            normal: run.entry_normal,
            factor_before: 0.0,
            factor_after: first.factor,
            welded: false,
            run: index,
        });
        for pair in run.segments.windows(2) {
            let (before, after) = (pair[0], pair[1]);
            events.push(BoundaryEvent {
                kind: BoundaryKind::Step,
                t: before.end,
                position: at(before.end),
                normal: corridor.axis,
                factor_before: before.factor,
                factor_after: after.factor,
                welded: after.start != before.end,
                run: index,
            });
        }
        let last = run.segments[run.segments.len() - 1];
        events.push(BoundaryEvent {
            kind: BoundaryKind::Exit,
            t: run.end,
            position: at(run.end),
            normal: run.exit_normal,
            factor_before: last.factor,
            factor_after: 0.0,
            welded: false,
            run: index,
        });
    }
    events
}

/// Union each entity's primitive intervals, then read its own chord at its own factor: what the ray
/// presents of it, a readout. The dose is the mechanism's ([`super::mechanism::SolidShot`], §13.2).
fn entity_presence(
    intervals: &[(ShellKey, f64, f64)],
    volumes: &VolumeTable,
) -> Result<Vec<EntityPresence>, WalkError> {
    let mut by_entity: BTreeMap<Entity, Vec<(f64, f64)>> = BTreeMap::new();
    for (key, open, close) in intervals {
        by_entity
            .entry(key.volume)
            .or_default()
            .push((*open, *close));
    }
    let mut out = Vec::new();
    for (entity, mut spans) in by_entity {
        spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for span in spans {
            match merged.last_mut() {
                Some(last) if span.0 <= last.1 => last.1 = last.1.max(span.1),
                _ => merged.push(span),
            }
        }
        let factor = volumes.factor(entity)?;
        let chord: f64 = merged.iter().map(|(a, b)| *b - *a).sum();
        out.push(EntityPresence {
            entity,
            factor,
            chord: chord as f32,
            cost: (chord * factor as f64) as f32,
            spans: merged,
        });
    }
    Ok(out)
}

/// One witness's presence along one sample ray (ADR-0040): the union of its shells' intervals,
/// disjoint and ordered. A witness has no factor, so nothing here integrates.
#[derive(Clone, Debug, PartialEq)]
pub struct WitnessSpans {
    pub witness: Entity,
    pub spans: Vec<(f64, f64)>,
}

/// Pair the face claims of witness surfaces along a ray that starts and ends outside every one of
/// them, into each witness's presence intervals.
///
/// The hard walk's pairing law exactly ([`canonical_crossings`], alternation per shell, an exit
/// with nothing open or a shell left open is an error), with no union field: witnesses are never
/// layers, so they neither weld, nor step a factor, nor emit events. `hits` past `length` are not
/// this ray's.
pub fn witness_spans(
    sample: usize,
    length: f64,
    hits: &[FaceHit],
) -> Result<Vec<WitnessSpans>, WalkError> {
    let order: Vec<&FaceHit> = hits.iter().filter(|hit| hit.t < length).collect();
    if order.iter().any(|hit| !hit.t.is_finite() || hit.t < 0.0) {
        return Err(WalkError::BadCorridor {
            sample,
            reason: "a face hit is behind the corridor origin or non-finite",
        });
    }
    let mut crossings = canonical_crossings(&order);
    crossings.sort_by(|a, b| {
        a.t.total_cmp(&b.t)
            .then(a.key.cmp(&b.key))
            .then(a.contact.cmp(&b.contact))
    });
    let mut open: BTreeMap<ShellKey, f64> = BTreeMap::new();
    let mut by_entity: BTreeMap<Entity, Vec<(f64, f64)>> = BTreeMap::new();
    for crossing in &crossings {
        let key = crossing.key;
        if crossing.sign > 0 {
            if open.insert(key, crossing.t).is_some() {
                return Err(WalkError::UnexpectedEntry {
                    sample,
                    key,
                    t: crossing.t,
                    triangles: crossing.triangles.clone(),
                });
            }
        } else {
            let Some(opened) = open.remove(&key) else {
                return Err(WalkError::UnexpectedExit {
                    sample,
                    key,
                    t: crossing.t,
                    triangles: crossing.triangles.clone(),
                });
            };
            by_entity
                .entry(key.volume)
                .or_default()
                .push((opened, crossing.t));
        }
    }
    if !open.is_empty() {
        return Err(WalkError::IncompleteCorridor {
            sample,
            open: open.keys().copied().collect(),
            length: length as f32,
        });
    }
    Ok(by_entity
        .into_iter()
        .map(|(witness, mut spans)| {
            spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let mut merged: Vec<(f64, f64)> = Vec::new();
            for span in spans {
                match merged.last_mut() {
                    Some(last) if span.0 <= last.1 => last.1 = last.1.max(span.1),
                    _ => merged.push(span),
                }
            }
            WitnessSpans {
                witness,
                spans: merged,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Stage 2 — the disc (§13.5)
// ---------------------------------------------------------------------------------------------

/// The orthonormal basis the sample ring is laid out in.
///
/// It is an INPUT to the core, never reconstructed here. A finite ring is sensitive to rotation
/// about the axis, and every "cross with world Y, else world X" rule has a discontinuous branch: a
/// shell whose direction crosses it would have its sample pattern snap to a new phase mid-flight,
/// and normalization/ricochet would re-roll the ring at every contact. Slice 2 anchors the basis at
/// spawn (from the muzzle) and parallel-transports it, storing it in the shell's sim bundle — it
/// affects collision results, so it belongs with the spawn-time invariants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiscFrame {
    pub u: Vec3,
    pub v: Vec3,
}

impl DiscFrame {
    /// The SPAWN basis for a shell fired along `axis`, with no external roll reference.
    ///
    /// A gun does have a roll about its bore, but the shell is axisymmetric and the ring is a
    /// SAMPLING pattern, not a physical feature — what matters (and what this buys) is that the
    /// pattern never snaps mid-flight, which [`transport`](Self::transport) guarantees from here on.
    /// Anchoring to the muzzle's actual up-vector would need it on `FireShell`, and therefore on the
    /// wire, for a phase the aggregates are near-invariant to.
    ///
    /// The branch is evaluated ONCE, at spawn: two shots either side of it sample differently, but
    /// they are different shots, and the aggregates (η, `n̄`, cost) barely notice — the residual is
    /// the sampling noise at small η that §13.5 already accepts as k's business.
    pub fn anchored(axis: Vec3) -> Option<Self> {
        // The less-aligned of two world axes, so the Gram-Schmidt below is never near-degenerate.
        let reference = if axis.y.abs() > 0.9 { Vec3::X } else { Vec3::Y };
        Self::from_axis_and_reference(axis, reference)
    }

    /// Build the SPAWN basis from an explicit reference direction. The caller owns the choice, so no
    /// world-axis branch can hide inside the core.
    pub fn from_axis_and_reference(axis: Vec3, reference: Vec3) -> Option<Self> {
        let axis = axis.normalize_or_zero();
        let u = reference - axis * axis.dot(reference);
        let len = u.length();
        if axis == Vec3::ZERO || len < 1.0e-4 {
            return None;
        }
        let u = u / len;
        Some(Self {
            u,
            v: axis.cross(u),
        })
    }

    /// Carry the basis through a direction change by the MINIMAL rotation taking `from` to `to`
    /// (rotation-minimizing transport). Roll is therefore never re-rolled by a bend or a bounce.
    ///
    /// The result is re-orthogonalized against `to`, which is the operation's actual contract: what
    /// comes back is a basis FOR `to`, not merely a rotated pair of vectors. Rotation alone preserves
    /// perpendicularity only when the input already had it, and a basis with a component ALONG the
    /// travel axis is not a sampling artefact — it puts sample rays in FRONT of and BEHIND the disc,
    /// so a ring ray starts inside geometry the axis has not reached and the walk reports an exit it
    /// never entered.
    pub fn transport(&self, from: Vec3, to: Vec3) -> Self {
        let to = to.normalize_or_zero();
        let rotation = Quat::from_rotation_arc(from.normalize_or_zero(), to);
        let carried = rotation * self.u;
        let u = carried - to * to.dot(carried);
        match Self::from_axis_and_reference(to, u) {
            Some(frame) => frame,
            // The carried vector collapsed onto the axis (a near-reversal, or a frame that was never
            // perpendicular). Re-anchor rather than return a basis that is not one.
            None => Self::anchored(to).unwrap_or(*self),
        }
    }
}

/// The disc's sample offsets: the axis, then a ring of `ring` samples at `radius`.
///
/// This is a deterministic sample MEAN, not an equal-area quadrature — so `η` below is sampled
/// coverage, not exact geometric area (§13.5's own note: k is the resolution dial). The ring's
/// `cos`/`sin` are `libm`'s, so the samples are the same bits on every shipped target.
pub fn disc_offsets(frame: &DiscFrame, radius: f32, ring: usize) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(ring + 1);
    out.push(Vec3::ZERO);
    for index in 0..ring {
        let angle = std::f32::consts::TAU * (index as f32) / (ring as f32);
        out.push((frame.u * libm::cosf(angle) + frame.v * libm::sinf(angle)) * radius);
    }
    out
}

/// DERIVED from §13.5's `k ≈ 8–16`: the axis plus a twelve-sample ring.
pub const DEFAULT_RING: usize = 12;

/// One sample ray of a disc corridor.
#[derive(Clone, Debug, Default)]
pub struct SampleCorridor {
    pub offset: Vec3,
    pub initial_presence: Vec<ShellKey>,
    pub hits: Vec<FaceHit>,
}

/// The caliber-wide probe: k parallel sample corridors.
#[derive(Clone, Debug)]
pub struct DiscCorridor {
    /// WORLD anchor; see [`RayCorridor::anchor`]. Every sample corridor inherits it unchanged, which
    /// is what makes their `t` directly comparable.
    pub anchor: Vec3,
    /// Corridor start, RELATIVE to [`anchor`](Self::anchor).
    pub origin: Vec3,
    pub axis: Vec3,
    pub length: f32,
    pub radius: f32,
    pub frame: DiscFrame,
    pub samples: Vec<SampleCorridor>,
}

/// One sample's part in a [`BodyExit`]: the body's extent along that sample's ray.
#[derive(Clone, Debug, PartialEq)]
pub struct ExitMember {
    pub sample: usize,
    /// Where the sample entered the body.
    pub start: f64,
    /// Where it left it: the body's rear boundary on this ray.
    pub end: f64,
    /// Corridor-relative position of `end` on this sample's ray.
    pub position: Vec3,
    /// The rear face's outward normal at a genuine exit (material to air); `None` at an internal
    /// step (material to softer material), which has no face of its own.
    pub face: Option<Vec3>,
    /// The volumes this sample's exit leaves, in entity order: present in the body's last layer on
    /// this ray and absent from the next (every one of them at a genuine exit).
    pub exited: Vec<Entity>,
    /// The factor of the body's last layer on this ray, the one the exit leaves: the body's highest,
    /// since a body never steps down in factor.
    pub rear_factor: f32,
    /// The debris ratio of that layer: its hardest presence's, the greatest on a tie of factors.
    pub debris_ratio: f32,
}

/// One body exit of a crossing, aggregated over the disc: geometry only, before any mechanism
/// decides whether the round reaches it.
///
/// A BODY is a maximal stretch of material along a sample, welded voids inside it included, over
/// which no layer resists less than the one before it. It EXITS where the next medium resists less
/// — the next layer has a lower factor, or the run ends in air — and the next body starts there. A
/// step to an equal or higher factor continues the body, whatever the substances: plates of equal
/// factor in contact are one body, and a body's spend is everything crossed since the run began or
/// the last exit. Members are grouped by the substances of the body (every substance crossed in
/// it), by whether the exit is genuine, and by their ordinal among such exits on their own ray;
/// longitudinal position cannot be the key, since on an oblique slab the ring's exits are spread by
/// `r·tan(incidence)` yet are one face.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyExit {
    /// The substances of the body exited, in id order.
    pub substances: Vec<SubstanceId>,
    /// Whether the exit is genuine (into air through a rear face) rather than an internal step.
    pub genuine: bool,
    /// The contributing samples, in sample order.
    pub members: Vec<ExitMember>,
}

/// A body exit the round reached: where it throws its spall cone from, and the round's state across
/// the body it left (design doc §5).
#[derive(Clone, Debug, PartialEq)]
pub struct SpallSource {
    /// Mean progress of the reached members' exits.
    pub t: f64,
    /// Mean position of the reached members' exits (corridor-relative from [`finish`]).
    pub position: Vec3,
    /// Mean outward rear-face normal at a genuine exit; `None` at an internal step.
    pub face: Option<Vec3>,
    /// The round's unit direction of travel through the crossing.
    pub direction: Vec3,
    /// The substances of the body exited, in id order.
    pub substances: Vec<SubstanceId>,
    /// The volumes the reached members' exits leave ([`ExitMember::exited`]), in entity order: the
    /// body that throws the cone. The surface the cone is thrown from is theirs, so a direction
    /// that starts inside one of them has not entered it there, and the cone never doses one of
    /// them anywhere, since the line has dosed them the energy its strength is a share of. A volume
    /// only an unreached member leaves is not among them: the round never left it, and a direction
    /// starting inside it enters it there.
    pub exited: Vec<Entity>,
    /// Reference-mm the mechanism spent across this body: its readout drop over the body's
    /// stretches, each stretch's drop shared among the samples by the factor each was resisted by,
    /// summed over the reached members. A readout (a burst fuze arms on it), not the cone's
    /// strength.
    pub spend: f32,
    /// The kinetic energy (J) the round lost across this body: its energy drop over the body's
    /// stretches, `½ m (v_before² − v_after²)` with `m` the mass that meets the plate
    /// ([`super::mechanism::Mechanism::energy`]), each stretch's drop shared among the samples as
    /// the spend's is, summed over the reached members. Across a body the whole disc crosses it is
    /// `½ m (v_entry² − v_exit²)`, the energy the line's dose deals the body's volumes.
    pub energy: f32,
    /// `λ`, the field law's ratio of the cone thrown here: the debris ratio of the body's last layer
    /// before the exit, the reached member's of the highest rear factor, the greatest ratio on a
    /// tie ([`ExitMember::rear_factor`], [`ExitMember::debris_ratio`]).
    pub debris_ratio: f32,
    /// The exited substance's strength against the reference armour steel, the share of the
    /// cone's potency it keeps: [`exit_strength_ratio`] of the highest rear factor among the
    /// reached members, the layer [`Self::debris_ratio`] is read from. Exactly 1 at or above the
    /// reference.
    pub strength_ratio: f32,
    /// The round's speed (m/s) where the earliest member entered the body.
    pub entry_speed: f32,
    /// The round's speed (m/s) where the latest reached member left it.
    pub exit_speed: f32,
    /// Fraction of the disc whose ray reached this exit.
    pub coverage: f32,
}

impl SpallSource {
    /// The cone's strength at the shipped energy share ([`super::spall_cone::energy_share`]);
    /// [`Self::strength_at`] states the law.
    pub fn strength(&self) -> f32 {
        self.strength_at(super::spall_cone::energy_share())
    }

    /// The cone's strength `S = share × energy × ratio` (J): `share` of the kinetic energy the
    /// round lost across the body ([`Self::energy`]), times the exited substance's strength ratio
    /// ([`Self::strength_ratio`]), as IEEE f32 products in that order. Whatever the round or the
    /// plate, a round that loses more energy across a body of one substance throws a stronger
    /// cone, and one that loses none throws none. A ratio of exactly 1 leaves `share × energy`
    /// to the bit, so every exit at or above the reference steel throws what it threw before the
    /// ratio existed; a softer exit (brick, wood) keeps only its share.
    pub(crate) fn strength_at(&self, share: f32) -> f32 {
        share * self.energy * self.strength_ratio
    }
}

/// The spall potency share an exit through a substance of `rear_factor` keeps: its strength against
/// the reference armour steel, `rear_factor / REFERENCE_FACTOR`, capped at 1. A substance's factor
/// is its strength ratio × [`crate::substances::REFERENCE_FACTOR`] (`materials.ron`), so the cap
/// leaves every substance at or above the reference exactly at 1.
pub(crate) fn exit_strength_ratio(rear_factor: f32) -> f32 {
    (rear_factor / crate::substances::REFERENCE_FACTOR).min(1.0)
}

/// One volume met by one sample over part of an [`Encounter`]: its identity (the address a dose
/// goes to), its material, and the stretch `[start, end)` of the encounter it covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeContact {
    pub volume: Entity,
    pub material: VolumeMaterial,
    pub start: f64,
    pub end: f64,
}

/// What one sample of the disc meets over an [`Encounter`].
#[derive(Clone, Debug, PartialEq)]
pub struct SampleLayer {
    pub sample: usize,
    /// The volumes the sample meets, ordered by where they open and then by volume; empty where
    /// the sample is in a gap of this crossing (a welded void). The substances among them are the
    /// same at every point of the encounter; which volume carries them may change.
    pub present: Vec<VolumeContact>,
}

/// One stretch of a disc crossing over which every sample's SUBSTANCE SET is constant: the ordered
/// encounter a mechanism advances through. A [`DiscEvent`]'s encounters tile the crossing
/// `[start, end]` in order, cut at every sample's [`Layer`] boundary, so there each one's present
/// volumes are constant too. The driver hands a mechanism each run of them joined across no
/// interface as one encounter ([`Encounter::absorb`]); such a merged encounter's `present` may list
/// volumes of one substance that never overlap, each over its own `[start, end)`, and its endpoints
/// are not every volume boundary inside it. Whatever needs volume boundaries reads the contact
/// intervals, never `present` membership. Samples outside the crossing over a stretch are absent
/// from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Encounter {
    pub start: f64,
    pub end: f64,
    /// Some sample's substance set differs here from the previous encounter's (the first
    /// encounter is always one). A boundary where only volume identity changes — two plates of one
    /// substance in contact — is not an interface: they are one plate, and the driver hands a
    /// mechanism the run they make as one encounter ([`Encounter::absorb`]).
    pub interface: bool,
    /// The samples inside the crossing over this stretch, in sample order.
    pub samples: Vec<SampleLayer>,
    /// k — the disc's sample count, the denominator of every disc mean.
    pub disc: usize,
}

impl Encounter {
    /// Extend this encounter over `next`, which follows it across no interface: every sample's
    /// substance set is the same over both, so to a mechanism they are one stretch of one
    /// material. A volume met on both sides of the seam is one contact.
    pub(crate) fn absorb(&mut self, next: &Encounter) {
        debug_assert!(!next.interface && next.start == self.end);
        self.end = next.end;
        for layer in &next.samples {
            let index = match self
                .samples
                .binary_search_by_key(&layer.sample, |own| own.sample)
            {
                Ok(index) => index,
                Err(index) => {
                    self.samples.insert(
                        index,
                        SampleLayer {
                            sample: layer.sample,
                            present: Vec::new(),
                        },
                    );
                    index
                }
            };
            let present = &mut self.samples[index].present;
            for contact in &layer.present {
                match present
                    .iter_mut()
                    .find(|own| own.volume == contact.volume && own.end == contact.start)
                {
                    Some(own) => own.end = contact.end,
                    None => present.push(*contact),
                }
            }
        }
    }
}

/// One entity's share of a whole disc crossing: what the crossing presents of it, before any
/// mechanism has decided how far the round gets.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityShare {
    pub entity: Entity,
    pub factor: f32,
    /// Disc-mean chord (m): the entity's covered chords summed over samples, divided by k.
    pub chord: f32,
    /// Disc-mean readout cost (reference-mm), `chord × factor`; the game's hard-pool dose is energy.
    pub cost: f32,
    /// Fraction of the disc that met this entity at all.
    pub coverage: f32,
}

/// One crossing of the world by the disc: the cluster of sample runs that belong to the same
/// physical surface (§13.5).
#[derive(Clone, Debug, PartialEq)]
pub struct DiscEvent {
    pub start: f64,
    pub end: f64,
    /// Area-mean entry normal over covered samples. Normals integrate (§13.5) — this is what
    /// repairs the point model's degenerate normal at an edge or on curved cast.
    pub entry_normal: Vec3,
    pub entry_position: Vec3,
    pub exit_normal: Vec3,
    pub exit_position: Vec3,
    /// The ENGAGEMENT fraction: covered samples / k.
    pub coverage: f32,
    pub exit_coverage: f32,
    /// The volume owning the entrance surface — the first body physically touched, and the one
    /// §13.5's ratified rule hands the whole momentum exchange to. Taken from the sample that
    /// touched FIRST, ties broken by sample index.
    pub entrance_volume: Entity,
    /// The entrance volume's factor (reference-mm per metre of chord): the resistance of the
    /// substance the round bites into, which the entry bend reads ([`begin`]).
    pub entrance_factor: f32,
    /// Every primitive this crossing touched, sorted — the identity half of the transit check.
    pub primitives: Vec<PrimitiveKey>,
    /// `(1/k) Σᵢ costᵢ` (reference-mm) — automatically `η × mean covered chord cost`. A readout:
    /// the entrance rules read it, no mechanism advances on it.
    pub cost: f32,
    /// The crossing's ordered encounters ([`Encounter`]).
    pub encounters: Vec<Encounter>,
    /// Every body exit along the crossing ([`BodyExit`]), ordered by mean exit progress.
    pub exits: Vec<BodyExit>,
    pub shares: Vec<EntityShare>,
    /// The `(sample, run)` pairs the crossing clusters, each an index into [`DiscWalk::walks`] and
    /// that walk's [`RayWalk::runs`], sorted. Crossings are rebuilt over these
    /// ([`DiscWalk::crossing_of`]).
    pub runs: Vec<(usize, usize)>,
}

/// Every crossing the disc corridor met, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscWalk {
    pub axis: Vec3,
    /// WORLD anchor; see [`RayCorridor::anchor`]. Every position this walk reports is relative to it.
    pub anchor: Vec3,
    /// Corridor start, RELATIVE to [`anchor`](Self::anchor).
    pub origin: Vec3,
    pub frame: DiscFrame,
    pub radius: f32,
    /// Lateral offsets, index-aligned with `walks`. `offsets[0]` is `ZERO` — the AXIS sample.
    pub offsets: Vec<Vec3>,
    /// Metres each sample's walk covers from its own start: what it can say anything about is
    /// `t ∈ [0, length]`.
    pub length: f32,
    /// The per-sample walks, RETAINED. The staged handoff has to export which primitives each
    /// sample is inside where it hands off, and neither the events nor the per-entity presence
    /// carry primitive identity — without this the caller would have to infer the seed state, which
    /// is the one thing this module never does.
    pub walks: Vec<RayWalk>,
    pub events: Vec<DiscEvent>,
}

impl DiscWalk {
    /// k — the sample count the disc aggregates over.
    pub fn samples(&self) -> usize {
        self.walks.len()
    }

    /// The seed state a corridor RESUMED at progress `t` would need — the lookup [`begin`] makes at
    /// each sample's hand-off, here at one progress for every sample.
    ///
    /// This is what lets the march step out of one crossing and into the next without ever inferring
    /// its own interior state: a shell that perforates an outer plate while a ring sample is already
    /// inside a volume behind it resumes KNOWING that, because the ray it would have to guess about
    /// is one this walk already covered.
    pub fn resume_at(&self, t: f64, laws: &WalkLaws) -> Vec<SampleSeed> {
        self.walks
            .iter()
            .enumerate()
            .map(|(sample, walk)| SampleSeed {
                sample,
                offset: self.offsets[sample],
                t,
                inside: walk.inside_at(t, laws),
            })
            .collect()
    }

    /// The events this walk's first crossing is made of, ascending: the first event, then every
    /// event that begins before the crossing so far ends (design doc §13.5, the hand-off). Empty
    /// when the walk met nothing.
    ///
    /// Both stages read the crossing this way. Which of two pieces that overlap in depth is struck
    /// first, by however little, decides nothing: they are one crossing in either order. And the
    /// flight resumes past the crossing's end, so a run that began inside it and was resolved apart
    /// from it would be skipped, or charged only from the resume point on.
    pub fn first_crossing(&self) -> Vec<usize> {
        let mut pieces = Vec::new();
        let mut end = f64::NEG_INFINITY;
        for (index, event) in self.events.iter().enumerate() {
            if !pieces.is_empty() && event.start >= end {
                break;
            }
            pieces.push(index);
            end = end.max(event.end);
        }
        pieces
    }

    /// Where each sample of this ENTRANCE walk hands off, and the crossing the entrance reads
    /// (design doc §13.5, the hand-off); `None` when the disc met nothing.
    ///
    /// A sample whose first run is in a piece of the [first crossing](Self::first_crossing) hands
    /// off at that run's start, on the face it crossed, and the read is those runs: the faces the
    /// samples strike and the material welded to them, never a run behind a face on its own ray, so
    /// spaced armour is not read as one plate. Any other sample hands off in air at the
    /// crossing's DEPTH: the read's mean entry point, square to the axis, clamped to the span
    /// between the sample's start and the crossing's end. Such a sample's first face, if it has
    /// one, is in a piece that begins at or past that end, so no sample hands off past the first
    /// face on its own ray, and every hand-off lies on ground its walk covered, whatever the
    /// corridor's length.
    pub fn hand_off(&self) -> Result<Option<HandOff<'_>>, WalkError> {
        let pieces = self.first_crossing();
        let Some(end) = pieces
            .iter()
            .map(|&piece| self.events[piece].end)
            .reduce(f64::max)
        else {
            return Ok(None);
        };
        let mut first_piece: Vec<Option<usize>> = vec![None; self.samples()];
        for (index, event) in self.events.iter().enumerate() {
            for &(sample, run) in &event.runs {
                if run == 0 {
                    first_piece[sample] = Some(index);
                }
            }
        }
        let on_face: Vec<bool> = first_piece
            .iter()
            .map(|piece| piece.is_some_and(|piece| pieces.contains(&piece)))
            .collect();
        let read = self.crossing_of(
            (0..self.samples())
                .filter(|&sample| on_face[sample])
                .map(|sample| (sample, 0))
                .collect(),
        )?;
        let at = self
            .walks
            .iter()
            .enumerate()
            .map(|(sample, walk)| {
                if on_face[sample] {
                    walk.runs[0].start
                } else {
                    let start = self.origin + self.offsets[sample];
                    f64::from((read.entry_position - start).dot(self.axis))
                        .min(end)
                        .max(0.0)
                }
            })
            .collect();
        Ok(Some(HandOff { read, at }))
    }

    /// The crossing the `(sample, run)` pairs `members` make: the first event itself when they are
    /// exactly its runs, or every aggregate rebuilt over them.
    pub fn crossing_of(
        &self,
        mut members: Vec<(usize, usize)>,
    ) -> Result<Cow<'_, DiscEvent>, WalkError> {
        members.sort_unstable();
        members.dedup();
        if let Some(first) = self.events.first()
            && members == first.runs
        {
            return Ok(Cow::Borrowed(first));
        }
        let rays = Rays {
            origin: self.origin,
            axis: self.axis,
            offsets: &self.offsets,
        };
        build_event(&rays, &self.walks, &members, self.samples()).map(Cow::Owned)
    }
}

/// Where an entrance disc's samples hand off ([`DiscWalk::hand_off`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HandOff<'w> {
    /// The crossing the entrance reads: `n̄`, η, the steel-equivalent thickness and the struck
    /// volume come from it.
    pub read: Cow<'w, DiscEvent>,
    /// Per sample, the progress along its entrance ray it hands off at, in `[0, length]`.
    pub at: Vec<f64>,
}

/// Walk every sample and associate their runs into crossing events.
pub fn walk_disc(
    corridor: &DiscCorridor,
    volumes: &VolumeTable,
    laws: &WalkLaws,
) -> Result<DiscWalk, WalkError> {
    let k = corridor.samples.len();
    if k == 0 {
        return Err(WalkError::BadCorridor {
            sample: 0,
            reason: "a disc corridor needs at least the axis sample",
        });
    }
    // Sample 0 IS the axis (the convention [`disc_offsets`] builds to). The staged handoff anchors
    // the transit ray on the axis rather than on a coverage centroid, so the axis must be
    // identifiable rather than inferred.
    if corridor.samples[0].offset != Vec3::ZERO {
        return Err(WalkError::BadCorridor {
            sample: 0,
            reason: "sample 0 must be the disc axis (zero lateral offset)",
        });
    }

    let mut walks: Vec<RayWalk> = Vec::with_capacity(k);
    for (index, sample) in corridor.samples.iter().enumerate() {
        walks.push(walk_ray(
            index,
            &RayCorridor {
                anchor: corridor.anchor,
                origin: corridor.origin + sample.offset,
                axis: corridor.axis,
                length: corridor.length,
                initial_presence: sample.initial_presence.clone(),
                hits: sample.hits.clone(),
            },
            volumes,
            laws,
        )?);
    }

    // Association: union-find over (sample, run). Longitudinal overlap alone is NOT the relation
    // (see `WalkLaws::event_plane_tolerance`); two runs join when they share a primitive, or when
    // their entrance patches lie on one surface.
    let mut nodes: Vec<(usize, usize)> = Vec::new();
    for (sample, walk) in walks.iter().enumerate() {
        for run in 0..walk.runs.len() {
            nodes.push((sample, run));
        }
    }
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    fn find(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    for a in 0..nodes.len() {
        for b in (a + 1)..nodes.len() {
            let (sa, ra) = nodes[a];
            let (sb, rb) = nodes[b];
            if sa == sb {
                continue;
            }
            let run_a = &walks[sa].runs[ra];
            let run_b = &walks[sb].runs[rb];

            // Association takes TWO conjuncts, always: the runs must be longitudinally coherent AND
            // related. Either alone merges crossings that are not one.
            //
            // LONGITUDINAL. A hard ceiling first — beyond it no single surface patch sampled by this
            // disc could present both runs, whatever else they have in common. Within it, the two
            // branches ask different questions, because they measure different things.
            let overlap = run_a.start < run_b.end && run_b.start < run_a.end;
            let gap_along = (run_b.start - run_a.end)
                .max(run_a.start - run_b.end)
                .max(0.0) as f32;
            let facing = unit_or_zero(run_a.entry_normal + run_b.entry_normal);
            // Not "are these close enough along the ray" — one plane can put two samples half a
            // metre apart and mean it — but "are they where ONE surface would have put them".
            //
            // This is BRANCH 2's conjunct alone. Branch 1 has its own longitudinal relation
            // (`overlap || weld_class`) and must not get this one: the same primitive crossed twice
            // at overlapping depths is one crossing even when its faces are a curved cast surface
            // with no shared plane to predict from, and a residual measured against a `n̄` that is
            // the average of two unrelated normals would shatter it.
            let residual = separation_residual(corridor, (sa, run_a), (sb, run_b), facing, laws);
            let one_surface = residual.abs() <= laws.event_residual_tolerance;

            // BRANCH 1 — same primitive, and the runs actually touch.
            //
            // Identity alone is not the relation. One watertight CONCAVE primitive can be crossed
            // twice by the disc at genuinely separate places — the two arms of a bracket are the same
            // mesh, and unioning them collapses two crossings into one: one entrance law instead of
            // two, one summed overmatch thickness, one exit where there are two. So the runs must
            // overlap, or be separated by no more than a WELD-CLASS void — the same micro-gap §13.4
            // would have deleted had the two runs lain on one ray. "Inside the weld LOOKAHEAD" is not
            // that: the lookahead bounds where a weld may be looked for, never what qualifies as one.
            let shares_primitive = run_a
                .primitives
                .iter()
                .any(|key| run_b.primitives.binary_search(key).is_ok());
            let weld_class = gap_along * corridor.axis.dot(facing).abs() <= laws.weld_perp;

            // BRANCH 2 — one surface, whatever the mesh partition.
            //
            // This is what keeps seam invisibility (§13.6): two flush plates of DIFFERENT entities
            // must associate, or a shot near their seam resolves as two crossings where a mid-plate
            // shot resolves as one. It also carries the oblique slab that no along-ray test can —
            // the ring's chords shift by `r·tan(incidence)` and stop overlapping entirely, yet every
            // entry point still lies on the one plane. The plane test IS the longitudinal measure for
            // parallel faces (two faces 400 mm apart are 400 mm apart along their shared normal);
            // the residual is what stops it stretching down a grazing corridor without limit, since
            // a fixed PERPENDICULAR tolerance buys `tolerance / cos(incidence)` of along-ray slack —
            // 44 mm at 87°, and unbounded as the ray goes parallel.
            let coplanar = {
                let na = run_a.entry_normal;
                let nb = run_b.entry_normal;
                let pa = corridor.origin
                    + corridor.samples[sa].offset
                    + corridor.axis * run_a.start as f32;
                let pb = corridor.origin
                    + corridor.samples[sb].offset
                    + corridor.axis * run_b.start as f32;
                na.dot(nb) >= laws.event_plane_cos
                    && facing != Vec3::ZERO
                    && (pa - pb).dot(facing).abs() <= laws.event_plane_tolerance
            };

            if (shares_primitive && (overlap || weld_class)) || (coplanar && one_surface) {
                let (ra_root, rb_root) = (find(&mut parent, a), find(&mut parent, b));
                if ra_root != rb_root {
                    parent[ra_root.max(rb_root)] = ra_root.min(rb_root);
                }
            }
        }
    }

    let mut clusters: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (node, &member) in nodes.iter().enumerate() {
        let root = find(&mut parent, node);
        clusters.entry(root).or_default().push(member);
    }

    let offsets: Vec<Vec3> = corridor.samples.iter().map(|s| s.offset).collect();
    let rays = Rays {
        origin: corridor.origin,
        axis: corridor.axis,
        offsets: &offsets,
    };
    let mut events: Vec<DiscEvent> = Vec::new();
    for members in clusters.values() {
        events.push(build_event(&rays, &walks, members, k)?);
    }
    events.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));

    Ok(DiscWalk {
        axis: corridor.axis,
        anchor: corridor.anchor,
        origin: corridor.origin,
        frame: corridor.frame,
        radius: corridor.radius,
        offsets,
        length: corridor.length,
        walks,
        events,
    })
}

/// Where a disc's sample rays start and run: the corridor geometry an event's positions are read
/// from.
struct Rays<'a> {
    origin: Vec3,
    axis: Vec3,
    /// Index-aligned with the walks.
    offsets: &'a [Vec3],
}

/// Where ONE shared surface would put sample `b`'s run relative to sample `a`'s, along the ray.
///
/// Exact, not bounded: two rays offset laterally by `d` meet a plane of normal `n̄` at progresses
/// differing by `−(d·n̄)/(axis·n̄)`. That is the whole geometry of "these two runs are the same
/// surface seen by two samples", and its worst case over a disc — `|d| = 2r`, `d` up the plane's
/// steepest line — is the `2·r·tan(incidence)` §13.5 named.
///
/// Bounding the SEPARATION by that worst case (which is what shipped) is not the same statement, and
/// the difference is not academic. The worst case is reached only by diametrically opposite samples;
/// every other pair on the same plane is closer, so a bound sized for the extreme admits pairs that
/// are nowhere near one surface, while a bound sized for the typical pair rejects the extreme. The
/// second half of that is MEASURED: an 88 at 80° whose axis and intermediates pass through an
/// opening leaves only the two opposite rim samples on one plane, and the pair splits into two
/// events — a valid contact refused because it sat exactly at the extreme the bound was cut to.
///
/// The RESIDUAL has no such tension. It is zero for one surface at every incidence, every calibre,
/// and every pair, however far apart along the ray they land; and for two genuinely separate
/// crossings that happen to lie near one plane it is the distance between them, which is what the
/// longitudinal conjunct was always trying to measure.
///
/// The secant cap is the only approximation, and it exists solely because the division diverges as
/// the ray goes parallel to the surface — see [`WalkLaws::event_secant_cap`] for its dimensions.
fn separation_residual(
    corridor: &DiscCorridor,
    (sa, run_a): (usize, &WeldedRun),
    (sb, run_b): (usize, &WeldedRun),
    facing: Vec3,
    laws: &WalkLaws,
) -> f32 {
    let lateral = corridor.samples[sb].offset - corridor.samples[sa].offset;
    let denominator = corridor.axis.dot(facing);
    // `1/x = sec·sign(x)`, so the cap applies to the magnitude and the direction survives it.
    let secant = if denominator == 0.0 {
        laws.event_secant_cap
    } else {
        (1.0 / denominator.abs()).min(laws.event_secant_cap)
    };
    let sign = if denominator < 0.0 { -1.0 } else { 1.0 };
    let predicted = -lateral.dot(facing) * secant * sign;
    ((run_b.start - run_a.start) - predicted as f64) as f32
}

/// The crossing the `(sample, run)` pairs `members` make: every aggregate over the disc of `k`.
fn build_event(
    rays: &Rays<'_>,
    walks: &[RayWalk],
    members: &[(usize, usize)],
    k: usize,
) -> Result<DiscEvent, WalkError> {
    let kf = k as f64;
    let mut by_sample: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(sample, run) in members {
        by_sample.entry(sample).or_default().push(run);
    }
    for runs in by_sample.values_mut() {
        runs.sort_unstable();
    }
    let runs: Vec<(usize, usize)> = by_sample
        .iter()
        .flat_map(|(&sample, runs)| runs.iter().map(move |&run| (sample, run)))
        .collect();

    let mut start = f64::INFINITY;
    let mut end = f64::NEG_INFINITY;
    let mut entrance_volume = None;
    let mut entry_sum = Vec3::ZERO;
    let mut entry_position = Vec3::ZERO;
    let mut exit_sum = Vec3::ZERO;
    let mut exit_position = Vec3::ZERO;
    for (&sample, runs) in &by_sample {
        let first = &walks[sample].runs[runs[0]];
        let last = &walks[sample].runs[runs[runs.len() - 1]];
        if first.start < start {
            entrance_volume = Some((sample, first.entry_volume));
        }
        start = start.min(first.start);
        end = end.max(last.end);
        entry_sum += first.entry_normal;
        exit_sum += last.exit_normal;
        let base = rays.origin + rays.offsets[sample];
        entry_position += base + rays.axis * first.start as f32;
        exit_position += base + rays.axis * last.end as f32;
    }
    let covered = by_sample.len();
    let coverage = (covered as f64 / kf) as f32;

    // A single covered sample IS the mean: normalizing an already-unit vector perturbs its low bits
    // and would break the r→0, k=1 fragment degeneracy (§13.5: a fragment is a shell with k=1, and
    // it must reduce EXACTLY to the single-ray walk, not approximately).
    let entry_normal = if covered == 1 {
        entry_sum
    } else {
        let normal = unit_or_zero(entry_sum);
        if normal == Vec3::ZERO {
            return Err(WalkError::DegenerateEntryNormal { coverage });
        }
        normal
    };
    let exit_normal = if covered == 1 {
        exit_sum
    } else {
        unit_or_zero(exit_sum)
    };
    let entry_position = entry_position / covered as f32;
    let exit_position = exit_position / covered as f32;

    let cost = disc_mean_cost(walks, &by_sample, start, end, k);
    let encounters = encounters(walks, &by_sample, start, end, k);
    let shares = entity_shares(walks, &by_sample, k);
    let (exits, exit_covered) = body_exits(rays, walks, &by_sample);

    let mut primitives: Vec<PrimitiveKey> = by_sample
        .iter()
        .flat_map(|(sample, runs)| {
            runs.iter()
                .flat_map(move |run| walks[*sample].runs[*run].primitives.iter().copied())
        })
        .collect();
    primitives.sort();
    primitives.dedup();

    let Some((entrance_sample, entrance_volume)) = entrance_volume else {
        // Defensive: a cluster is built from at least one welded run, and every run names the
        // volume whose shell opens it. A stand-in entity here would be an event with no owner,
        // spending capability and marking a penetration against nothing.
        return Err(WalkError::BadCorridor {
            sample: 0,
            reason: "a disc crossing cluster holds no run",
        });
    };
    // Defensive: a run's entry volume is read off its sample's own intervals, and that sample's
    // presence lists every volume with an interval.
    let entrance_factor = walks[entrance_sample]
        .presence
        .iter()
        .find(|presence| presence.entity == entrance_volume)
        .map(|presence| presence.factor)
        .ok_or(WalkError::BadCorridor {
            sample: entrance_sample,
            reason: "the entrance volume has no presence on the sample that touched it",
        })?;
    Ok(DiscEvent {
        start,
        end,
        entrance_volume,
        entrance_factor,
        primitives,
        entry_normal,
        entry_position,
        exit_normal,
        exit_position,
        coverage,
        exit_coverage: (exit_covered as f64 / kf) as f32,
        cost,
        encounters,
        exits,
        shares,
        runs,
    })
}

/// The disc-mean cost of one crossing over `[start, end]`, accumulated over the union of every
/// contributing sample's canonical span boundaries; between two of them every sample's factor is
/// constant, so the mean slope is exact.
fn disc_mean_cost(
    walks: &[RayWalk],
    by_sample: &BTreeMap<usize, Vec<usize>>,
    start: f64,
    end: f64,
    k: usize,
) -> f32 {
    let kf = k as f64;
    let mut breaks: Vec<f64> = vec![start, end];
    for (&sample, runs) in by_sample {
        for &run in runs {
            let extent = &walks[sample].runs[run];
            let spans = &walks[sample].spans;
            // Spans tile the corridor in order: those overlapping the run, by bisection.
            let from = spans.partition_point(|span| span.end <= extent.start);
            let to = spans
                .partition_point(|span| span.start < extent.end)
                .max(from);
            for span in &spans[from..to] {
                breaks.push(span.start);
                breaks.push(span.end);
            }
        }
    }
    breaks.sort_by(f64::total_cmp);
    breaks.dedup();
    breaks.retain(|t| *t >= start && *t <= end);

    let mut total = 0.0f64;
    for window in breaks.windows(2) {
        let (a, b) = (window[0], window[1]);
        let mid = 0.5 * (a + b);
        let mut slope = 0.0f64;
        for (&sample, runs) in by_sample {
            // The CANONICAL spans carry the field, welded voids included at factor 0 — reading the
            // material segments instead would smear steel across a deleted gap, and welding deletes
            // faces, never material (§13.4).
            let inside = runs.iter().any(|&run| {
                let extent = &walks[sample].runs[run];
                extent.start <= mid && mid < extent.end
            });
            if !inside {
                continue;
            }
            let spans = &walks[sample].spans;
            if let Some(span) = spans
                .get(spans.partition_point(|span| span.end <= mid))
                .filter(|span| span.start <= mid && mid < span.end)
            {
                slope += span.factor as f64;
            }
        }
        total += (b - a) * slope / kf;
    }
    total as f32
}

/// The per-entity shares of one crossing, unioned per sample then averaged over the disc.
fn entity_shares(
    walks: &[RayWalk],
    by_sample: &BTreeMap<usize, Vec<usize>>,
    k: usize,
) -> Vec<EntityShare> {
    let kf = k as f64;
    let mut shares: BTreeMap<Entity, (f32, Vec<(f64, f64)>, BTreeSet<usize>)> = BTreeMap::new();
    for (&sample, runs) in by_sample {
        let (lo, hi) = (
            walks[sample].runs[runs[0]].start,
            walks[sample].runs[runs[runs.len() - 1]].end,
        );
        for presence in &walks[sample].presence {
            for span in &presence.spans {
                let clipped = (span.0.max(lo), span.1.min(hi));
                if clipped.1 > clipped.0 {
                    let slot = shares.entry(presence.entity).or_insert((
                        presence.factor,
                        Vec::new(),
                        BTreeSet::new(),
                    ));
                    slot.1.push(clipped);
                    slot.2.insert(sample);
                }
            }
        }
    }
    shares
        .into_iter()
        .map(|(entity, (factor, spans, samples))| {
            let chord: f64 = spans.iter().map(|(a, b)| *b - *a).sum::<f64>() / kf;
            EntityShare {
                entity,
                factor,
                chord: chord as f32,
                cost: (chord * factor as f64) as f32,
                coverage: (samples.len() as f64 / kf) as f32,
            }
        })
        .collect()
}

/// The debris ratio of a layer of `present` volumes: its hardest presence's, the greatest on a tie
/// of factors (a total order on `(factor, ratio)`, so the answer does not hang on volume order);
/// zero for an empty layer.
fn debris_ratio(present: &[Presence]) -> f32 {
    present
        .iter()
        .map(|presence| (presence.material.factor, presence.material.debris_ratio))
        .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)))
        .map_or(0.0, |(_, ratio)| ratio)
}

/// The body exits of one crossing, per sample, then grouped over the disc ([`BodyExit`]) and
/// ordered by mean exit depth; with the number of samples one of whose runs exits material.
fn body_exits(
    rays: &Rays<'_>,
    walks: &[RayWalk],
    by_sample: &BTreeMap<usize, Vec<usize>>,
) -> (Vec<BodyExit>, usize) {
    let mut exit_groups: BTreeMap<(Vec<SubstanceId>, bool, usize), Vec<ExitMember>> =
        BTreeMap::new();
    let mut exit_covered = 0usize;
    for (&sample, runs) in by_sample {
        let base = rays.origin + rays.offsets[sample];
        let mut ordinals: BTreeMap<(Vec<SubstanceId>, bool), usize> = BTreeMap::new();
        let mut member = |substances: Vec<SubstanceId>,
                          (rear_factor, debris_ratio): (f32, f32),
                          start: f64,
                          end: f64,
                          face: Option<Vec3>,
                          mut exited: Vec<Entity>| {
            let genuine = face.is_some();
            let ordinal = ordinals.entry((substances.clone(), genuine)).or_insert(0);
            exited.sort();
            exited.dedup();
            exit_groups
                .entry((substances, genuine, *ordinal))
                .or_default()
                .push(ExitMember {
                    sample,
                    start,
                    end,
                    position: base + rays.axis * end as f32,
                    face,
                    exited,
                    rear_factor,
                    debris_ratio,
                });
            *ordinal += 1;
        };
        let mut saw_exit = false;
        for &run in runs {
            let extent = &walks[sample].runs[run];
            if walks[sample]
                .events
                .iter()
                .any(|e| e.run == run && e.kind == BoundaryKind::Exit && e.factor_before > 0.0)
            {
                saw_exit = true;
            }
            // (substances so far, factor and debris ratio of its latest layer, start, end, volumes
            // of its latest layer) of the body the sample is in. A layer resisting at least as
            // much as the one before it continues the body.
            type Body = (Vec<SubstanceId>, (f32, f32), f64, f64, Vec<Entity>);
            let mut body: Option<Body> = None;
            for layer in layers_over(&walks[sample].layers, extent.start, extent.end) {
                let (from, to) = (layer.start.max(extent.start), layer.end.min(extent.end));
                if layer.present.is_empty() || to <= from {
                    continue;
                }
                let substances = layer.substances();
                let factor = layer
                    .present
                    .iter()
                    .fold(0.0f32, |most, presence| most.max(presence.material.factor));
                let rear = (factor, debris_ratio(&layer.present));
                let volumes: Vec<Entity> = layer.present.iter().map(|p| p.volume).collect();
                body = Some(match body.take() {
                    Some((held, before, start, end, last)) if factor < before.0 => {
                        let left = last
                            .into_iter()
                            .filter(|volume| !volumes.contains(volume))
                            .collect();
                        member(held, before, start, end, None, left);
                        (substances, rear, from, to, volumes)
                    }
                    Some((mut held, _, start, _, _)) => {
                        held.extend(substances);
                        held.sort();
                        held.dedup();
                        (held, rear, start, to, volumes)
                    }
                    None => (substances, rear, from, to, volumes),
                });
            }
            if let Some((held, rear, start, end, last)) = body
                && rear.0 > 0.0
            {
                member(held, rear, start, end, Some(extent.exit_normal), last);
            }
        }
        if saw_exit {
            exit_covered += 1;
        }
    }
    let mut exits: Vec<BodyExit> = exit_groups
        .into_iter()
        .map(|((substances, genuine, _), members)| BodyExit {
            substances,
            genuine,
            members,
        })
        .collect();
    let mean_end = |exit: &BodyExit| {
        exit.members.iter().map(|m| m.end).sum::<f64>() / exit.members.len() as f64
    };
    exits.sort_by(|a, b| mean_end(a).total_cmp(&mean_end(b)));
    (exits, exit_covered)
}

/// The layers of `layers` (one sample's, which tile its corridor in order) that touch
/// `[start, end]`, found by bisection: a corridor of many crossings is not rescanned per crossing.
fn layers_over(layers: &[Layer], start: f64, end: f64) -> &[Layer] {
    let from = layers.partition_point(|layer| layer.end < start);
    let to = layers.partition_point(|layer| layer.start <= end).max(from);
    &layers[from..to]
}

/// The ordered encounters of one disc crossing: `[start, end]` cut at every boundary of every
/// contributing sample's layers, each stretch listing what each sample inside the crossing meets.
fn encounters(
    walks: &[RayWalk],
    by_sample: &BTreeMap<usize, Vec<usize>>,
    start: f64,
    end: f64,
    k: usize,
) -> Vec<Encounter> {
    let mut cuts: Vec<f64> = vec![start, end];
    for &sample in by_sample.keys() {
        for layer in layers_over(&walks[sample].layers, start, end) {
            cuts.push(layer.start);
            cuts.push(layer.end);
        }
    }
    cuts.retain(|t| *t >= start && *t <= end);
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();

    let mut out: Vec<Encounter> = Vec::with_capacity(cuts.len());
    let mut before: BTreeMap<usize, Vec<SubstanceId>> = BTreeMap::new();
    for window in cuts.windows(2) {
        let (a, b) = (window[0], window[1]);
        let mid = 0.5 * (a + b);
        let mut samples = Vec::new();
        let mut now: BTreeMap<usize, Vec<SubstanceId>> = BTreeMap::new();
        for (&sample, runs) in by_sample {
            let inside = runs.iter().any(|&run| {
                let extent = &walks[sample].runs[run];
                extent.start <= mid && mid < extent.end
            });
            if !inside {
                continue;
            }
            let Some(layer) = layers_over(&walks[sample].layers, a, b)
                .iter()
                .find(|layer| layer.start <= mid && mid < layer.end)
            else {
                continue;
            };
            now.insert(sample, layer.substances());
            samples.push(SampleLayer {
                sample,
                present: layer
                    .present
                    .iter()
                    .map(|presence| VolumeContact {
                        volume: presence.volume,
                        material: presence.material,
                        start: a,
                        end: b,
                    })
                    .collect(),
            });
        }
        let interface = out.is_empty() || {
            let substances = |map: &BTreeMap<usize, Vec<SubstanceId>>, sample: &usize| {
                map.get(sample).cloned().unwrap_or_default()
            };
            before
                .keys()
                .chain(now.keys())
                .any(|sample| substances(&before, sample) != substances(&now, sample))
        };
        out.push(Encounter {
            start: a,
            end: b,
            interface,
            samples,
            disc: k,
        });
        before = now;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Stage 3 — the staged resolution (§13.3 surface laws, §5 spall, §3 capability)
// ---------------------------------------------------------------------------------------------

/// The striking round, reduced to what the entrance rules read.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub caliber: f32,
}

/// What the entrance disc decided, and what the surface laws read to decide it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntranceRead {
    pub position: Vec3,
    /// The volume owning the surface struck, the read's [`DiscEvent::entrance_volume`]: the body
    /// the whole momentum exchange goes to (§13.5), for a bounce and a bite alike.
    pub volume: Entity,
    /// `n̄`, the area-mean entry normal.
    pub normal: Vec3,
    pub coverage: f32,
    pub incidence: f32,
    /// Steel-equivalent thickness ALONG THE NORMAL (m) of the material actually engaged.
    ///
    /// §13.3's amendment: overmatch and ricochet consult `thickness × factor`, not geometry, or an
    /// exposed forearm reads 80 mm "thick" and past 70° an arm ricochets an 88.
    ///
    /// Measured over the COVERED samples (`cost ÷ η`), not the whole disc: a partial engagement must
    /// not fake a thin plate and suppress the ricochet that makes a graze a graze (§13.5). Whether a
    /// bounce happens at all lives here, in the factor-weighted overmatch law; how HARD the round is
    /// turned is η's business, in [`Begin::Ricochet`].
    pub steel_equivalent: f32,
    pub overmatched: bool,
}

/// Where [`begin`] leaves the shot.
#[derive(Clone, Debug, PartialEq)]
pub enum Begin {
    /// The disc met no material.
    Miss,
    /// Too oblique, not overmatched: deflect off `n̄`, no entry, no spall (§4).
    Ricochet {
        entrance: EntranceRead,
        /// Outgoing direction, blended from the incoming ray toward the full specular reflection by
        /// η (§13.5, RULED 2026-08-07). η = 1 is the classic bounce, returned bit-for-bit; η → 0
        /// fades to undisturbed flight. Continuity is the whole point — 1 mm of aim must never flip
        /// the outcome between "flies past" and "full bounce".
        ///
        /// The lateral TORQUE a corner clip really applies stays deferred (§13.5, re-affirmed
        /// 2026-08-07): only the deflection MAGNITUDE ships. The mean lateral offset of the covered
        /// samples is the ready-made kick direction whenever it is wanted.
        direction: Vec3,
        /// Share of the striking speed retained. At η = 1 it is the share along the surface,
        /// `sin θ` (`θ` the incidence from `n̄`): the shell keeps its sliding speed and loses the
        /// motion pointed into the face, restitution 0, so the energy it loses there is
        /// `½ m (v cos θ)²`. η blends it from 1 (η → 0, nothing lost) to that, as it blends the
        /// direction — a graze IS a partial ricochet.
        speed_scale: f32,
        /// Per sample, the progress along its incoming ray where its way in ends: its hand-off
        /// ([`DiscWalk::hand_off`]), as a transit's way in does.
        hand_offs: Vec<f64>,
    },
    /// It bit in. The caller must now collect the transit corridor along `request.axis`.
    Transit(TransitRequest),
}

/// One transit sample's starting state — where its ray resumes, and what it is already inside.
///
/// Each sample resumes on its OWN entrance ray, at its hand-off ([`DiscWalk::hand_off`]): on the
/// face of the piece it crossed, or in air at the crossing's depth when it crossed none by then.
/// Never on a disc re-projected around the bent axis. Two things fall out of that, and neither is
/// available any other way:
///
/// - The seed is EXACT. The resume point lies on a ray this module already walked, so "which
///   primitives is it inside" is a lookup ([`RayWalk::inside_at`]), not an inference — and
///   inference is the one thing the walk never does, because "it must have started inside" is how a
///   dropped entry face becomes free armour.
/// - It is what physically happens. The rays bend where they strike; a sample that met the plate
///   40 mm further along resumes 40 mm further along.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleSeed {
    pub sample: usize,
    /// Displacement of this sample's transit ray origin from [`TransitRequest::origin`]. Not purely
    /// lateral — it carries the longitudinal spread of an oblique contact. `seeds[0].offset` is
    /// `ZERO`: the axis sample defines the origin.
    pub offset: Vec3,
    /// Progress along that sample's ENTRANCE ray at which the seed was read: its hand-off, in
    /// `[0, DiscWalk::length]`.
    pub t: f64,
    pub inside: Vec<ShellKey>,
}

/// The query the core needs answered to finish the crossing.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitRequest {
    pub entrance: EntranceRead,
    /// The NORMALIZED (bent) travel axis — the transit happens along this, not along the incoming
    /// direction the entrance was sampled on.
    pub axis: Vec3,
    /// WORLD anchor, carried through unchanged from the entrance corridor — see
    /// [`RayCorridor::anchor`]. Sharing it is the point: the handoff below is then a LOCAL offset
    /// from the same origin the entrance's face positions were measured from, so the two agree to
    /// corridor-scale rounding however far downrange the shot is.
    pub anchor: Vec3,
    /// The AXIS sample's hand-off, RELATIVE to [`anchor`](Self::anchor): on the face it crossed, or
    /// on its own ray at the crossing's depth when it crossed none ([`DiscWalk::hand_off`]).
    ///
    /// Deliberately NOT the covered-sample centroid. A centroid drags the shell's line of flight
    /// sideways toward whatever part of the disc happened to touch geometry — a lateral kick, which
    /// is exactly the asymmetry §13.5 rules out ("lateral asymmetry unmodeled", re-affirmed
    /// 2026-08-07). The aggregate SURFACE is a mean (`entrance.position`, `n̄`); the shell's AXIS is
    /// not.
    pub origin: Vec3,
    /// The spawn-anchored basis, parallel-transported onto the bent axis. Never re-derived.
    pub frame: DiscFrame,
    pub radius: f32,
    /// k — the corridor supplied to [`finish`] must sample the disc the same way.
    pub samples: usize,
    /// The primitives the entrance crossing touched, sorted. [`finish`] checks the supplied
    /// corridor's crossing against these, so a corridor collected for some OTHER contact cannot be
    /// resolved as if it were this one. Entity identity alone is too coarse for that: one volume can
    /// present several primitives metres apart, so "the corridor's crossing names a volume the
    /// entrance also named" is satisfied by a crossing of a completely different part of the same
    /// hull.
    pub entrance_primitives: Vec<PrimitiveKey>,
    pub seeds: Vec<SampleSeed>,
}

/// Read the entrance disc and decide whether the round enters (§4's decision tree, §13.3's inputs).
///
/// The read is the first crossing's ([`DiscWalk::hand_off`]), so a disc meeting two pieces that
/// overlap in depth reads both surfaces, whichever it meets first.
pub fn begin(entrance: &DiscWalk, shot: &Shot, laws: &WalkLaws) -> Result<Begin, WalkError> {
    let Some(HandOff { read: event, at }) = entrance.hand_off()? else {
        return Ok(Begin::Miss);
    };
    if event.coverage <= 0.0 || event.entry_normal == Vec3::ZERO {
        return Ok(Begin::Miss);
    }
    let normal = event.entry_normal;
    let incidence = entrance.axis.angle_between(-normal);
    // Covered-sample mean, then projected onto the normal: the thickness of the material that is
    // actually there, along the direction "thickness" means.
    let along_normal = entrance.axis.dot(normal).abs();
    let steel_equivalent =
        (event.cost / event.coverage.max(f32::MIN_POSITIVE)) / laws.rha_reference * along_normal;
    let overmatched =
        steel_equivalent > 0.0 && shot.caliber >= laws.overmatch_ratio * steel_equivalent;
    let read = EntranceRead {
        position: event.entry_position,
        volume: event.entrance_volume,
        normal,
        coverage: event.coverage,
        incidence,
        steel_equivalent,
        overmatched,
    };

    let Ok(axis) = Dir3::new(entrance.axis) else {
        return Ok(Begin::Miss);
    };
    let Ok(surface) = Dir3::new(normal) else {
        return Ok(Begin::Miss);
    };

    // η scales the deflection ANGLE as well as the speed lost (§13.5). A graze that turned the
    // round as hard as a square bounce made 1 mm of aim the difference between "flies past" and
    // "full bounce" — the cliff the disc primitive exists to remove.
    let blend_by_coverage = |from: Dir3, toward: Dir3, coverage: f32| {
        super::bend_toward(
            from,
            toward,
            coverage * Vec3::from(from).angle_between(toward.into()),
        )
    };

    if !overmatched && incidence > laws.ricochet_angle {
        let specular = super::reflect(axis, surface);
        // At full coverage the blend IS the classic bounce, so it is returned VERBATIM: rotating a
        // direction onto its own reflection by quaternion would perturb the low bits, and the
        // pre-ruling full-bounce path must stay bit-identical.
        let direction = if event.coverage >= 1.0 {
            specular
        } else {
            blend_by_coverage(axis, specular, event.coverage)
        };
        // `sin θ` as the striking direction's part along the surface, `√(1 − (axis · n̄)²)`: an
        // IEEE square root rather than the sine of the read incidence, so the share is the same on
        // every target.
        let sliding = (1.0 - along_normal * along_normal).max(0.0).sqrt();
        return Ok(Begin::Ricochet {
            entrance: read,
            direction: Vec3::from(direction),
            speed_scale: 1.0 - event.coverage * (1.0 - sliding),
            hand_offs: at,
        });
    }

    // Normalize: a modest bend toward the inward normal as it bites in, scaled by η on the same
    // ruling — a barely-engaged entry is barely bent — and by the entrance substance's resistance
    // over the reference's, so a substance that resists a round less turns it less. The reference
    // substance's share is exactly 1, so its bend is `η × normalization × incidence` to the bit. The
    // share is unclamped: a substance declared above the reference would bend a round further.
    // Overmatch adds no bend; it cancels the SLOPE COST instead (charged in `finish`).
    let resistance_share = event.entrance_factor / laws.rha_reference;
    let bent = super::bend_toward(
        axis,
        -surface,
        event.coverage * laws.normalization * incidence * resistance_share,
    );

    // Each sample resumes where [`DiscWalk::hand_off`] put it, and the transit ray is the AXIS
    // sample's: the request's origin is its hand-off, every other sample's offset carries its own.
    // The seed is a lookup on the ray each sample walked, at the point it resumes from.
    let axis_t = at[0];
    let origin = entrance.origin + entrance.axis * axis_t as f32;
    let seeds = entrance
        .walks
        .iter()
        .enumerate()
        .map(|(index, walk)| {
            let t = at[index];
            SampleSeed {
                sample: index,
                offset: entrance.offsets[index] + entrance.axis * (t - axis_t) as f32,
                t,
                inside: walk.inside_at(t, laws),
            }
        })
        .collect();

    Ok(Begin::Transit(TransitRequest {
        entrance: read,
        axis: Vec3::from(bent),
        anchor: entrance.anchor,
        origin,
        frame: entrance.frame.transport(entrance.axis, Vec3::from(bent)),
        radius: entrance.radius,
        samples: entrance.samples(),
        entrance_primitives: event.primitives.clone(),
        seeds,
    }))
}

/// How the crossing ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// Defeated: buried where the mechanism came to rest.
    Embedded { at: Vec3, t: f32 },
    /// Punched through: the march resumes at the aggregate exit face.
    Perforated {
        exit: Vec3,
        exit_normal: Vec3,
        direction: Vec3,
        t: f32,
    },
}

/// Everything the crossing decided, as declarative facts for the caller to apply in order.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolutionPlan<S> {
    pub entrance: EntranceRead,
    pub outcome: Outcome,
    /// The mechanism's state where the crossing left it: at rest on an embed, past the exit on a
    /// perforation.
    pub state: S,
    /// Reference-mm spent — the readout of the state in, less the readout of the state out.
    pub cost_spent: f32,
    /// The line's doses, one per volume it put a positive chord into, summed over the crossing in
    /// volume order ([`DoseSource::Line`]): each the energy the round lost in that volume, joules.
    ///
    /// Damage attribution needs no arbitration between bodies: each depth's dose goes to the
    /// strongest volume present there, ties sharing (§13.2). The IMPULSE does, when a crossing's
    /// presence set spans several rigid bodies — which can only happen when distinct bodies
    /// overlap in space, since within one tank every volume shares the body. RULED 2026-08-07
    /// (§13.5): the full momentum exchange goes to the body owning the ENTRANCE surface, the first
    /// one physically touched; anything behind it is shoved through contact by the physics engine.
    /// So the caller routes the shove by
    /// [`ResolutionPlan::entrance`], not by these doses — they are the damage.
    pub deposits: Vec<Dose>,
    /// One per body exit the round reached, in order: every exit before a stop throws, and the body
    /// the round stops inside throws nothing.
    pub spall: Vec<SpallSource>,
    /// The transit crossing the plan resolved ([`DiscWalk::first_crossing`]), corridor-relative.
    pub crossing: DiscEvent,
}

/// The corridor [`finish`] was handed must be the corridor [`begin`] asked for; if it is, its
/// crossing ([`DiscWalk::first_crossing`]).
///
/// Between the two stages the caller runs a spatial query, and nothing in the type system stops it
/// from collecting along the UNBENT axis, from the wrong origin, with a re-derived frame, or around
/// a different contact entirely. Any of those silently resolves one surface's geometry against
/// another surface's entrance verdict. Tolerances are loose enough for honest f32 round-tripping and
/// far tighter than any of these mistakes.
fn validate_transit<'t>(
    transit: &'t DiscWalk,
    request: &TransitRequest,
) -> Result<Cow<'t, DiscEvent>, WalkError> {
    let mismatch = |reason| Err(WalkError::CorridorMismatch { reason });
    if (transit.axis - request.axis).length() > 1.0e-4 {
        return mismatch(Mismatch::Axis);
    }
    if transit.anchor != request.anchor {
        return mismatch(Mismatch::Anchor);
    }
    if (transit.origin - request.origin).length() > 1.0e-4 {
        return mismatch(Mismatch::Origin);
    }
    if (transit.frame.u - request.frame.u).length() > 1.0e-4
        || (transit.frame.v - request.frame.v).length() > 1.0e-4
    {
        return mismatch(Mismatch::Frame);
    }
    if (transit.radius - request.radius).abs() > 1.0e-6 {
        return mismatch(Mismatch::Radius);
    }
    if transit.samples() != request.samples {
        return mismatch(Mismatch::SampleCount);
    }

    // PER-SAMPLE geometry, not just the disc's. The handoff exports where each ray resumes, and a
    // caller that laid its ring out freshly — or nudged one origin — produces a corridor whose
    // aggregate axis, origin, frame and radius all still match while the rays themselves sample
    // somewhere else entirely. The seeds are only sound for the rays they were read on.
    for (index, seed) in request.seeds.iter().enumerate() {
        if (transit.offsets[index] - seed.offset).length() > SEED_OFFSET_TOLERANCE {
            return mismatch(Mismatch::SampleOffset);
        }
    }

    if transit.events.is_empty() {
        return Err(WalkError::BadCorridor {
            sample: 0,
            reason: "the transit corridor met no material after the entrance decided to enter",
        });
    }
    // Every sample starts at its hand-off, on the face it crossed or in air before the first face
    // on its ray, so the crossing the entrance decided to enter begins there.
    if transit.events[0].start > HANDOFF_PROXIMITY as f64 {
        return mismatch(Mismatch::NotAtHandOff);
    }
    let crossing = transit.crossing_of(
        transit
            .first_crossing()
            .iter()
            .flat_map(|&piece| transit.events[piece].runs.iter().copied())
            .collect(),
    )?;
    // IDENTITY, on the two things that actually name a crossing: the geometry it is made of, and
    // the surface it presents. Sharing an ENTITY is too coarse — one hull presents primitives
    // metres apart.
    let shares_primitive = crossing
        .primitives
        .iter()
        .any(|key| request.entrance_primitives.binary_search(key).is_ok());
    let same_surface = crossing.entry_normal.dot(request.entrance.normal) >= ENTRANCE_SURFACE_COS;
    if !(shares_primitive || same_surface) {
        return mismatch(Mismatch::NotTheEntrance);
    }
    Ok(crossing)
}

/// How far a supplied sample origin may sit from the one the handoff exported. Generous enough for
/// an honest f32 round-trip of a position through a corridor build, orders below any real re-aim.
const SEED_OFFSET_TOLERANCE: f32 = 1.0e-4;

/// How far into a transit corridor a crossing may begin and still begin at the hand-off. A sample
/// that hands off on a face re-measures it at zero; a millimetre covers the rounding of a face
/// position that was computed, transported and re-collected.
const HANDOFF_PROXIMITY: f32 = 1.0e-3;

/// Cosine bound for "the transit's crossing is on the surface the entrance read". Shares the
/// magnitude of the event-plane rule for the same reason: two reads of one surface patch.
const ENTRANCE_SURFACE_COS: f32 = 0.9;

/// Resolve the transit corridor into the plan (§3's perforate/embed, §5's spall, §6's deposits):
/// `mechanism` advances `state` through the crossing's ordered encounters until it stops or the
/// crossing ends. This is the one driver of a crossing's outcome, for the runtime and the §13.6 gate
/// alike.
pub(crate) fn finish<M: Mechanism>(
    transit: &DiscWalk,
    request: &TransitRequest,
    mechanism: &M,
    state: M::State,
    limits: &Limits,
    _laws: &WalkLaws,
) -> Result<ResolutionPlan<M::State>, WalkError> {
    let crossing = validate_transit(transit, request)?;
    let event = &*crossing;
    let geometry = Geometry {
        axis: transit.axis,
        entrance_normal: request.entrance.normal,
    };
    let advanced = if request.entrance.overmatched {
        advance_through(
            &OvermatchProjection { inner: mechanism },
            state,
            &event.encounters,
            limits,
            &geometry,
        )?
    } else {
        advance_through(mechanism, state, &event.encounters, limits, &geometry)?
    };
    let cost_spent =
        (mechanism.reference_mm(state) - mechanism.reference_mm(advanced.state)) as f32;

    let outcome = match advanced.stop {
        Some(t) => Outcome::Embedded {
            at: transit.origin + transit.axis * t as f32,
            t: t as f32,
        },
        None => Outcome::Perforated {
            exit: event.exit_position,
            exit_normal: event.exit_normal,
            direction: transit.axis,
            t: event.end as f32,
        },
    };

    let deposits = advanced
        .doses
        .into_iter()
        .filter(|(_, line)| line.chord as f32 > 0.0)
        .map(|(address, line)| Dose {
            address,
            amount: line.energy as f32,
            source: DoseSource::Line {
                chord: line.chord as f32,
                cost: line.cost as f32,
            },
        })
        .collect();

    let spall = spall_sources(
        mechanism,
        &event.exits,
        &advanced.stretches,
        advanced.stop,
        transit.axis,
        transit.samples(),
    );

    Ok(ResolutionPlan {
        entrance: request.entrance,
        outcome,
        state: advanced.state,
        cost_spent,
        deposits,
        spall,
        crossing: crossing.into_owned(),
    })
}

/// One volume's line over a crossing, summed over the encounters: the disc-mean chord (m), its
/// readout cost (reference-mm) and the energy the round lost in it (J), the dose.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LineSum {
    chord: f64,
    cost: f64,
    energy: f64,
}

/// Where a mechanism's run through a crossing ended.
struct Advanced<S> {
    state: S,
    /// Progress at which it came to rest; `None` when it crossed every encounter.
    stop: Option<f64>,
    /// Per addressed volume, its summed line.
    doses: BTreeMap<Entity, LineSum>,
    /// Every encounter the mechanism advanced through, in order.
    stretches: Vec<Stretch<S>>,
}

/// One encounter as the mechanism advanced through it: the progress it covered, the states either
/// side, and the factor each present sample was resisted by there.
struct Stretch<S> {
    start: f64,
    /// Where the advance left the round: the encounter's end, or the rest point.
    end: f64,
    before: S,
    after: S,
    /// `(sample, factor)` for every sample in the encounter ([`overlap_factor`]).
    resisted: Vec<(usize, f32)>,
}

/// The encounters a mechanism advances through: each run of encounters joined across no interface
/// is one ([`Encounter::absorb`]), so a seam inside one substance cannot change an outcome.
fn one_substance_runs(encounters: &[Encounter]) -> Vec<Cow<'_, Encounter>> {
    let mut out: Vec<Cow<'_, Encounter>> = Vec::with_capacity(encounters.len());
    for encounter in encounters {
        match out.last_mut() {
            Some(run) if !encounter.interface => run.to_mut().absorb(encounter),
            _ => out.push(Cow::Borrowed(encounter)),
        }
    }
    out
}

/// Advance `state` through `encounters` in order until the mechanism stops.
fn advance_through<M: Mechanism>(
    mechanism: &M,
    mut state: M::State,
    encounters: &[Encounter],
    limits: &Limits,
    geometry: &Geometry,
) -> Result<Advanced<M::State>, WalkError> {
    let mut doses: BTreeMap<Entity, LineSum> = BTreeMap::new();
    let mut stretches: Vec<Stretch<M::State>> = Vec::new();
    for encounter in one_substance_runs(encounters) {
        let encounter = &*encounter;
        let before = state;
        let advance = mechanism.advance(state, encounter, limits, geometry);
        state = advance.state;
        for effect in advance.effects {
            match effect {
                Effect::Line {
                    address,
                    chord,
                    cost,
                    energy,
                } => {
                    let slot = doses.entry(address).or_default();
                    slot.chord += chord;
                    slot.cost += cost;
                    slot.energy += energy;
                }
            }
        }
        let stop = advance.control == Control::Stop;
        stretches.push(Stretch {
            start: encounter.start,
            end: if stop {
                encounter.start + advance.consumed
            } else {
                encounter.end
            },
            before,
            after: state,
            resisted: encounter
                .samples
                .iter()
                .map(|sample| (sample.sample, overlap_factor(&sample.present)))
                .collect(),
        });
        match advance.control {
            Control::Continue => {}
            Control::Stop => {
                return Ok(Advanced {
                    state,
                    stop: Some(encounter.start + advance.consumed),
                    doses,
                    stretches,
                });
            }
            Control::Redirect { .. } | Control::Schedule => {
                return Err(WalkError::UnsupportedControl);
            }
        }
    }
    Ok(Advanced {
        state,
        stop: None,
        doses,
        stretches,
    })
}

/// The spall sources of the body exits the round reached ([`SpallSource`]).
///
/// A member is reached when its exit lies at or before the rest point; a body the round stops inside
/// throws nothing. Body boundaries are run ends or factor drops on their sample, each a change of
/// substance set, so they are interfaces and every stretch lies wholly inside or wholly outside a
/// member's body: a stretch belongs to it
/// when its midpoint does. A stretch's readout drop, and its energy drop alike, is shared among its
/// samples by the factor each was resisted by — the share the rigid force law gives each sample of
/// the disc-mean force.
fn spall_sources<M: Mechanism>(
    mechanism: &M,
    exits: &[BodyExit],
    stretches: &[Stretch<M::State>],
    stop: Option<f64>,
    direction: Vec3,
    // k: the disc's sample count.
    disc: usize,
) -> Vec<SpallSource> {
    // Each stretch's `(readout drop, energy drop)` over the whole disc.
    let disc_drops: Vec<(f64, f64)> = stretches
        .iter()
        .map(|stretch| {
            (
                mechanism.reference_mm(stretch.before) - mechanism.reference_mm(stretch.after),
                mechanism.energy(stretch.before) - mechanism.energy(stretch.after),
            )
        })
        .collect();
    let mut out = Vec::new();
    for exit in exits {
        let reached: Vec<&ExitMember> = exit
            .members
            .iter()
            .filter(|member| stop.is_none_or(|rest| member.end <= rest))
            .collect();
        if reached.is_empty() {
            continue;
        }
        let (mut spend, mut energy) = (0.0f64, 0.0f64);
        for member in &reached {
            for (stretch, (drop, lost)) in stretches.iter().zip(&disc_drops) {
                let mid = 0.5 * (stretch.start + stretch.end);
                if !(member.start <= mid && mid < member.end) {
                    continue;
                }
                let total: f64 = stretch.resisted.iter().map(|(_, f)| *f as f64).sum();
                let own = stretch
                    .resisted
                    .iter()
                    .find(|(sample, _)| *sample == member.sample)
                    .map_or(0.0, |(_, f)| *f as f64);
                if total > 0.0 {
                    spend += drop * own / total;
                    energy += lost * own / total;
                }
            }
        }
        let first = reached
            .iter()
            .map(|m| m.start)
            .fold(f64::INFINITY, f64::min);
        let last = reached
            .iter()
            .map(|m| m.end)
            .fold(f64::NEG_INFINITY, f64::max);
        let entry = stretches
            .iter()
            .find(|stretch| 0.5 * (stretch.start + stretch.end) >= first)
            .map(|stretch| stretch.before);
        let exit_state = stretches
            .iter()
            .rev()
            .find(|stretch| 0.5 * (stretch.start + stretch.end) < last)
            .map(|stretch| stretch.after);
        let (Some(entry), Some(exit_state)) = (entry, exit_state) else {
            continue;
        };
        let count = reached.len();
        let faces: Vec<Vec3> = reached.iter().filter_map(|m| m.face).collect();
        let face = match faces.as_slice() {
            [] => None,
            // One face IS the mean; normalising a unit vector would perturb its low bits.
            [one] => Some(*one),
            several => Some(unit_or_zero(several.iter().copied().sum())),
        };
        let exited: BTreeSet<Entity> = reached
            .iter()
            .flat_map(|m| m.exited.iter().copied())
            .collect();
        out.push(SpallSource {
            t: reached.iter().map(|m| m.end).sum::<f64>() / count as f64,
            position: reached.iter().map(|m| m.position).sum::<Vec3>() / count as f32,
            face,
            direction,
            substances: exit.substances.clone(),
            exited: exited.into_iter().collect(),
            spend: spend as f32,
            energy: energy as f32,
            debris_ratio: reached
                .iter()
                .map(|member| (member.rear_factor, member.debris_ratio))
                .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)))
                .map_or(0.0, |(_, ratio)| ratio),
            strength_ratio: exit_strength_ratio(
                reached
                    .iter()
                    .map(|member| member.rear_factor)
                    .fold(0.0, f32::max),
            ),
            entry_speed: mechanism.speed(entry) as f32,
            exit_speed: mechanism.speed(exit_state) as f32,
            coverage: (count as f64 / disc as f64) as f32,
        });
    }
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}
