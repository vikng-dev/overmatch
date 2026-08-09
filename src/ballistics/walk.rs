//! The §13 union field walk — the pure core.
//!
//! NOT YET WIRED. The live march still runs the serial resolver ([`super::resolve_armor_crossing`]);
//! slice 2 replaces it with this. Everything here is a pure function over pre-collected data: no
//! ECS, no spatial queries, no commands, no wall clock, no RNG. Those live outside (slice 2's Avian
//! adapter), which is what makes the laws testable to the bit.
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
//! # Staged, because normalization bends the axis
//!
//! Entry faces are sampled along the INCOMING axis, but the transit happens along the bent one (the
//! serial resolver bends at `ballistics.rs`'s `bend_toward` call before probing the exit). One
//! pre-collected hit list therefore cannot describe both. So the core is a two-stage continuation:
//! [`begin`] consumes the entrance disc and returns either a ricochet plan or a
//! [`TransitRequest`] naming the axis and frame the caller must collect the transit corridor along;
//! [`finish`] consumes that corridor and returns the [`ResolutionPlan`]. Slice-1 tests answer the
//! request from fixtures; slice 2 answers it with real casts.
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
//! [`WalkLaws::topology_abs`]/[`WalkLaws::topology_rel`] group numerically coincident face hits into
//! one boundary; [`WalkLaws::weld_perp`] is the §13.4 physical event-topology knob (~2 mm
//! perpendicular); [`WalkLaws::event_plane_tolerance`] is the lateral surface-patch relation the
//! disc uses to associate samples. They are three orders of magnitude apart and must never be
//! collapsed into one "epsilon" — a march/query offset in particular must never reach this module,
//! since the old marcher's 1 mm nudge alone would eat half the weld budget.

use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------------------------
// Input vocabulary
// ---------------------------------------------------------------------------------------------

/// One closed island of one ballistic volume — the unit presence is tracked on.
///
/// Primitive identity is load-bearing and cannot be replaced by entity identity (§13.4's
/// per-entity parity pairing is unsound the moment one mesh carries two overlapping islands: the
/// hit order reads `enter A, enter B, exit A, exit B`, and adjacent pairing produces
/// `[enter, enter]`, which is not material presence). Two coplanar triangles of ONE face must
/// collapse to one crossing; two different shells sharing an entry plane must stay distinct,
/// because their exits differ. Only primitive identity separates those cases.
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

/// One triangle crossing along a sample ray, as the spatial adapter reports it.
///
/// `true_normal` is the face's OUTWARD normal recovered from triangle winding — NOT parry's
/// reported normal, which is flipped to oppose the ray, so a backface read from inside is
/// indistinguishable from a head-on entry (§13.1). Recovering the true orientation is the adapter's
/// contract; this module classifies entry/exit from it and would silently invert the whole walk if
/// handed the flipped one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceHit {
    pub volume: Entity,
    pub primitive: Entity,
    /// Diagnostic identity only. It never participates in coincidence clustering or in the
    /// topological reduction (those are geometric, so a re-triangulated mesh cannot change the
    /// answer); it exists so a [`WalkError`] can name the offending faces.
    pub triangle: u32,
    /// Distance along the sample ray from the corridor origin.
    pub t: f32,
    pub true_normal: Vec3,
}

impl FaceHit {
    fn key(&self) -> PrimitiveKey {
        PrimitiveKey {
            volume: self.volume,
            primitive: self.primitive,
        }
    }
}

/// The factor of every volume the corridor can meet, supplied ONCE per volume.
///
/// Not per hit: a volume has one substance (§13.7), and repeating the scalar on every face invites
/// two faces of one plate disagreeing — an unrepresentable state that the union `max` would then
/// have to arbitrate.
#[derive(Clone, Debug, Default)]
pub struct VolumeTable {
    factors: BTreeMap<Entity, f32>,
}

impl VolumeTable {
    /// Build the table, rejecting factors that cannot participate in `max` or in a deterministic
    /// sum. A `NaN` poisons both the field maximum and the sort; a negative factor breaks
    /// monotonicity (§13.6). Zero is canonicalized so `-0.0` cannot produce a second bit pattern
    /// for "no material" and defeat the bit-equality coalescing in [`walk_ray`].
    pub fn new(entries: impl IntoIterator<Item = (Entity, f32)>) -> Result<Self, WalkError> {
        let mut factors = BTreeMap::new();
        for (volume, factor) in entries {
            if !factor.is_finite() || factor < 0.0 {
                return Err(WalkError::BadFactor { volume, factor });
            }
            factors.insert(volume, if factor == 0.0 { 0.0 } else { factor });
        }
        Ok(Self { factors })
    }

    /// Every `(volume, factor)` the table carries, in entity order.
    ///
    /// Exists for the §13.6 idempotence/monotonicity gate ([`super::fuzz`]), which re-walks a real
    /// corridor with one volume DUPLICATED and therefore has to state the same table plus one row.
    /// Reconstructing it from the world instead would prove the property about a table the walk was
    /// never handed.
    pub fn entries(&self) -> impl Iterator<Item = (Entity, f32)> + '_ {
        self.factors
            .iter()
            .map(|(volume, factor)| (*volume, *factor))
    }

    fn factor(&self, volume: Entity) -> Result<f32, WalkError> {
        self.factors
            .get(&volume)
            .copied()
            .ok_or(WalkError::UnknownVolume { volume })
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
    /// handoff 0.38 mm off the very face it was computed from (MEASURED by codex 2026-08-07 at
    /// `(2499.9, 924.963, 1524.939)`, 38° incidence) and let a real entry face be pruned in-envelope.
    /// Anchoring kills it at the source: the world-scale subtraction happens ONCE, on the anchor, and
    /// every relationship the walk cares about is then arithmetic on small numbers.
    pub anchor: Vec3,
    /// Corridor start, RELATIVE to [`anchor`](Self::anchor).
    pub origin: Vec3,
    /// Unit travel direction.
    pub axis: Vec3,
    pub length: f32,
    /// Primitives the corridor origin is ALREADY inside, declared by the caller.
    ///
    /// Never inferred. "The first hit is an exit, so we must have started inside" silently converts
    /// a dropped entry face into legitimate topology — which is how a hole in a mesh becomes free
    /// armour. An exit with neither declared presence nor a prior entry is an error.
    pub initial_presence: Vec<PrimitiveKey>,
    pub hits: Vec<FaceHit>,
}

// ---------------------------------------------------------------------------------------------
// Laws / knobs
// ---------------------------------------------------------------------------------------------

/// Every tunable the walk consumes. Sandbox knobs (§4: "the structure is the design; the magnitudes
/// are live knobs"), gathered so no law reads a hidden constant.
#[derive(Clone, Copy, Debug)]
pub struct WalkLaws {
    /// TOPOLOGY domain — absolute floor (m) under which two face hits are the same boundary.
    pub topology_abs: f32,
    /// TOPOLOGY domain — relative term, so a corridor anchored far downrange (where f32 spacing is
    /// coarser) still groups the two triangles of one face diagonal.
    pub topology_rel: f32,
    /// TOPOLOGY domain — how many coincidence windows ONE cluster may span in total.
    ///
    /// Coincidence chains from face to face, because the surfaces meeting at one geometric point are
    /// each computed from their own triangle's plane and so spread over several ULP: MEASURED on the
    /// bound Tiger, a four-face corner spread 4.8 µm against a 4.4 µm window (1.09 windows). A chain
    /// with no ceiling is single-linkage and unbounded, and a cluster is a claim that its faces are
    /// ONE boundary — so the chain is capped, and the cap is above the widest corner measured and
    /// well below any authored feature.
    ///
    /// It is not a safety knob. What a cluster's width can cost is bounded by the reduction's
    /// direction (entries open at the cluster's first face, exits close at its last), so widening
    /// this over-charges and never erases.
    pub topology_cluster_windows: f32,
    /// A face is tangent — and toggles nothing — when `|axis · n|` is at most this. A tangent face
    /// bounds zero material, so refusing to toggle on it cannot lose armour.
    pub tangent_cos: f32,
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
    /// collapse into one run with one terminal spall budget by chaining.
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
    /// DIMENSIONS, EXPLICITLY, because getting this wrong is exactly what round 4 caught. The
    /// predicted separation of two samples is `−(d·n̄)/(axis·n̄)`, whose worst case over a disc is
    /// `|d| = 2r` with `d` up the plane's steepest line: `2·r·sin(i)·sec(i) = 2·r·tan(i)`. Capping
    /// the SECANT at `C` therefore caps the reach at `2·r·C·sin(i) ≤ 2·r·C` — that is `C` DIAMETERS,
    /// not `C` radii. The previous formulation capped `2·tan` itself, which made the same `10` mean
    /// ten RADII, five diameters, and split a valid ring-only same-plane contact in two.
    ///
    /// GENEROUS, deliberately. Its only job is numerical: the division blows up as the ray goes
    /// parallel to the surface. Below it the relation is EXACT, and a cap that engages anywhere a
    /// valid crossing can happen does not bound the geometry, it mispredicts it — which is precisely
    /// the defect round 4 caught. A hundred engages past 89.43°, an order beyond any incidence at
    /// which a round both declines to ricochet and still presents a resolvable chord; and the module
    /// already refuses to toggle on faces within `tangent_cos` of edge-on, whose secant would be
    /// 10 000. The reach it implies is never the operative bound — the residual is.
    pub event_secant_cap: f32,
    /// Past this incidence (rad, from the surface normal) an un-overmatched round ricochets.
    pub ricochet_angle: f32,
    /// Speed retained through a FULLY covered ricochet. Partial coverage bleeds proportionally less
    /// (§13.5: "a graze IS a partial ricochet").
    pub ricochet_bleed: f32,
    /// Share of the incidence angle the round straightens toward the normal as it bites in.
    pub normalization: f32,
    /// Overmatch when caliber ≥ this × the crossing's factor-weighted perpendicular thickness.
    pub overmatch_ratio: f32,
    /// Reference-mm per metre of the reference substance (RHA), the divisor turning a cost back
    /// into steel-equivalent METRES so it can be compared against a caliber.
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
            // ~3 f32 ULP. Deliberately tiny: the corridor is ORIGIN-RELATIVE (slice 2 anchors it at
            // first contact), so `t` stays in metres and this term only has to cover intersection
            // rounding — a relative term large enough to matter at world scale would swallow the
            // millimetre distinction the weld tolerance is written in.
            topology_rel: 4.0e-7,
            topology_cluster_windows: 2.0,
            tangent_cos: 1.0e-4,
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
            ricochet_bleed: 0.6,
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
    /// A hit named a volume the table does not carry — a bind gap, never a defaultable factor.
    UnknownVolume { volume: Entity },
    /// The corridor itself is malformed (non-unit axis, non-finite length, a hit behind the origin).
    BadCorridor { sample: usize, reason: &'static str },
    /// An exit face for a primitive the walk is not inside: a dropped entry, an inverted winding, or
    /// an undeclared start-inside. NOT repaired by inventing an interval from the corridor origin.
    UnexpectedExit {
        sample: usize,
        key: PrimitiveKey,
        t: f32,
        triangles: Vec<u32>,
    },
    /// An entry face for a primitive the walk is already inside — a self-overlapping island, which
    /// means the mesh is not the closed positively-oriented shell the bake gate promises.
    UnexpectedEntry {
        sample: usize,
        key: PrimitiveKey,
        t: f32,
        triangles: Vec<u32>,
    },
    /// The corridor ran out with material still open. The caller must extend the corridor until the
    /// crossing closes (§13.4's atomic resolution corridor); the core will not synthesize the exit.
    IncompleteCorridor {
        sample: usize,
        open: Vec<PrimitiveKey>,
        length: f32,
    },
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
    CorridorMismatch { reason: &'static str },
    /// The covered samples' entry normals cancelled, so no aggregate `n̄` exists.
    ///
    /// Defensive: the tangent gate ([`WalkLaws::tangent_cos`]) admits a face as an ENTRY only when
    /// `axis · n < -tangent_cos`, so every contributing normal leans against the axis and their sum
    /// cannot vanish — `n̄` is well-defined by construction, which is precisely §13.5's repair of
    /// the point model's degenerate normal. The arm exists so that a future zeroed tangent gate
    /// fails loud instead of normalizing a zero vector.
    DegenerateEntryNormal { coverage: f32 },
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
    pub start: f32,
    pub end: f32,
    pub factor: f32,
}

impl Span {
    fn len(&self) -> f64 {
        self.end as f64 - self.start as f64
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
    pub start: f32,
    pub end: f32,
    pub factor: f32,
    pub material: f32,
    pub cost: f32,
}

/// A contiguous crossing after ε-welding: one entrance, one exit, and the material steps between.
#[derive(Clone, Debug, PartialEq)]
pub struct WeldedRun {
    pub start: f32,
    pub end: f32,
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

/// One field transition. Spall fires wherever the step is DOWNWARD, including the exit (§13.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundaryEvent {
    pub kind: BoundaryKind,
    pub t: f32,
    pub position: Vec3,
    pub normal: Vec3,
    pub factor_before: f32,
    pub factor_after: f32,
    /// §5's body term: the cost of the constant-factor stretch just exited. Zero unless the step is
    /// downward. For a plain single-material plate this is the whole plate — identical to what the
    /// serial resolver hands the spall cone today; for an equal-factor welded sandwich it is the
    /// SUMMED cost, matching §13.4's "budget = the run's summed cost".
    pub spall_budget: f32,
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
    pub spans: Vec<(f32, f32)>,
}

/// One primitive's presence intervals along a sample ray.
///
/// Kept alongside the per-ENTITY union because the staged handoff needs primitive identity: after
/// normalization bends the axis and transports the ring, a transit sample can begin INSIDE material,
/// and [`RayCorridor::initial_presence`] must be told which primitives — inference is forbidden
/// (that is how a dropped entry face becomes free armour).
#[derive(Clone, Debug, PartialEq)]
pub struct PrimitivePresence {
    pub key: PrimitiveKey,
    /// Disjoint, ordered, half-open `[open, close)`.
    pub spans: Vec<(f32, f32)>,
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
    /// Per-primitive presence — the seed state the staged handoff exports.
    pub primitives: Vec<PrimitivePresence>,
    pub events: Vec<BoundaryEvent>,
}

impl RayWalk {
    /// Which primitives a corridor RESTARTED at progress `t` would have to declare as initial
    /// presence.
    ///
    /// The interval is `(open, close]` — the entry face must be STRICTLY behind the restart. A face
    /// sitting on the handoff plane arrives in the new corridor's own hit list at `t = 0` and is
    /// processed there, so seeding it as well would double-count the entry
    /// ([`WalkError::UnexpectedEntry`]). An exit sitting on the plane, conversely, must be seeded or
    /// it has nothing to pair with.
    ///
    /// "ON the plane" is a TOLERANCE, not an equality, and that is the whole reason this takes laws.
    /// The restart `t` is computed from the aggregate entrance plane while each span's boundary came
    /// from that sample's own ray, so the two agree only to within rounding — at oblique incidence,
    /// where the ring's crossings spread along the ray, they were measured 1.4e-8 apart. Read as an
    /// exact comparison, that hair decides between "the seed owns this entry" and "the corridor
    /// does", and getting it wrong is `UnexpectedEntry` and a round stopped dead on a plate it
    /// should have crossed. [`coincident`] is the module's existing answer to "do these two `t` name
    /// one boundary", so it is the one used here.
    pub fn inside_at(&self, t: f32, laws: &WalkLaws) -> Vec<PrimitiveKey> {
        self.primitives
            .iter()
            .filter(|presence| {
                presence.spans.iter().any(|(open, close)| {
                    let entered = *open < t && !coincident(*open, t, laws);
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
pub(crate) fn coincident(anchor: f32, t: f32, laws: &WalkLaws) -> bool {
    (t - anchor).abs() <= laws.topology_abs + laws.topology_rel * anchor.abs().max(t.abs())
}

/// Whether `t` is still inside the widest span ONE cluster may cover, measured from its anchor.
///
/// Coincidence chains, so without a ceiling a cluster grows for as long as faces keep arriving
/// within a window of each other, and a cluster is a claim that its faces name ONE boundary.
fn within_cluster_span(anchor: f32, t: f32, laws: &WalkLaws) -> bool {
    (t - anchor).abs()
        <= laws.topology_cluster_windows
            * (laws.topology_abs + laws.topology_rel * anchor.abs().max(t.abs()))
}

/// What one primitive's faces in one coincident cluster mean.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Toggle {
    Enter,
    Exit,
    /// Entry AND exit faces of ONE primitive inside one cluster, at `t` values that are not
    /// bit-equal: a shell thinner than the window, not a point contact. It bounds real material —
    /// `(last − first) · factor` of it — and that material is charged across the cluster.
    Graze,
    /// Mixed entry and exit faces at ONE bit-equal `t`, or nothing but tangents: a zero-measure
    /// touch. It must not toggle presence — fabricating a zero-length interval here is how an edge
    /// graze grows a spurious entrance/exit event pair out of no material at all.
    Touch,
}

/// One primitive's faces inside one coincident cluster, as the reduction reads them.
#[derive(Default)]
struct ClusterFaces {
    has_entry: bool,
    has_exit: bool,
    triangles: Vec<u32>,
    entry_sum: Vec3,
    exit_sum: Vec3,
    /// First and last `t` among the faces that TOGGLE. Tangent faces bound no material, so they are
    /// not allowed to widen a graze into a charge.
    span: Option<(f32, f32)>,
}

impl ClusterFaces {
    fn widen(&mut self, t: f32) {
        self.span = Some(match self.span {
            None => (t, t),
            Some((first, last)) => (first.min(t), last.max(t)),
        });
    }

    /// Whether this primitive's own toggling faces name ONE point exactly.
    ///
    /// Bit equality, not tolerance: tolerance is what the cluster already applied to decide these
    /// faces are one boundary, and applying it twice is how a positive-volume shell becomes a point.
    fn is_zero_measure(&self) -> bool {
        self.span
            .is_none_or(|(first, last)| first.to_bits() == last.to_bits())
    }
}

/// The live union field: which primitives are open, and therefore what `max(factor)` is.
///
/// Openness is a DEPTH, not a flag, because §13.7 legalizes several closed shells inside one
/// primitive — the road wheels are the standing precedent, with bodies and axle authored as one
/// MildSteel primitive. A ray through one of those meets `enter, enter, exit, exit`, and a boolean
/// pairing reads the second entry as a topology error. The §13.6 fuzzer measured 0.47% of a million
/// rays failing closed on exactly that shape: the sixteen wheels, `Hull_Rear` and `Turret_Cupola`.
///
/// Depth changes nothing about the field's meaning. Presence is `depth > 0`, so the interval a
/// primitive contributes is the UNION of its shells, and §13.2 takes `max(factor)` over whatever is
/// present — shell multiplicity inside one primitive charges exactly once, which is the same answer
/// the per-ENTITY union already gives for two overlapping primitives of one volume. It is also why
/// §13.6's idempotence still holds: a duplicated shell coincides face-for-face with the original, so
/// the topology reduction collapses it to one toggle before the field ever sees it.
struct Field {
    /// Per primitive: how many shells the ray is currently inside, and where the OUTERMOST one
    /// opened — the start of the union interval this primitive will contribute.
    open: BTreeMap<PrimitiveKey, (u32, f32)>,
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
        .filter(|hit| hit.t < corridor.length)
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
            .then(a.triangle.cmp(&b.triangle))
    });

    let mut field = Field {
        open: BTreeMap::new(),
        per_entity: BTreeMap::new(),
    };
    for key in &corridor.initial_presence {
        // Declared presence opens at depth ONE, whatever nesting produced it upstream: the seed says
        // the ray is inside this primitive, and the exits it will meet close that presence from the
        // inside out.
        if field.open.insert(*key, (1, 0.0)).is_none() {
            *field.per_entity.entry(key.volume).or_insert(0) += 1;
        }
        volumes.factor(key.volume)?;
    }

    // Per-primitive presence intervals, in close order.
    let mut intervals: Vec<(PrimitiveKey, f32, f32)> = Vec::new();
    // Field transitions: (t, factor_before, factor_after, entry_normal, exit_normal).
    struct Transition {
        t: f32,
        before: f32,
        after: f32,
        entry_normal: Option<Vec3>,
        exit_normal: Option<Vec3>,
    }
    let mut transitions: Vec<Transition> = Vec::new();

    let mut i = 0usize;
    while i < order.len() {
        let anchor = order[i].t;
        // Coincidence CHAINS, under two ceilings.
        //
        // Chaining, because the faces meeting at ONE geometric point do not share a `t`: each is
        // computed from its own triangle's plane, so a corner where four surfaces meet spreads its
        // four `t` over several ULP. Anchoring the window on the first of them puts the last one
        // outside it whenever that spread exceeds the window — the cluster splits mid-corner, one
        // primitive's entry lands in the next cluster, and its exit is left facing a depth that has
        // not opened yet.
        //
        // Chaining alone is single-linkage, which is transitive and therefore unbounded: one face
        // every few microns merges boundaries arbitrarily far apart. Two ceilings bound it.
        //
        // TOTAL SPAN — a cluster claims its faces are one boundary, so it may not reach further than
        // `topology_cluster_windows` windows from its anchor, whatever bridges the gap.
        //
        // ONE PRIMITIVE'S OWN FACES — a face may only join a cluster that already holds a face of
        // the SAME primitive if the two are pairwise coincident. A primitive reads entry-AND-exit in
        // one cluster only when its own two faces name one point, which is a graze; a bridging face
        // belonging to anything else can no longer merge an entry with an exit that are a real
        // traversal apart, and so can no longer reduce a plate to a no-op.
        let mut j = i + 1;
        while j < order.len() && coincident(order[j - 1].t, order[j].t, laws) {
            let t = order[j].t;
            if !within_cluster_span(anchor, t, laws) {
                break;
            }
            let own = order[i..j]
                .iter()
                .find(|hit| hit.key() == order[j].key())
                .map(|hit| hit.t);
            if own.is_some_and(|own| !coincident(own, t, laws)) {
                break;
            }
            j += 1;
        }
        let cluster = &order[i..j];
        i = j;

        // Reduce the cluster to at most one toggle per primitive.
        let mut per_primitive: BTreeMap<PrimitiveKey, ClusterFaces> = BTreeMap::new();
        for hit in cluster {
            let d = corridor.axis.dot(hit.true_normal);
            let slot = per_primitive.entry(hit.key()).or_default();
            slot.triangles.push(hit.triangle);
            if d < -laws.tangent_cos {
                slot.has_entry = true;
                slot.entry_sum += hit.true_normal;
                slot.widen(hit.t);
            } else if d > laws.tangent_cos {
                slot.has_exit = true;
                slot.exit_sum += hit.true_normal;
                slot.widen(hit.t);
            }
        }

        let mut toggles: Vec<(PrimitiveKey, Toggle, Vec<u32>, Vec3, Vec3)> = Vec::new();
        for (key, faces) in per_primitive {
            let toggle = match (faces.has_entry, faces.has_exit) {
                (true, false) => Toggle::Enter,
                (false, true) => Toggle::Exit,
                // A THIN SHELL IS NOT A POINT GRAZE. One primitive reading entry-AND-exit inside one
                // cluster is either a corner the ray only brushes, or a plate thinner than the
                // window — and the two are indistinguishable from the tolerance that grouped them.
                // Only the `t` values themselves separate them, so only bit equality may reduce the
                // pair to nothing.
                (true, true) if !faces.is_zero_measure() => Toggle::Graze,
                _ => Toggle::Touch,
            };
            toggles.push((
                key,
                toggle,
                faces.triangles,
                faces.entry_sum,
                faces.exit_sum,
            ));
        }

        // The whole batch is applied atomically: read the field, apply every exit AND entry, read it
        // again, emit at most one transition. Entity or triangle order cannot influence the result.
        let before = field.max_factor(volumes)?;
        let mut entry_normal = Vec3::ZERO;
        let mut exit_normal = Vec3::ZERO;
        let mut any_entry = false;
        let mut any_exit = false;
        // A CLUSTER COLLAPSES IN THE CONSERVATIVE DIRECTION. Material appears at the FIRST face of
        // the cluster and disappears at its LAST, so the factor charged anywhere inside a cluster is
        // `max(before, after, graze)` and dominates the field's true value there. Collapsing both
        // onto one end would charge `before` past an entry, or `after` before an exit, and either
        // reading is a stretch of armour up to the cluster's own width that the walk declines to
        // charge.
        //
        // DOMINANCE. A primitive present anywhere strictly inside the cluster either was open
        // before it, is open after it, or opened AND closed inside it — and the third case is
        // exactly `Toggle::Graze`, whose factor joins the maximum. There is no fourth case: a
        // cluster holds only the faces at its own `t`. So the level charged across the cluster is an
        // upper bound on `max(factor)` everywhere in it, and the only thing declined is a pair whose
        // two `t` are bit-equal, which bounds no length to charge.
        let opens_at = anchor;
        let closes_at = cluster.last().map_or(anchor, |hit| hit.t);
        // The highest factor GRAZING this cluster: present inside it, gone by its far side.
        let mut graze = 0.0f32;

        for (key, toggle, triangles, entry_sum, exit_sum) in toggles {
            match toggle {
                Toggle::Enter => {
                    volumes.factor(key.volume)?;
                    match field.open.get_mut(&key) {
                        // A further shell of a primitive the ray is already inside (§13.7). Presence
                        // does not change — it is already present — so no interval opens and the
                        // entity count does not move; only the depth that must be unwound.
                        Some((depth, _)) => *depth += 1,
                        None => {
                            field.open.insert(key, (1, opens_at));
                            *field.per_entity.entry(key.volume).or_insert(0) += 1;
                        }
                    }
                    entry_normal += entry_sum;
                    any_entry = true;
                }
                Toggle::Exit => {
                    // FAIL-LOUD SURVIVES. Balanced nesting is legal; an exit with no shell open is
                    // still a dropped entry, an inverted winding or a hole in the mesh, and still an
                    // error. What changed is only that "already inside" stopped being a contradiction.
                    let Some((depth, open_at)) = field.open.get_mut(&key) else {
                        return Err(WalkError::UnexpectedExit {
                            sample,
                            key,
                            t: closes_at,
                            triangles,
                        });
                    };
                    *depth -= 1;
                    if *depth > 0 {
                        // Out of an inner shell and still inside the primitive: presence is
                        // unbroken, the union interval stays open, and this face is NOT a field
                        // boundary — so it must not contribute to the boundary normal either.
                        continue;
                    }
                    let open_at = *open_at;
                    field.open.remove(&key);
                    if let Some(count) = field.per_entity.get_mut(&key.volume) {
                        *count -= 1;
                    }
                    intervals.push((key, open_at, closes_at));
                    exit_normal += exit_sum;
                    any_exit = true;
                }
                Toggle::Graze => {
                    // A further shell of a primitive the ray is ALREADY inside (§13.7) charges
                    // nothing new: its presence is open across the whole cluster and its factor is
                    // already in both `before` and `after`.
                    if !field.open.contains_key(&key) {
                        // The shell's own faces bound the charge, and its presence is REPORTED: a
                        // charged stretch with no primitive behind it would reach the run with no
                        // entry volume to name. The interval is the cluster's, not the pair's,
                        // because that is the stretch the field level below is applied over — wider
                        // than the pair by at most the cluster's own width, and never narrower.
                        graze = graze.max(volumes.factor(key.volume)?);
                        intervals.push((key, opens_at, closes_at));
                        entry_normal += entry_sum;
                        exit_normal += exit_sum;
                        any_entry = true;
                        any_exit = true;
                    }
                }
                Toggle::Touch => {}
            }
        }

        let after = field.max_factor(volumes)?;
        // The level held ACROSS the cluster, and the two steps that bracket it. With no graze this
        // is `max(before, after)` reached in one step — bit-for-bit the reduction that shipped:
        // a rise lands on the cluster's first face, a fall on its last, and an unchanged field
        // emits nothing. A graze is the case that needs both steps at once.
        let level = before.max(after).max(graze);
        let entry_normal = any_entry.then_some(entry_normal);
        let exit_normal = any_exit.then_some(exit_normal);
        if level.to_bits() != before.to_bits() {
            transitions.push(Transition {
                t: opens_at,
                before,
                after: level,
                entry_normal,
                exit_normal,
            });
        }
        if after.to_bits() != level.to_bits() {
            transitions.push(Transition {
                t: closes_at,
                before: level,
                after,
                entry_normal,
                exit_normal,
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

    // Canonical spans. Cut ONLY at factor changes, so equal-factor abutment and one thick plate
    // produce the identical span list and therefore the identical cost bits.
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor = 0.0f32;
    let mut factor = if transitions.is_empty() {
        // No steps at all: either empty air, or the corridor lies wholly inside declared presence.
        field_start_factor(corridor, volumes)?
    } else {
        transitions[0].before
    };
    for transition in &transitions {
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
    if corridor.length > cursor {
        spans.push(Span {
            start: cursor,
            end: corridor.length,
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

    // Runs: maximal stretches of factor > 0, as index ranges into `spans`.
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

    // Boundary normals, looked up by the transition that opens/closes each run.
    let normal_at = |t: f32, entry: bool| -> Vec3 {
        // Several transitions can share one `t`: a cluster that ends between two faces at the same
        // distance leaves a step on each side of the split. The normal is taken from whichever of
        // them carries the face this boundary is asking for.
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

    let runs = weld_runs(corridor, &spans, &raw_runs, &intervals, &normal_at, laws);
    let events = boundary_events(corridor, &runs);
    let presence = entity_presence(&intervals, volumes)?;

    let mut by_primitive: BTreeMap<PrimitiveKey, Vec<(f32, f32)>> = BTreeMap::new();
    for (key, open, close) in &intervals {
        by_primitive.entry(*key).or_default().push((*open, *close));
    }
    let primitives = by_primitive
        .into_iter()
        .map(|(key, mut spans)| {
            spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            PrimitivePresence { key, spans }
        })
        .collect();

    Ok(RayWalk {
        spans,
        runs,
        cost,
        presence,
        primitives,
        events,
    })
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
    corridor: &RayCorridor,
    spans: &[Span],
    raw_runs: &[(usize, usize)],
    intervals: &[(PrimitiveKey, f32, f32)],
    normal_at: &dyn Fn(f32, bool) -> Vec3,
    laws: &WalkLaws,
) -> Vec<WeldedRun> {
    let mut welded: Vec<WeldedRun> = Vec::new();
    let mut group_start = 0usize;
    let mut gap_budget = 0.0f32;

    let flush = |from: usize, to: usize| {
        let start = spans[raw_runs[from].0].start;
        let end = spans[raw_runs[to].1 - 1].end;
        let mut segments: Vec<MaterialSegment> = Vec::new();
        let mut cost_acc = 0.0f64;
        for run in &raw_runs[from..=to] {
            for span in &spans[run.0..run.1] {
                cost_acc += span.cost();
                match segments.last_mut() {
                    // Coalesce across a welded void so an equal-factor sandwich reads as ONE
                    // stretch — that is what makes the welded exit's spall budget the SUMMED cost
                    // (§13.4) without any special case.
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
            .map(|(key, _, _)| *key)
            .collect();
        primitives.sort();
        primitives.dedup();
        let entry_volume = intervals
            .iter()
            .filter(|(_, open, _)| *open == start)
            .map(|(key, _, _)| key.volume)
            .min()
            .unwrap_or_else(|| primitives.first().map_or(Entity::PLACEHOLDER, |k| k.volume));
        WeldedRun {
            start,
            end,
            cost: cost_acc as f32,
            entry_normal: normal_at(start, true),
            exit_normal: normal_at(end, false),
            entry_volume,
            segments,
            joints: (to - from) as u32,
            primitives,
        }
    };

    for index in 0..raw_runs.len() {
        if index + 1 == raw_runs.len() {
            welded.push(flush(group_start, index));
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
        let gap_perp = gap_along * projection;
        let opposing = n_exit.dot(n_entry) <= -laws.weld_face_cos;
        let weld = opposing
            && gap_along <= laws.weld_max_lookahead
            && gap_perp <= laws.weld_perp
            && gap_budget + gap_perp <= laws.weld_run_gap_budget;

        if weld {
            gap_budget += gap_perp;
        } else {
            welded.push(flush(group_start, index));
            group_start = index + 1;
            gap_budget = 0.0;
        }
    }

    welded
}

/// Turn each welded run's material segments into the ordered field transitions the consequence step
/// consumes. Spall fires at every DOWNWARD step (§13.2's field law), including the exit.
///
/// RULED 2026-08-07 (§13.4, superseding its own original "spall once per welded run", which
/// contradicted §13.2): welding deletes only the void's exit/entry face pair, and the direct
/// material step it exposes still obeys the field law. A welded `RHA | 0.4 mm air | Cast` joint
/// therefore spalls with its own budget; an equal-factor joint emits nothing at all, which is where
/// the common case still reads as "one exit". A 0.4 mm void never "develops" spall of its own —
/// voids have no events, material steps do.
fn boundary_events(corridor: &RayCorridor, runs: &[WeldedRun]) -> Vec<BoundaryEvent> {
    let mut events = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let at = |t: f32| corridor.origin + corridor.axis * t;
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
            spall_budget: 0.0,
            welded: false,
            run: index,
        });
        for pair in run.segments.windows(2) {
            let (before, after) = (pair[0], pair[1]);
            let downward = after.factor < before.factor;
            events.push(BoundaryEvent {
                kind: BoundaryKind::Step,
                t: before.end,
                position: at(before.end),
                normal: corridor.axis,
                factor_before: before.factor,
                factor_after: after.factor,
                spall_budget: if downward { before.cost } else { 0.0 },
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
            spall_budget: last.cost,
            welded: false,
            run: index,
        });
    }
    events
}

/// Union each entity's primitive intervals, then charge its own chord at its own factor (§13.2's
/// damage law: no ownership, no priority, no argmax).
fn entity_presence(
    intervals: &[(PrimitiveKey, f32, f32)],
    volumes: &VolumeTable,
) -> Result<Vec<EntityPresence>, WalkError> {
    let mut by_entity: BTreeMap<Entity, Vec<(f32, f32)>> = BTreeMap::new();
    for (key, open, close) in intervals {
        by_entity
            .entry(key.volume)
            .or_default()
            .push((*open, *close));
    }
    let mut out = Vec::new();
    for (entity, mut spans) in by_entity {
        spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for span in spans {
            match merged.last_mut() {
                Some(last) if span.0 <= last.1 => last.1 = last.1.max(span.1),
                _ => merged.push(span),
            }
        }
        let factor = volumes.factor(entity)?;
        let chord: f64 = merged.iter().map(|(a, b)| *b as f64 - *a as f64).sum();
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
/// coverage, not exact geometric area (§13.5's own note: k is the resolution dial).
pub fn disc_offsets(frame: &DiscFrame, radius: f32, ring: usize) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(ring + 1);
    out.push(Vec3::ZERO);
    for index in 0..ring {
        let angle = std::f32::consts::TAU * (index as f32) / (ring as f32);
        out.push((frame.u * angle.cos() + frame.v * angle.sin()) * radius);
    }
    out
}

/// DERIVED from §13.5's `k ≈ 8–16`: the axis plus a twelve-sample ring.
pub const DEFAULT_RING: usize = 12;

/// One sample ray of a disc corridor.
#[derive(Clone, Debug, Default)]
pub struct SampleCorridor {
    pub offset: Vec3,
    pub initial_presence: Vec<PrimitiveKey>,
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

/// A downward field step aggregated over the disc — one spall source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiscSpall {
    pub t: f32,
    pub position: Vec3,
    pub normal: Vec3,
    pub from_factor: f32,
    pub to_factor: f32,
    /// §5's body term, already η-weighted: the contributing samples' budgets divided by k.
    pub budget: f32,
    /// The fraction of the disc that saw THIS step — entrance, internal-step and exit coverage are
    /// different subsets and are not interchangeable (they describe different faces).
    pub coverage: f32,
}

/// The disc-mean cumulative cost as a function of longitudinal progress.
///
/// Kept because embedding cannot use the serial resolver's uniform `span × cap/cost`: with several
/// factors along the corridor the cost is not linear in progress, so the embed point is the
/// INVERSE of this prefix integral, and per-entity transit damage is clipped at the same progress
/// rather than scaled proportionally.
#[derive(Clone, Debug, PartialEq)]
pub struct CostProfile {
    pub ts: Vec<f32>,
    pub cumulative: Vec<f64>,
}

impl CostProfile {
    pub fn total(&self) -> f64 {
        self.cumulative.last().copied().unwrap_or(0.0)
    }

    /// The progress `t` at which the accumulated cost first reaches `budget`, or `None` when the
    /// whole profile costs less than that (i.e. the round perforates).
    pub fn invert(&self, budget: f64) -> Option<f32> {
        if budget >= self.total() {
            return None;
        }
        for index in 1..self.ts.len() {
            if self.cumulative[index] > budget {
                let slice = self.cumulative[index] - self.cumulative[index - 1];
                if slice <= 0.0 {
                    return Some(self.ts[index - 1]);
                }
                let share = (budget - self.cumulative[index - 1]) / slice;
                let a = self.ts[index - 1] as f64;
                let b = self.ts[index] as f64;
                return Some((a + (b - a) * share) as f32);
            }
        }
        self.ts.last().copied()
    }

    /// Scale the whole profile — how overmatch charges the perpendicular projection instead of the
    /// oblique chord without breaking the inversion (§4: overmatch cancels the slope cost).
    fn scaled(&self, factor: f64) -> Self {
        Self {
            ts: self.ts.clone(),
            cumulative: self.cumulative.iter().map(|c| c * factor).collect(),
        }
    }
}

/// One entity's share of a disc crossing — the damage deposit AND the material slice-2 needs to
/// decide impulse ownership.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityShare {
    pub entity: Entity,
    pub factor: f32,
    /// Disc-mean chord (m): the entity's covered chords summed over samples, divided by k.
    pub chord: f32,
    /// Disc-mean cost (reference-mm) — `chord × factor`, §13.2's damage law.
    pub cost: f32,
    /// Fraction of the disc that met this entity at all.
    pub coverage: f32,
    /// Per-sample presence spans, kept only to clip at an embed. Private detail of the plan.
    spans: Vec<(f32, f32)>,
}

/// One crossing of the world by the disc: the cluster of sample runs that belong to the same
/// physical surface (§13.5).
#[derive(Clone, Debug, PartialEq)]
pub struct DiscEvent {
    pub start: f32,
    pub end: f32,
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
    /// Every primitive this crossing touched, sorted — the identity half of the transit check.
    pub primitives: Vec<PrimitiveKey>,
    /// `(1/k) Σᵢ costᵢ` — automatically `η × mean covered chord cost`.
    pub cost: f32,
    pub profile: CostProfile,
    pub spall: Vec<DiscSpall>,
    pub shares: Vec<EntityShare>,
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
    /// The per-sample walks, RETAINED. The staged handoff has to export which primitives each
    /// sample is inside at the transit plane, and neither the events nor the per-entity presence
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

    /// The seed state a corridor RESUMED at progress `t` would need — the same lookup [`begin`] makes
    /// at the handoff, on a plane square to the axis instead of on the entrance surface.
    ///
    /// This is what lets the march step out of one crossing and into the next without ever inferring
    /// its own interior state: a shell that perforates an outer plate while a ring sample is already
    /// inside the crewman behind it resumes KNOWING that, because the ray it would have to guess
    /// about is one this walk already covered.
    pub fn resume_at(&self, t: f32, laws: &WalkLaws) -> Vec<SampleSeed> {
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
                .max(0.0);
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
                let pa =
                    corridor.origin + corridor.samples[sa].offset + corridor.axis * run_a.start;
                let pb =
                    corridor.origin + corridor.samples[sb].offset + corridor.axis * run_b.start;
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

    let mut clusters: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for node in 0..nodes.len() {
        let root = find(&mut parent, node);
        clusters.entry(root).or_default().push(node);
    }

    let mut events: Vec<DiscEvent> = Vec::new();
    for members in clusters.values() {
        events.push(build_event(corridor, &walks, &nodes, members, k, laws)?);
    }
    events.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));

    Ok(DiscWalk {
        axis: corridor.axis,
        anchor: corridor.anchor,
        origin: corridor.origin,
        frame: corridor.frame,
        radius: corridor.radius,
        offsets: corridor.samples.iter().map(|s| s.offset).collect(),
        walks,
        events,
    })
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
/// are nowhere near one surface, while a bound sized for the typical pair rejects the extreme. Codex
/// measured the second half of that: an 88 at 80° whose axis and intermediates passed through an
/// opening, leaving only the two opposite rim samples on one plane, split into two events — a valid
/// contact refused because it sat exactly at the extreme the bound was cut to.
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
    (run_b.start - run_a.start) - predicted
}

fn build_event(
    corridor: &DiscCorridor,
    walks: &[RayWalk],
    nodes: &[(usize, usize)],
    members: &[usize],
    k: usize,
    _laws: &WalkLaws,
) -> Result<DiscEvent, WalkError> {
    let kf = k as f64;
    let mut by_sample: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &node in members {
        let (sample, run) = nodes[node];
        by_sample.entry(sample).or_default().push(run);
    }
    for runs in by_sample.values_mut() {
        runs.sort_unstable();
    }

    let mut start = f32::INFINITY;
    let mut end = f32::NEG_INFINITY;
    let mut entrance_volume = None;
    let mut entry_sum = Vec3::ZERO;
    let mut entry_position = Vec3::ZERO;
    let mut exit_sum = Vec3::ZERO;
    let mut exit_position = Vec3::ZERO;
    for (&sample, runs) in &by_sample {
        let first = &walks[sample].runs[runs[0]];
        let last = &walks[sample].runs[runs[runs.len() - 1]];
        if first.start < start {
            entrance_volume = Some(first.entry_volume);
        }
        start = start.min(first.start);
        end = end.max(last.end);
        entry_sum += first.entry_normal;
        exit_sum += last.exit_normal;
        let base = corridor.origin + corridor.samples[sample].offset;
        entry_position += base + corridor.axis * first.start;
        exit_position += base + corridor.axis * last.end;
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

    // Disc-mean cumulative cost. Breakpoints are the union of every contributing sample's canonical
    // span boundaries; between two of them every sample's factor is constant, so the mean slope is
    // exact and the profile is piecewise linear.
    let mut breaks: Vec<f32> = vec![start, end];
    for (&sample, runs) in &by_sample {
        for &run in runs {
            let extent = &walks[sample].runs[run];
            for span in &walks[sample].spans {
                if span.end > extent.start && span.start < extent.end {
                    breaks.push(span.start);
                    breaks.push(span.end);
                }
            }
        }
    }
    breaks.sort_by(f32::total_cmp);
    breaks.dedup();
    breaks.retain(|t| *t >= start && *t <= end);

    let mut cumulative: Vec<f64> = Vec::with_capacity(breaks.len());
    cumulative.push(0.0);
    for window in breaks.windows(2) {
        let (a, b) = (window[0], window[1]);
        let mid = 0.5 * (a as f64 + b as f64);
        let mut slope = 0.0f64;
        for (&sample, runs) in &by_sample {
            // The CANONICAL spans carry the field, welded voids included at factor 0 — reading the
            // material segments instead would smear steel across a deleted gap, and welding deletes
            // faces, never material (§13.4).
            let inside = runs.iter().any(|&run| {
                let extent = &walks[sample].runs[run];
                (extent.start as f64) <= mid && mid < (extent.end as f64)
            });
            if !inside {
                continue;
            }
            if let Some(span) = walks[sample]
                .spans
                .iter()
                .find(|span| (span.start as f64) <= mid && mid < (span.end as f64))
            {
                slope += span.factor as f64;
            }
        }
        let last = *cumulative.last().unwrap();
        cumulative.push(last + (b as f64 - a as f64) * slope / kf);
    }
    let profile = CostProfile {
        ts: breaks,
        cumulative,
    };
    let cost = profile.total() as f32;

    // Per-entity shares, unioned per sample then averaged over the disc.
    let mut shares: BTreeMap<Entity, (f32, Vec<(f32, f32)>, BTreeSet<usize>)> = BTreeMap::new();
    for (&sample, runs) in &by_sample {
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
    let shares = shares
        .into_iter()
        .map(|(entity, (factor, spans, samples))| {
            let chord: f64 = spans
                .iter()
                .map(|(a, b)| *b as f64 - *a as f64)
                .sum::<f64>()
                / kf;
            EntityShare {
                entity,
                factor,
                chord: chord as f32,
                cost: (chord * factor as f64) as f32,
                coverage: (samples.len() as f64 / kf) as f32,
                spans,
            }
        })
        .collect();

    // Spall: cluster each sample's downward steps by their factor pair and their ordinal among
    // same-pair steps in that sample. Longitudinal position cannot be the key — on an oblique slab
    // the ring's exits are spread by `r·tan(incidence)` yet they are all one exit face.
    let mut spall_groups: BTreeMap<(u32, u32, usize), (Vec<f32>, Vec3, Vec3, f64, usize)> =
        BTreeMap::new();
    let mut exit_covered = 0usize;
    for (&sample, runs) in &by_sample {
        let mut ordinals: BTreeMap<(u32, u32), usize> = BTreeMap::new();
        let mut saw_exit = false;
        for &run in runs {
            for event in walks[sample].events.iter().filter(|e| e.run == run) {
                if event.factor_after >= event.factor_before {
                    continue;
                }
                if event.kind == BoundaryKind::Exit {
                    saw_exit = true;
                }
                let pair = (event.factor_before.to_bits(), event.factor_after.to_bits());
                let ordinal = ordinals.entry(pair).or_insert(0);
                let slot = spall_groups.entry((pair.0, pair.1, *ordinal)).or_insert((
                    Vec::new(),
                    Vec3::ZERO,
                    Vec3::ZERO,
                    0.0,
                    0,
                ));
                slot.0.push(event.t);
                slot.1 += event.position;
                slot.2 += event.normal;
                slot.3 += event.spall_budget as f64;
                slot.4 += 1;
                *ordinal += 1;
            }
        }
        if saw_exit {
            exit_covered += 1;
        }
    }
    let mut spall: Vec<DiscSpall> = spall_groups
        .into_iter()
        .map(|((from, to, _), (ts, position, normal, budget, count))| {
            let n = count as f32;
            DiscSpall {
                t: ts.iter().map(|t| *t as f64).sum::<f64>() as f32 / n,
                position: position / n,
                normal: if count == 1 {
                    normal
                } else {
                    unit_or_zero(normal)
                },
                from_factor: f32::from_bits(from),
                to_factor: f32::from_bits(to),
                budget: (budget / kf) as f32,
                coverage: (count as f64 / kf) as f32,
            }
        })
        .collect();
    spall.sort_by(|a, b| a.t.total_cmp(&b.t));

    let mut primitives: Vec<PrimitiveKey> = by_sample
        .iter()
        .flat_map(|(sample, runs)| {
            runs.iter()
                .flat_map(move |run| walks[*sample].runs[*run].primitives.iter().copied())
        })
        .collect();
    primitives.sort();
    primitives.dedup();

    Ok(DiscEvent {
        start,
        end,
        entrance_volume: entrance_volume.unwrap_or(Entity::PLACEHOLDER),
        primitives,
        entry_normal,
        entry_position,
        exit_normal,
        exit_position,
        coverage,
        exit_coverage: (exit_covered as f64 / kf) as f32,
        cost,
        profile,
        spall,
        shares,
    })
}

// ---------------------------------------------------------------------------------------------
// Stage 3 — the staged resolution (§13.3 surface laws, §5 spall, §3 capability)
// ---------------------------------------------------------------------------------------------

/// The striking round, reduced to what the laws read.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub caliber: f32,
    /// Reference-mm this round can defeat right now (`super::capability`). The core spends it and
    /// reports what it spent; inverting back to a residual speed stays with the caller, which owns
    /// the DeMarre constants.
    pub capability: f32,
}

/// What the entrance disc decided, and what the surface laws read to decide it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntranceRead {
    pub position: Vec3,
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
        /// Speed retained. FULL bleed at η = 1, none at η → 0 — a graze IS a partial ricochet.
        speed_scale: f32,
    },
    /// It bit in. The caller must now collect the transit corridor along `request.axis`.
    Transit(TransitRequest),
}

/// One transit sample's starting state — where its ray resumes, and what it is already inside.
///
/// Each sample resumes from the point where its OWN entrance ray met the crossing surface, not from
/// a flat disc re-projected around the bent axis. Two things fall out of that, and neither is
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
    /// Progress along that sample's ENTRANCE ray at which the seed was read.
    pub t: f32,
    pub inside: Vec<PrimitiveKey>,
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
    /// Where the DISC AXIS crosses the entrance surface, RELATIVE to [`anchor`](Self::anchor).
    ///
    /// Deliberately NOT the covered-sample centroid. A centroid drags the shell's line of flight
    /// sideways toward whatever part of the disc happened to touch geometry — a lateral kick, which
    /// is exactly the asymmetry §13.5 rules out ("lateral asymmetry unmodeled", re-affirmed
    /// 2026-08-07). The aggregate SURFACE is a mean (`entrance.position`, `n̄`); the shell's AXIS is
    /// not. This also gives the axis-miss/ring-hit case a definition instead of a special case: the
    /// axis ray still crosses the mean entrance plane.
    pub origin: Vec3,
    /// The spawn-anchored basis, parallel-transported onto the bent axis. Never re-derived.
    pub frame: DiscFrame,
    pub radius: f32,
    /// k — the corridor supplied to [`finish`] must sample the disc the same way.
    pub samples: usize,
    /// The entities the entrance crossing engaged, sorted. [`finish`] checks the supplied corridor's
    /// first crossing against these, so a corridor collected for some OTHER contact cannot be
    /// resolved as if it were this one.
    pub entrance_entities: Vec<Entity>,
    /// The primitives the entrance crossing touched, sorted. Entity identity alone is too coarse for
    /// the transit check: one volume can present several primitives metres apart, so "the corridor's
    /// first crossing names a volume the entrance also named" is satisfied by a crossing of a
    /// completely different part of the same hull.
    pub entrance_primitives: Vec<PrimitiveKey>,
    pub seeds: Vec<SampleSeed>,
}

/// Read the entrance disc and decide whether the round enters (§4's decision tree, §13.3's inputs).
pub fn begin(entrance: &DiscWalk, shot: &Shot, laws: &WalkLaws) -> Begin {
    let Some(event) = entrance.events.first() else {
        return Begin::Miss;
    };
    if event.coverage <= 0.0 || event.entry_normal == Vec3::ZERO {
        return Begin::Miss;
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
        normal,
        coverage: event.coverage,
        incidence,
        steel_equivalent,
        overmatched,
    };

    let Ok(axis) = Dir3::new(entrance.axis) else {
        return Begin::Miss;
    };
    let Ok(surface) = Dir3::new(normal) else {
        return Begin::Miss;
    };

    // η scales the deflection ANGLE as well as the bleed (§13.5, RULED 2026-08-07). A graze that
    // turned the round as hard as a square bounce made 1 mm of aim the difference between "flies
    // past" and "full bounce" — the cliff the disc primitive exists to remove.
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
        return Begin::Ricochet {
            entrance: read,
            direction: Vec3::from(direction),
            speed_scale: 1.0 - event.coverage * (1.0 - laws.ricochet_bleed),
        };
    }

    // Normalize: a modest bend toward the inward normal as it bites in, scaled by η on the same
    // ruling — a barely-engaged entry is barely bent. Overmatch does not bend it further; it cancels
    // the SLOPE COST instead (charged in `finish`).
    let bent = super::bend_toward(
        axis,
        -surface,
        event.coverage * laws.normalization * incidence,
    );

    // The handoff plane is the mean entrance surface; the transit ray is the DISC AXIS crossing it.
    // `axis · n̄ < -tangent_cos` always (every contributing entry face leans against the ray), so
    // this cannot divide by zero.
    let denominator = entrance.axis.dot(normal);
    let plane = |from: Vec3| (event.entry_position - from).dot(normal) / denominator;
    let axis_t = plane(entrance.origin);
    let handoff = entrance.origin + entrance.axis * axis_t;
    let seeds = entrance
        .walks
        .iter()
        .enumerate()
        .map(|(index, walk)| {
            let offset = entrance.offsets[index];
            let t = plane(entrance.origin + offset);
            SampleSeed {
                sample: index,
                // Where this sample resumes, relative to the handoff. Carries the longitudinal
                // spread of an oblique contact, so the seed below is a lookup on a ray already
                // walked rather than a guess about a re-projected disc.
                offset: offset + entrance.axis * (t - axis_t),
                t,
                inside: walk.inside_at(t, laws),
            }
        })
        .collect();
    let mut entrance_entities: Vec<Entity> = event.shares.iter().map(|s| s.entity).collect();
    entrance_entities.sort();

    Begin::Transit(TransitRequest {
        entrance: read,
        axis: Vec3::from(bent),
        anchor: entrance.anchor,
        origin: handoff,
        frame: entrance.frame.transport(entrance.axis, Vec3::from(bent)),
        radius: entrance.radius,
        samples: entrance.samples(),
        entrance_entities,
        entrance_primitives: event.primitives.clone(),
        seeds,
    })
}

/// How the crossing ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// Defeated: buried where the prefix integral of the disc-mean cost reaches the capability.
    Embedded { at: Vec3, t: f32 },
    /// Punched through: the march resumes at the aggregate exit face.
    Perforated {
        exit: Vec3,
        exit_normal: Vec3,
        direction: Vec3,
        t: f32,
    },
}

/// One entity's deposit from this crossing.
///
/// Damage attribution is per-presence and needs no arbitration (§13.2: no ownership, no priority, no
/// argmax). The IMPULSE does, when a crossing's presence set spans several rigid bodies — which can
/// only happen when distinct bodies overlap in space, since within one tank every volume shares the
/// body. RULED 2026-08-07 (§13.5): the full momentum exchange goes to the body owning the ENTRANCE
/// surface, the first one physically touched; anything behind it is shoved through contact by the
/// physics engine. So the caller routes the shove by [`ResolutionPlan::entrance`], not by these
/// shares — they are the damage, and the diagnostic if that default is ever revisited.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityDeposit {
    pub entity: Entity,
    pub factor: f32,
    pub chord: f32,
    /// Reference-mm of ITS OWN material the round chewed — §6's transit damage, before the caller's
    /// `TRANSIT_K`.
    pub cost: f32,
    pub coverage: f32,
}

/// Everything the crossing decided, as declarative facts for the caller to apply in order.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolutionPlan {
    pub entrance: EntranceRead,
    pub outcome: Outcome,
    /// Reference-mm actually spent (the whole crossing when it perforates, the capability when it
    /// embeds).
    pub cost_spent: f32,
    pub deposits: Vec<EntityDeposit>,
    pub spall: Vec<DiscSpall>,
}

/// The corridor [`finish`] was handed must be the corridor [`begin`] asked for.
///
/// Between the two stages the caller runs a spatial query, and nothing in the type system stops it
/// from collecting along the UNBENT axis, from the wrong origin, with a re-derived frame, or around
/// a different contact entirely. Any of those silently resolves one surface's geometry against
/// another surface's entrance verdict. Tolerances are loose enough for honest f32 round-tripping and
/// far tighter than any of these mistakes.
fn validate_transit(transit: &DiscWalk, request: &TransitRequest) -> Result<(), WalkError> {
    let mismatch = |reason| Err(WalkError::CorridorMismatch { reason });
    if (transit.axis - request.axis).length() > 1.0e-4 {
        return mismatch(
            "the corridor was collected along a different axis than the bend asked for",
        );
    }
    if transit.anchor != request.anchor {
        return mismatch("the corridor was collected against a different world anchor");
    }
    if (transit.origin - request.origin).length() > 1.0e-4 {
        return mismatch("the corridor starts somewhere other than the entrance handoff point");
    }
    if (transit.frame.u - request.frame.u).length() > 1.0e-4
        || (transit.frame.v - request.frame.v).length() > 1.0e-4
    {
        return mismatch("the ring frame was re-derived instead of transported");
    }
    if (transit.radius - request.radius).abs() > 1.0e-6 {
        return mismatch("the corridor samples a different disc radius");
    }
    if transit.samples() != request.samples {
        return mismatch("the corridor has a different sample count than the entrance disc");
    }

    // PER-SAMPLE geometry, not just the disc's. The handoff exports where each ray resumes, and a
    // caller that laid its ring out freshly — or nudged one origin — produces a corridor whose
    // aggregate axis, origin, frame and radius all still match while the rays themselves sample
    // somewhere else entirely. The seeds are only sound for the rays they were read on.
    for (index, seed) in request.seeds.iter().enumerate() {
        if (transit.offsets[index] - seed.offset).length() > SEED_OFFSET_TOLERANCE {
            return mismatch("a sample ray resumes somewhere other than where the handoff put it");
        }
    }

    let Some(event) = transit.events.first() else {
        return Ok(());
    };
    // IDENTITY, on the two things that actually name a crossing: the geometry it is made of, and
    // where it sits. Sharing an ENTITY is too coarse — one hull presents primitives metres apart —
    // and a crossing that begins far from the handoff plane is not the one the entrance decided to
    // enter, however familiar its volumes look.
    let shares_primitive = event
        .primitives
        .iter()
        .any(|key| request.entrance_primitives.binary_search(key).is_ok());
    let same_surface = event.entry_normal.dot(request.entrance.normal) >= ENTRANCE_SURFACE_COS;
    if !(shares_primitive || same_surface) {
        return mismatch(
            "the corridor's first crossing is neither made of the entrance's geometry nor on its surface",
        );
    }
    if event.start > HANDOFF_PROXIMITY {
        return mismatch("the corridor's first crossing does not begin at the handoff plane");
    }
    Ok(())
}

/// How far a supplied sample origin may sit from the one the handoff exported. Generous enough for
/// an honest f32 round-trip of a position through a corridor build, orders below any real re-aim.
const SEED_OFFSET_TOLERANCE: f32 = 1.0e-4;

/// How far into the transit corridor its first crossing may begin. The handoff plane IS the entrance
/// surface, so the crossing the entrance decided to enter starts at zero; a millimetre covers the
/// rounding of a face position that was computed, transported and re-collected.
const HANDOFF_PROXIMITY: f32 = 1.0e-3;

/// Cosine bound for "the transit's first crossing is on the surface the entrance read". Shares the
/// magnitude of the event-plane rule for the same reason: two reads of one surface patch.
const ENTRANCE_SURFACE_COS: f32 = 0.9;

/// Resolve the transit corridor into the plan (§3's perforate/embed, §5's spall, §6's deposits).
pub fn finish(
    transit: &DiscWalk,
    request: &TransitRequest,
    shot: &Shot,
    _laws: &WalkLaws,
) -> Result<ResolutionPlan, WalkError> {
    validate_transit(transit, request)?;
    let Some(event) = transit.events.first() else {
        return Err(WalkError::BadCorridor {
            sample: 0,
            reason: "the transit corridor met no material after the entrance decided to enter",
        });
    };

    // Overmatch cannot be made to present its oblique line of sight to a round that dwarfs it, so
    // the CHARGED chord is the perpendicular projection, not the full slope chord (§4).
    //
    // It applies to every consequence, not just to the capability spend. §13.5 defines the spall
    // budget AS the event's cost and §6 defines transit damage from the cost paid — so a projection
    // that scaled only `cost_spent` would leave a 60° overmatched crossing spending 15 reference-mm
    // while throwing spall and depositing damage for 30. One `charge` factor, applied to the prefix
    // profile, the spall budgets and the deposits alike. The GEOMETRIC quantities (`t`, the embed
    // position, the presence spans) are untouched: the round still travels the distance it travels.
    let charge = if request.entrance.overmatched {
        transit.axis.dot(request.entrance.normal).abs() as f64
    } else {
        1.0
    };
    let profile = event.profile.scaled(charge);
    let total = profile.total();

    let embed_t = profile.invert(shot.capability as f64);
    let (outcome, cost_spent, cut) = match embed_t {
        Some(t) => {
            let at = transit.origin + transit.axis * t;
            (
                Outcome::Embedded { at, t },
                shot.capability.min(total as f32),
                t,
            )
        }
        None => (
            Outcome::Perforated {
                exit: event.exit_position,
                exit_normal: event.exit_normal,
                direction: transit.axis,
                t: event.end,
            },
            total as f32,
            event.end,
        ),
    };

    // Per-entity damage is clipped at the embed progress, never scaled: a round that stops in the
    // first plate must deposit nothing in the crew behind it.
    let deposits = event
        .shares
        .iter()
        .map(|share| {
            let chord: f64 = share
                .spans
                .iter()
                .map(|(a, b)| (b.min(cut) as f64 - *a as f64).max(0.0))
                .sum::<f64>()
                / transit.samples() as f64;
            let covered = share
                .spans
                .iter()
                .filter(|(a, _)| *a < cut)
                .count()
                .min(transit.samples());
            let charged = chord * charge;
            EntityDeposit {
                entity: share.entity,
                factor: share.factor,
                chord: charged as f32,
                cost: (charged * share.factor as f64) as f32,
                coverage: if covered == 0 { 0.0 } else { share.coverage },
            }
        })
        .filter(|deposit| deposit.chord > 0.0)
        .collect();

    // No exit, no spall (§5: spall is an exit/perforation event). An embed keeps only the downward
    // steps it actually reached, and every budget is charged on the same projection as the cost it
    // is defined to equal.
    let spall = event
        .spall
        .iter()
        .filter(|mark| mark.t <= cut)
        .map(|mark| DiscSpall {
            budget: (mark.budget as f64 * charge) as f32,
            ..*mark
        })
        .collect();

    Ok(ResolutionPlan {
        entrance: request.entrance,
        outcome,
        cost_spent,
        deposits,
        spall,
    })
}
