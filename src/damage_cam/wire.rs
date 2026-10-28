//! A pen cam record on the wire: [`ShotExplanation`], built once on the authority from a
//! [`RecordedShot`] ([`BuiltExplanation::build`]), projected per recipient
//! ([`BuiltExplanation::projections`]), and turned back into a [`PenetrationReport`] for the client's
//! views ([`ShotExplanation::decode`]). Transport is not here.
//!
//! # Vocabulary
//!
//! No name rides the wire. [`ShotVocabulary`] maps every name a record holds (vehicle file, servo,
//! volume — armour, external armour and component alike —, weapon, round) to an index, and holds
//! the shapes the client rebuilds each ray's direction from: the spall cone's and the burst's.
//! Both ends build it from data: each vehicle's tank file and sim artifact, the round registry, the
//! cone shape and the burst fields. What a
//! component is (role, HP pool, function) and an armour volume's class are read from the vocabulary
//! on decode, not carried. Its fingerprint rides every explanation, folded the way the protocol
//! fingerprint folds its manifest ([`crate::fingerprint`]); a client whose vocabulary
//! folds another refuses the explanation rather than misread it.
//!
//! # Quanta
//!
//! Every bound is DERIVED from the encoding, and checked by the round-trip test over every recorded
//! scenario (`assets/dev/pen_cam/`) and by a dense sweep of each quantizer
//! (`wire::tests::the_quanta_hold_their_bounds`). MEASURED worst cases: a direction 6.35e-5 rad, a
//! rotation 5.7e-5 rad, an angle 0.00275°, a recorded ray's end 0.99 mm (at most 0.67 of its
//! bound).
//!
//! | quantity | on the wire | bound on decode |
//! |---|---|---|
//! | point (hull-local or WORLD) | `i32` mm per axis | 0.5 mm per axis, 0.87 mm in all, plus half an f32 ulp of the coordinate |
//! | time from contact | `i32` µs | 0.5 µs plus half an f32 ulp of the time |
//! | unit direction | octahedral, two `u16` | [`DIRECTION_BOUND`] (6.5e-5 rad) |
//! | servo angle | `u16` of a full turn | 0.0028°, read back in (−180°, 180°] |
//! | hull rotation | quaternion, four `i16`; refused unless unit within [`ROTATION_SLACK`] | [`ROTATION_BOUND`] (6.2e-5 rad) |
//! | field reach | `u16` mm, `u16::MAX` marking a `u32` in the cone's overflow list | 0.5 mm |
//! | a ray's end | the cone's origin plus its direction times its reach | [`end_bound`]: 1.4 mm plus 1.8e-4 of the reach |
//! | doses, strengths, chord, cost, speed, calibre, mass | `f32` | exact |
//! | what a pool held at the shot, what the shot took | `u32` pool units | exact |
//! | plate factor | cost over chord, derived | four f32 ulps, relative |
//! | incidence | `f32`, the record's own: an f32 `acos` of the decoded directions is off by 0.04° near normal impact (MEASURED) | exact |
//!
//! A ray's direction and strength are not carried. A cone carries its field descriptor, its law (a
//! ring's with its half-width) and its ratio `λ` (`spall_cone::FieldDescriptor`), and the client
//! spreads the vocabulary's shape by it (a burst's field: the burst's shape, the ring's), rebuilding
//! each stratum and weight to the bit (`spall_cone` module doc, "Determinism");
//! the encoder refuses a cone whose rays' strengths are not `S·f_k` under its descriptor to the bit, or
//! whose half-angle is not the descriptor's. A ray's direction is its stratum about the cone's
//! axis, placed by the cone's roll — the turn about the axis from the shape's own lateral axis
//! ([`ConeShape::lateral_axis`]) to the one the throw used, which the record's hull frame and the
//! world's `+Y` decide. The encoder fits the roll to the record's ray ends, weighting each by its
//! reach. The client never re-integrates a ray: the reach is the authority's.
//!
//! # Disclosure
//!
//! The shooter's projection holds no component this shot's doses do not name: an unreached
//! component draws as neutral layout from tank data, so no HP of it, and nothing that depends on
//! it, leaves the authority toward the shooter. A reached component carries what its pool held at
//! the shot and what it lost, the record of what the shot found. The kill and its reason are
//! public (every client reads them in the tank status), so the attribution rides every projection.
//! The victim gets its own tank's full record; a killed victim also gets the kill cam's lesson
//! ([`Audience`]). A combatant in both roles gets one message, the victim's: a round is
//! transparent to the tank that fired it, so the two meet only when a round fired by one life
//! strikes the combatant's next.
//! Nobody else gets anything.
//!
//! # Encoded sizes
//!
//! MEASURED with the serializer Lightyear 0.30 sends messages with (postcard), every recorded
//! scenario, N = 64 rays per spall cone and 256 per burst field (`wire::tests::encoded_sizes`).
//! Total is the victim's message, fields its spall cones alone, burst the record's burst with its
//! field:
//!
//! | scenario | total B | shooter B | fields B | cones | rays | hits | B per cone | burst B |
//! |---|---|---|---|---|---|---|---|---|
//! | s1_through_crew | 938 | 890 | 565 | 1 | 64 | 66 | 565 | – |
//! | s2_stopped_mantlet | 218 | 142 | 0 | 0 | 0 | 0 | – | – |
//! | s3_ricochet | 224 | 148 | 0 | 0 | 0 | 0 | – | – |
//! | s4_spaced_then_stopped | 788 | 712 | 465 | 3 | 192 | 0 | 155 | – |
//! | s5_oblique_through | 810 | 738 | 539 | 1 | 64 | 64 | 539 | – |
//! | s6_overpen | 585 | 509 | 309 | 2 | 128 | 0 | 154 | – |
//! | s7_engine_shadow | 757 | 693 | 466 | 1 | 64 | 51 | 466 | – |
//! | s8a_mg_ricochet | 224 | 148 | 0 | 0 | 0 | 0 | – | – |
//! | s8b_mg_stopped | 216 | 140 | 0 | 0 | 0 | 0 | – | – |
//! | s9_breech_and_loader | 826 | 770 | 503 | 1 | 64 | 56 | 503 | – |
//! | s10_gun_barrel_struck | 946 | 874 | 696 | 2 | 128 | 64 | 348 | – |
//! | s11_mg_barrel_struck | 218 | 145 | 0 | 0 | 0 | 0 | – | – |
//! | s12_apcr_through_crew | 934 | 886 | 565 | 1 | 64 | 66 | 565 | – |
//! | s13_ring_through_crew | 2252 | 2212 | 565 | 1 | 64 | 66 | 565 | 1263 |
//! | s15_unarmed_overpen | 585 | 509 | 309 | 2 | 128 | 0 | 154 | – |
//! | s16_restart_at_rest | 1105 | 1029 | 465 | 3 | 192 | 0 | 155 | 309 |
//!
//! The DERIVED estimate `41 + 2 + 2N` per cone (171 B at N = 64) counts a cone's metadata, its
//! field descriptor's law byte and `f32` ratio among it, and its reaches; a cone that reaches no
//! component measures 154–155 B on average per scenario (a reach under 128 mm takes one varint
//! byte, one of 16.4 m or more three). Each hit adds about three bytes (ray skip, volume, dose
//! palette index, a varint each) and each distinct dose four. Under the ball every direction is
//! thrown at its own strength, so nearly every hit's dose is distinct: a cone that lights the
//! fighting compartment measures 460–590 B, and every recorded message without a burst stays under
//! 0.95 KB. A burst's field at its 256 directions adds about 310 B where no direction leaves the
//! burst point (s16) and 1.3 KB where its band reaches the compartment's boxes (s13), so the
//! largest recorded message is 2.2 KB.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::recorder::RecordedShot;
use super::{
    BoxDose, Burst, ComponentState, Consequence, ConsequenceState, Fragment, FragmentHit, Impact,
    PathPoint, PenetrationReport, Plate, PlateClass, Projectile, Ricochet, Role, RoundEnd,
    ShotVerdict, SpallCone,
};
use crate::bake::TankGeometry;
use crate::ballistics::spall_cone::{
    BALL_EXPONENT, BurstFields, ConeShape, FieldDescriptor, FieldLaw, Spread,
};
use crate::damage::{FunctionRole, KnockoutReason};
use crate::fingerprint::{FNV_OFFSET_BASIS, fingerprint_field, fnv1a_64};
use crate::rounds::RoundRegistry;
use crate::spec::{TankSpec, VolumeSpec};
use crate::{CombatantId, CombatantLife, ShotId};

/// The largest angle (rad) between a unit direction and its octahedral decode. DERIVED: rounding
/// moves each square coordinate by at most `q/2`, `q = 2/65535`; the octahedron point moves by at
/// most `q·√1.5` (the third coordinate takes both moves), and that point lies at least `1/√3` from
/// the origin, so the direction turns by at most `q·√1.5·√3 = 2.12 q`.
#[cfg(test)]
pub(crate) const DIRECTION_BOUND: f32 = 6.5e-5;

/// The largest rotation angle (rad) between a rotation and its decode. DERIVED: rounding moves each
/// of the four components by at most `0.5/32767`, the unit quaternion by at most `δ = 3.05e-5`, and
/// a rotation's angle is twice the quaternion's arc, so at most `2δ/(1 − δ)`.
#[cfg(test)]
pub(crate) const ROTATION_BOUND: f32 = 6.2e-5;

/// How far from unit length a received quaternion may be. DERIVED: rounding moves the encoded
/// quaternion's length by at most `√4 · 0.5/32767 = 3.05e-5`; the slack is three times that, and
/// anything outside it (a zero quaternion among them) is no rotation the encoder wrote.
pub(crate) const ROTATION_SLACK: f32 = 1.0e-4;

/// The largest angle (rad) a ray's decoded direction may lie from the thrown one: the axis's
/// [`DIRECTION_BOUND`], again for the roll fitted against the decoded axis, and half the roll's
/// quantum `π/65536` (a ray at most a right angle off its axis). DERIVED.
#[cfg(test)]
const RAY_DIRECTION_BOUND: f32 = 2.0 * DIRECTION_BOUND + std::f32::consts::PI / 65_536.0;

/// The largest distance (m) between a ray's decoded end and the record's, for a ray of reach
/// `reach` (m): the origin's point bound (0.87 mm), the reach's half millimetre, the direction's
/// [`RAY_DIRECTION_BOUND`] over the reach, and 10 µm for the record's own f32 rounding. DERIVED.
#[cfg(test)]
pub(crate) fn end_bound(reach: f32) -> f32 {
    0.000_87 + 0.000_5 + reach * RAY_DIRECTION_BOUND + 0.000_01
}

/// The reach marking a ray whose reach is in the cone's overflow list.
const REACH_OVERFLOW: u16 = u16::MAX;

/// Why a record could not be put on the wire, or a message could not be read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodecError(String);

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn refuse<T>(why: impl Into<String>) -> Result<T, CodecError> {
    Err(CodecError(why.into()))
}

// --- Vocabulary -----------------------------------------------------------------------------------

/// What a volume of a vehicle is to a record.
#[derive(Clone, Debug, PartialEq)]
enum VolumeWord {
    Armour(PlateClass),
    Component {
        role: Role,
        count: u32,
        function: Option<String>,
    },
}

/// One vehicle's names: servos, and every ballistic volume by the name a record gives it.
#[derive(Clone, Debug)]
struct VehicleWords {
    file: String,
    servos: Vec<String>,
    /// Name-sorted.
    volumes: Vec<(String, VolumeWord)>,
}

impl VehicleWords {
    /// The vehicle of tank file `file`, its sim artifact `geometry` and its spec `spec`. A volume
    /// is named as the record names it: its node, or for each substance of a node of several, the
    /// node, the substance and the primitive's place among the node's volumes,
    /// `node/substance#k`, in substance order, as spawn hangs them.
    fn new(file: &str, geometry: &TankGeometry, spec: &TankSpec) -> Self {
        let mut volumes = Vec::new();
        for &index in &geometry.ballistic_volumes {
            let node = &geometry.nodes[index];
            let facets = spec.volumes.get(&node.name);
            let groups = node.substance_groups();
            let class = if matches!(facets, Some(VolumeSpec::External)) {
                PlateClass::External
            } else {
                PlateClass::Main
            };
            if groups.len() > 1 {
                let shards = groups
                    .iter()
                    .flat_map(|(substance, primitives)| primitives.iter().map(move |_| *substance));
                for (place, substance) in shards.enumerate() {
                    volumes.push((
                        format!("{}/{substance}#{place}", node.name),
                        VolumeWord::Armour(class),
                    ));
                }
                continue;
            }
            let word = match facets.and_then(VolumeSpec::component) {
                Some(component) => VolumeWord::Component {
                    role: if component.crew.is_some() {
                        Role::Crew
                    } else if component.ammo {
                        Role::Ammo
                    } else if component.function == Some(FunctionRole::Engine) {
                        Role::Engine
                    } else {
                        Role::Module
                    },
                    count: component.hp,
                    function: component.function.map(|function| format!("{function:?}")),
                },
                None => VolumeWord::Armour(class),
            };
            volumes.push((node.name.clone(), word));
        }
        volumes.sort_by(|a, b| a.0.cmp(&b.0));
        let mut servos: Vec<String> = spec.servos.keys().cloned().collect();
        servos.sort();
        Self {
            file: file.to_owned(),
            servos,
            volumes,
        }
    }

    fn volume(&self, name: &str) -> Result<u16, CodecError> {
        match self
            .volumes
            .binary_search_by(|(known, _)| known.as_str().cmp(name))
        {
            Ok(index) => index_u16(index, "volume"),
            Err(_) => refuse(format!("{}: no volume named {name:?}", self.file)),
        }
    }

    fn volume_at(&self, index: u16) -> Result<&(String, VolumeWord), CodecError> {
        self.volumes
            .get(index as usize)
            .map_or_else(|| refuse(format!("{}: no volume {index}", self.file)), Ok)
    }

    /// The component at `index`: its name, role, HP pool and function.
    fn component_at(&self, index: u16) -> Result<(&str, Role, u32, Option<&str>), CodecError> {
        match self.volume_at(index)? {
            (
                name,
                VolumeWord::Component {
                    role,
                    count,
                    function,
                },
            ) => Ok((name, *role, *count, function.as_deref())),
            (name, VolumeWord::Armour(_)) => refuse(format!("{name} is not a component")),
        }
    }

    fn component(&self, name: &str) -> Result<u16, CodecError> {
        let index = self.volume(name)?;
        self.component_at(index)?;
        Ok(index)
    }
}

/// The fields the vocabulary's fingerprint spreads its cone shape under ([`fold_cone`]): probes of
/// each law's weights, the hemisphere, a 30° cone, the ball below a ratio of 1 and a ring. Probe
/// data, not any substance's or round's.
const FINGERPRINT_FIELDS: [FieldDescriptor; 4] = [
    FieldDescriptor::ball(1.0),
    FieldDescriptor::ball(2.0),
    FieldDescriptor::ball(0.5),
    FieldDescriptor::ring(0.5, 0.2),
];

/// `hash` folded with what a decoder rebuilds a cone's rays from besides the cone's own descriptor:
/// every law's id and name, the ball's exponent `exponent`, the shape's direction count and
/// azimuths, and the shape's strata under each of [`FINGERPRINT_FIELDS`]. Two ends that would
/// rebuild different rays from one descriptor fold differently.
fn fold_cone(mut hash: u64, cone: &ConeShape, exponent: u32) -> u64 {
    for (id, name) in FieldLaw::NAMES.iter().enumerate() {
        hash = fingerprint_field(hash, b"field_law", &[id as u8]);
        hash = fingerprint_field(hash, b"field_law_name", name.as_bytes());
    }
    hash = fingerprint_field(hash, b"ball_exponent", &exponent.to_le_bytes());
    hash = fingerprint_field(
        hash,
        b"cone_directions",
        &(cone.directions_count() as u64).to_le_bytes(),
    );
    for (cos, sin) in cone.azimuths() {
        let bytes: Vec<u8> = [cos, sin]
            .iter()
            .flat_map(|value| value.to_bits().to_le_bytes())
            .collect();
        hash = fingerprint_field(hash, b"azimuth", &bytes);
    }
    for field in FINGERPRINT_FIELDS {
        // A probe lies in its law's domain, so a refusal is a broken law or an exponent the ball's
        // sphere does not take: it is folded, not skipped, and no end that spreads it folds alike.
        hash = match cone.spread_under(field, exponent) {
            Ok(spread) => {
                let mut hash = fingerprint_field(
                    hash,
                    b"probe_half_angle",
                    &spread.field().half_angle().to_bits().to_le_bytes(),
                );
                for stratum in spread.strata() {
                    let bytes: Vec<u8> = stratum
                        .iter()
                        .flat_map(|value| value.to_bits().to_le_bytes())
                        .collect();
                    hash = fingerprint_field(hash, b"probe_stratum", &bytes);
                }
                hash
            }
            Err(error) => fingerprint_field(hash, b"probe_refused", error.as_bytes()),
        };
    }
    hash
}

/// Every name a pen cam record can hold, by index, and the shapes its rays were thrown in: the
/// cone shape and the burst fields'.
#[derive(Resource, Clone, Debug)]
pub(crate) struct ShotVocabulary {
    /// File-sorted.
    vehicles: Vec<VehicleWords>,
    /// Every weapon of every vehicle, sorted, once.
    weapons: Vec<String>,
    /// The round registry's keys, in key order.
    rounds: Vec<String>,
    cone: ConeShape,
    burst: BurstFields,
    fingerprint: u64,
}

impl ShotVocabulary {
    /// The vocabulary of `vehicles` — each a tank file (relative to the asset root), its sim
    /// artifact and its spec —, the registry `rounds`, the cone shape `cone` and the burst fields
    /// `burst`.
    pub(crate) fn new<'a>(
        vehicles: impl IntoIterator<Item = (&'a str, &'a TankGeometry, &'a TankSpec)>,
        rounds: &RoundRegistry,
        cone: &ConeShape,
        burst: &BurstFields,
    ) -> Self {
        let mut weapons = Vec::new();
        let mut words = Vec::new();
        for (file, geometry, spec) in vehicles {
            weapons.extend(spec.weapons.keys().cloned());
            words.push(VehicleWords::new(file, geometry, spec));
        }
        words.sort_by(|a, b| a.file.cmp(&b.file));
        weapons.sort();
        weapons.dedup();
        let mut vocabulary = Self {
            vehicles: words,
            weapons,
            rounds: rounds.keys().map(str::to_owned).collect(),
            cone: cone.clone(),
            burst: burst.clone(),
            fingerprint: 0,
        };
        vocabulary.fingerprint = vocabulary.fold();
        vocabulary
    }

    /// The fingerprint every explanation built against this vocabulary carries.
    #[cfg(test)]
    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// Every name and value the decode reads, in order, each labelled.
    fn fold(&self) -> u64 {
        let mut hash = fnv1a_64(FNV_OFFSET_BASIS, b"overmatch-shot-vocabulary-v2\0");
        for vehicle in &self.vehicles {
            hash = fingerprint_field(hash, b"vehicle", vehicle.file.as_bytes());
            for servo in &vehicle.servos {
                hash = fingerprint_field(hash, b"servo", servo.as_bytes());
            }
            for (name, word) in &vehicle.volumes {
                hash = fingerprint_field(hash, b"volume", name.as_bytes());
                hash = match word {
                    VolumeWord::Armour(class) => {
                        fingerprint_field(hash, b"armour", &[*class as u8])
                    }
                    VolumeWord::Component {
                        role,
                        count,
                        function,
                    } => {
                        let hash = fingerprint_field(hash, b"role", &[*role as u8]);
                        let hash = fingerprint_field(hash, b"count", &count.to_le_bytes());
                        fingerprint_field(
                            hash,
                            b"function",
                            function.as_deref().unwrap_or("").as_bytes(),
                        )
                    }
                };
            }
        }
        for weapon in &self.weapons {
            hash = fingerprint_field(hash, b"weapon", weapon.as_bytes());
        }
        for round in &self.rounds {
            hash = fingerprint_field(hash, b"round", round.as_bytes());
        }
        hash = fold_cone(hash, &self.cone, BALL_EXPONENT);
        fold_cone(
            fingerprint_field(hash, b"burst_shape", &[]),
            self.burst.shape(),
            BALL_EXPONENT,
        )
    }

    fn vehicle(&self, file: &str) -> Result<u16, CodecError> {
        match self
            .vehicles
            .iter()
            .position(|vehicle| vehicle.file == file)
        {
            Some(index) => index_u16(index, "vehicle"),
            None => refuse(format!("no vehicle {file:?}")),
        }
    }

    fn vehicle_at(&self, index: u16) -> Result<&VehicleWords, CodecError> {
        self.vehicles
            .get(index as usize)
            .map_or_else(|| refuse(format!("no vehicle {index}")), Ok)
    }

    fn named(list: &[String], name: &str, what: &str) -> Result<u16, CodecError> {
        match list.binary_search_by(|known| known.as_str().cmp(name)) {
            Ok(index) => index_u16(index, what),
            Err(_) => refuse(format!("no {what} named {name:?}")),
        }
    }

    fn name_at<'a>(list: &'a [String], index: u16, what: &str) -> Result<&'a str, CodecError> {
        list.get(index as usize)
            .map(String::as_str)
            .map_or_else(|| refuse(format!("no {what} {index}")), Ok)
    }
}

fn index_u16(index: usize, what: &str) -> Result<u16, CodecError> {
    u16::try_from(index).or_else(|_| refuse(format!("{what} index {index} exceeds u16")))
}

// --- Quanta ---------------------------------------------------------------------------------------

/// A point: millimetres per axis.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct Mm([i32; 3]);

/// `value` (m) in whole millimetres; `None` outside `i32`.
fn millimetres(value: f32) -> Option<i32> {
    let scaled = (value as f64 * 1000.0).round();
    (scaled >= i32::MIN as f64 && scaled <= i32::MAX as f64).then_some(scaled as i32)
}

impl Mm {
    fn of(point: [f32; 3]) -> Result<Self, CodecError> {
        let mut out = [0; 3];
        for (slot, value) in out.iter_mut().zip(point) {
            *slot = millimetres(value)
                .map_or_else(|| refuse(format!("point {point:?} outside i32 mm")), Ok)?;
        }
        Ok(Self(out))
    }

    fn of_vec(point: Vec3) -> Result<Self, CodecError> {
        Self::of(point.to_array())
    }

    fn point(self) -> [f32; 3] {
        self.0.map(|value| (value as f64 / 1000.0) as f32)
    }
}

/// `t` (s) in whole microseconds.
fn micros(t: f32) -> Result<i32, CodecError> {
    let scaled = (t as f64 * 1.0e6).round();
    if scaled >= i32::MIN as f64 && scaled <= i32::MAX as f64 {
        Ok(scaled as i32)
    } else {
        refuse(format!("time {t} s outside i32 µs"))
    }
}

fn seconds(micros: i32) -> f32 {
    (micros as f64 / 1.0e6) as f32
}

/// A unit direction, octahedral: the direction over its L1 norm, the lower half folded over the
/// upper, the two square coordinates in `u16`s.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct Oct([u8; 4]);

/// `+1` for `x ≥ 0` (either zero), `−1` otherwise.
fn sign(x: f32) -> f32 {
    if x >= 0.0 { 1.0 } else { -1.0 }
}

impl Oct {
    fn of(direction: [f32; 3]) -> Result<Self, CodecError> {
        let v = Vec3::from(direction);
        let l1 = v.x.abs() + v.y.abs() + v.z.abs();
        if !(l1.is_finite() && l1 > 0.0) {
            return refuse(format!("direction {direction:?} is not a direction"));
        }
        let n = v / l1;
        let (u, w) = if n.z >= 0.0 {
            (n.x, n.y)
        } else {
            ((1.0 - n.y.abs()) * sign(n.x), (1.0 - n.x.abs()) * sign(n.y))
        };
        let step = |c: f32| ((c.clamp(-1.0, 1.0) as f64 + 1.0) * 0.5 * 65_535.0).round() as u16;
        let (u, w) = (step(u).to_le_bytes(), step(w).to_le_bytes());
        Ok(Self([u[0], u[1], w[0], w[1]]))
    }

    fn direction(self) -> Vec3 {
        let [u0, u1, w0, w1] = self.0;
        let unstep = |q: u16| (q as f64 / 65_535.0 * 2.0 - 1.0) as f32;
        let (u, w) = (
            unstep(u16::from_le_bytes([u0, u1])),
            unstep(u16::from_le_bytes([w0, w1])),
        );
        let z = 1.0 - u.abs() - w.abs();
        let (x, y) = if z < 0.0 {
            ((1.0 - w.abs()) * sign(u), (1.0 - u.abs()) * sign(w))
        } else {
            (u, w)
        };
        Vec3::new(x, y, z).normalize()
    }
}

/// An angle as a `u16` share of a full turn.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct Turn([u8; 2]);

impl Turn {
    fn of_radians(angle: f32) -> Result<Self, CodecError> {
        if !angle.is_finite() {
            return refuse(format!("angle {angle} is not finite"));
        }
        let share = (angle as f64 / std::f64::consts::TAU).rem_euclid(1.0);
        Ok(Self(
            ((share * 65_536.0).round() as u32 as u16).to_le_bytes(),
        ))
    }

    /// The angle (rad) in `(−π, π]`.
    fn radians(self) -> f32 {
        let share = u16::from_le_bytes(self.0) as f64 / 65_536.0;
        let share = if share > 0.5 { share - 1.0 } else { share };
        (share * std::f64::consts::TAU) as f32
    }

    fn of_degrees(angle: f32) -> Result<Self, CodecError> {
        Self::of_radians((angle as f64).to_radians() as f32)
    }

    fn degrees(self) -> f32 {
        let share = u16::from_le_bytes(self.0) as f64 / 65_536.0;
        let share = if share > 0.5 { share - 1.0 } else { share };
        (share * 360.0) as f32
    }
}

/// A rotation: its unit quaternion, `w ≥ 0`, each component in an `i16` of `1/32767`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct Rotation([u8; 8]);

impl Rotation {
    fn of(rotation: Quat) -> Result<Self, CodecError> {
        if !rotation.is_finite() || rotation.length_squared() == 0.0 {
            return refuse(format!("rotation {rotation:?} is not a rotation"));
        }
        let q = rotation.normalize();
        let q = if q.w < 0.0 { -q } else { q };
        let mut out = [0; 8];
        for (index, component) in q.to_array().into_iter().enumerate() {
            let step = (component * 32_767.0).round() as i16;
            out[2 * index..2 * index + 2].copy_from_slice(&step.to_le_bytes());
        }
        Ok(Self(out))
    }

    /// The rotation; refused when the four components are not a unit quaternion within
    /// [`ROTATION_SLACK`].
    fn rotation(self) -> Result<Quat, CodecError> {
        let at = |index: usize| {
            i16::from_le_bytes([self.0[2 * index], self.0[2 * index + 1]]) as f32 / 32_767.0
        };
        let q = Quat::from_xyzw(at(0), at(1), at(2), at(3));
        if (q.length() - 1.0).abs() > ROTATION_SLACK {
            return refuse(format!("{q:?} is not a unit quaternion"));
        }
        Ok(q.normalize())
    }
}

// --- The message ----------------------------------------------------------------------------------

/// One life of a combatant: its match identity and which of its spawns it is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TankLife {
    pub combatant: CombatantId,
    pub life: CombatantLife,
}

/// Who an explanation is for.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Audience {
    /// The shooter: this shot's outcome, no unreached component.
    Shooter,
    /// The struck tank's own player: its full record.
    Victim,
    /// The struck tank's own player, killed by this shot: its full record and the kill cam's
    /// lesson.
    KilledVictim,
}

/// The pen cam's explanation of one shot's encounter with one tank, as one recipient receives it.
/// Ordered by [`Self::shot`], [`Self::encounter`] and the ticks, never by arrival.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct ShotExplanation {
    pub shot: ShotId,
    /// Which of the round's encounters with the struck tank, 0 the first.
    pub encounter: u32,
    /// The struck tank's life.
    pub struck: TankLife,
    /// The authority tick the encounter opened on (first contact) and the one its record finished
    /// on.
    pub contact_tick: u32,
    pub completion_tick: u32,
    /// The ordinal of the shot's sanctioned outcome a client draws as this encounter's impact: a
    /// bounce's sequence, or the end's count of bounces before it. `None` where no client draws it
    /// (the shot ended, for a client, at an earlier tank).
    pub outcome: Option<u32>,
    pub audience: Audience,
    /// The struck hull node's pose (WORLD) at contact.
    hull: Option<Pose>,
    record: Record,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
struct Pose {
    at: Mm,
    turn: Rotation,
}

/// A report, in indices and quanta. Points are hull-local, times from first contact.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Record {
    /// [`ShotVocabulary::fingerprint`].
    vocabulary: [u8; 8],
    vehicle: u16,
    round: u16,
    /// The projectile's name: the weapon that fired it.
    weapon: u16,
    /// Every servo of the vehicle, in the vocabulary's order.
    servos: Vec<Turn>,
    origin: Mm,
    muzzle: Option<Mm>,
    caliber_mm: f32,
    mass_kg: f32,
    speed_mps: f32,
    impact: WireImpact,
    plates: Vec<WirePlate>,
    bounces: Vec<WireBounce>,
    path: Vec<(i32, Mm)>,
    cones: Vec<WireCone>,
    burst: Option<WireBurst>,
    end: WireEnd,
    doses: Vec<WireDose>,
    components: Vec<WireComponent>,
    killed: Option<KnockoutReason>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct WireImpact {
    point: Mm,
    normal: Oct,
    incoming: Oct,
    incidence_deg: f32,
    verdict: ShotVerdict,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct WirePlate {
    volume: u16,
    t: i32,
    entry: Mm,
    exit: Option<Mm>,
    chord_mm: f32,
    /// The steel equivalent the record states, and the cost it charged: one number.
    cost_mm: f32,
    overmatched: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct WireBounce {
    volume: u16,
    t: i32,
    point: Mm,
    normal: Oct,
    incoming: Oct,
    outgoing: Oct,
}

/// One spall cone: its metadata, each ray's reach and each ray's hits. The ray count is the
/// reaches' count, the vocabulary's cone shape's; each ray's direction and strength are rebuilt
/// from the shape spread by `field`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct WireCone {
    t: i32,
    origin: Mm,
    axis: Oct,
    /// The turn about the axis from the shape's lateral axis to the throw's.
    roll: Turn,
    field: FieldDescriptor,
    strength: f32,
    /// Each ray's reach in stratum order, mm; [`REACH_OVERFLOW`] takes the next of `overflow`.
    reaches: Vec<u16>,
    overflow: Vec<u32>,
    /// Each distinct hit dose, in first-use order.
    doses: Vec<f32>,
    /// Every hit, rays in order, a ray's hits in flight order.
    hits: Vec<WireHit>,
}

/// A burst: when and where, the carried and the greatest sideways speed, the nose piece's mass, and
/// its field, a cone in the vocabulary's burst shape, the ring's.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct WireBurst {
    t: i32,
    point: Mm,
    speed_mps: f32,
    sideways_mps: f32,
    nose_kg: f32,
    field: Option<WireCone>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
struct WireHit {
    /// Rays after the previous hit's (after ray 0, for the first).
    skip: u16,
    volume: u16,
    /// Its place in the cone's doses.
    dose: u16,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
enum WireEnd {
    /// At rest in a volume; `None` where the march named none.
    Rest(Option<u16>),
    Left(Mm),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct WireDose {
    volume: u16,
    /// The game's damage model: HP points before conversion, as in `BoxDose`.
    line: f32,
    field: f32,
    /// Pool units the shot took, as in `BoxDose`.
    taken: u32,
    /// The game's damage model: the shown HP points the shot took, as in `BoxDose`; `None` where
    /// what the pool held at the shot less `taken` gives them.
    shown_taken: Option<u32>,
    entered: Option<Mm>,
    /// When the shot emptied it, and what that cost.
    emptied: Option<(i32, ConsequenceState)>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
struct WireComponent {
    volume: u16,
    /// Pool units its pool held at the shot.
    held: u32,
}

// --- Build and project ----------------------------------------------------------------------------

/// An explanation built once on the authority: the full record, before any recipient's projection.
#[derive(Clone, Debug)]
pub(crate) struct BuiltExplanation {
    full: ShotExplanation,
}

impl BuiltExplanation {
    /// The explanation of `recorded` against `vocabulary`. Refused for a record no match produced
    /// (no shot identity or no struck life), a synthetic one, or one holding a name or a cone the
    /// vocabulary does not.
    pub(crate) fn build(
        vocabulary: &ShotVocabulary,
        recorded: &RecordedShot,
    ) -> Result<Self, CodecError> {
        let Some(shot) = recorded.shot else {
            return refuse("the round has no shot identity");
        };
        let Some(struck) = recorded.life else {
            return refuse("the struck tank has no combatant life");
        };
        let hull = recorded
            .hull
            .map(|(at, turn)| {
                Ok::<_, CodecError>(Pose {
                    at: Mm::of_vec(at)?,
                    turn: Rotation::of(turn)?,
                })
            })
            .transpose()?;
        Ok(Self {
            full: ShotExplanation {
                shot,
                encounter: recorded.encounter,
                struck,
                contact_tick: recorded.contact_tick,
                completion_tick: recorded.completion_tick,
                outcome: recorded.outcome,
                audience: Audience::Victim,
                hull,
                record: encode(vocabulary, &recorded.report)?,
            },
        })
    }

    /// Who gets what: the struck tank's combatant its full record (with the kill cam's lesson when
    /// this shot killed it), the shooter this shot's outcome without an unreached component, one
    /// message per combatant, the victim's where the two are one. Nobody else is listed.
    pub(crate) fn projections(&self) -> Vec<(CombatantId, ShotExplanation)> {
        let victim = self.full.struck.combatant;
        let shooter = self.full.shot.combatant;
        let mut out = vec![(
            victim,
            ShotExplanation {
                audience: if self.full.record.killed.is_some() {
                    Audience::KilledVictim
                } else {
                    Audience::Victim
                },
                ..self.full.clone()
            },
        )];
        if shooter != victim {
            out.push((
                shooter,
                ShotExplanation {
                    audience: Audience::Shooter,
                    record: shooter_view(&self.full.record),
                    ..self.full.clone()
                },
            ));
        }
        out
    }
}

/// `record` without a component its doses do not name.
fn shooter_view(record: &Record) -> Record {
    let mut view = record.clone();
    view.components.retain(|component| {
        record
            .doses
            .iter()
            .any(|dose| dose.volume == component.volume)
    });
    view
}

impl ShotExplanation {
    /// The struck hull node's pose (WORLD) at contact; refused for a rotation the encoder could
    /// not have written.
    pub(crate) fn hull_pose(&self) -> Result<Option<(Vec3, Quat)>, CodecError> {
        self.hull
            .map(|pose| Ok((Vec3::from(pose.at.point()), pose.turn.rotation()?)))
            .transpose()
    }

    /// The kill cam's lesson is this recipient's.
    pub(crate) fn lesson(&self) -> bool {
        self.audience == Audience::KilledVictim
    }

    /// The report this explanation carries, read against `vocabulary`; refused when the vocabulary
    /// folds another fingerprint, an index is out of it, a number is not finite, or the hull pose
    /// is refused ([`Self::hull_pose`]). A shooter's report lists only the components the shot
    /// reached. The report names no shooter: [`Self::shot`] does.
    pub(crate) fn decode(
        &self,
        vocabulary: &ShotVocabulary,
    ) -> Result<PenetrationReport, CodecError> {
        self.hull_pose()?;
        decode(vocabulary, &self.record)
    }
}

// --- Encode ---------------------------------------------------------------------------------------

fn encode(vocabulary: &ShotVocabulary, report: &PenetrationReport) -> Result<Record, CodecError> {
    if report.synthetic {
        return refuse("a synthetic report is never sent");
    }
    let Some(file) = report.vehicle.as_deref() else {
        return refuse("the report names no vehicle");
    };
    let vehicle_index = vocabulary.vehicle(file)?;
    let vehicle = vocabulary.vehicle_at(vehicle_index)?;
    for name in report.servo_deg.keys() {
        ShotVocabulary::named(&vehicle.servos, name, "servo")?;
    }
    let servos = vehicle
        .servos
        .iter()
        .map(|name| Turn::of_degrees(report.servo_deg.get(name).copied().unwrap_or(0.0)))
        .collect::<Result<_, _>>()?;
    let plates = report
        .plates
        .iter()
        .map(|plate| {
            let volume = vehicle.volume(&plate.name)?;
            let class = match vehicle.volume_at(volume)?.1 {
                VolumeWord::Armour(class) => class,
                VolumeWord::Component { .. } => {
                    return refuse(format!("plate {} is a component", plate.name));
                }
            };
            if class != plate.class || plate.steel_equiv_mm != plate.cost_mm {
                return refuse(format!(
                    "plate {} states a class or a cost the vocabulary does not",
                    plate.name
                ));
            }
            Ok(WirePlate {
                volume,
                t: micros(plate.t)?,
                entry: Mm::of(plate.entry)?,
                exit: plate.exit.map(Mm::of).transpose()?,
                chord_mm: plate.chord_mm,
                cost_mm: plate.cost_mm,
                overmatched: plate.overmatched,
            })
        })
        .collect::<Result<_, _>>()?;
    let bounces = report
        .ricochets
        .iter()
        .map(|bounce| {
            Ok(WireBounce {
                volume: vehicle.volume(&bounce.volume)?,
                t: micros(bounce.t)?,
                point: Mm::of(bounce.point)?,
                normal: Oct::of(bounce.normal)?,
                incoming: Oct::of(bounce.incoming)?,
                outgoing: Oct::of(bounce.outgoing)?,
            })
        })
        .collect::<Result<_, CodecError>>()?;
    let path = report
        .path
        .iter()
        .map(|node| Ok((micros(node.t)?, Mm::of(node.point)?)))
        .collect::<Result<_, CodecError>>()?;
    let cones = report
        .spall
        .iter()
        .map(|cone| encode_cone(&vocabulary.cone, vehicle, cone))
        .collect::<Result<_, _>>()?;
    let burst = report
        .burst
        .as_ref()
        .map(|burst| {
            Ok::<_, CodecError>(WireBurst {
                t: micros(burst.t)?,
                point: Mm::of(burst.point)?,
                speed_mps: burst.speed_mps,
                sideways_mps: burst.sideways_mps,
                nose_kg: burst.nose_kg,
                field: burst
                    .field
                    .as_ref()
                    .map(|field| {
                        super::check_burst_field(field.field)
                            .map_err(|why| CodecError(why.into()))?;
                        encode_cone(vocabulary.burst.shape(), vehicle, field)
                    })
                    .transpose()?,
            })
        })
        .transpose()?;
    let end = match &report.end {
        RoundEnd::Rest { volume } if volume.is_empty() => WireEnd::Rest(None),
        RoundEnd::Rest { volume } => WireEnd::Rest(Some(vehicle.volume(volume)?)),
        RoundEnd::Left { at } => WireEnd::Left(Mm::of(*at)?),
    };
    for consequence in &report.consequences {
        if !report
            .doses
            .iter()
            .any(|dose| dose.volume == consequence.volume)
        {
            return refuse(format!(
                "a consequence on {} with no dose",
                consequence.volume
            ));
        }
    }
    let doses = report
        .doses
        .iter()
        .map(|dose| encode_dose(vehicle, report, dose))
        .collect::<Result<_, _>>()?;
    let components = report
        .components
        .iter()
        .map(|component| {
            let volume = vehicle.component(&component.name)?;
            let (_, role, count, _) = vehicle.component_at(volume)?;
            if role != component.role || count != component.count {
                return refuse(format!(
                    "{} states a role or a pool the vocabulary does not",
                    component.name
                ));
            }
            Ok(WireComponent {
                volume,
                held: component.held,
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(Record {
        vocabulary: vocabulary.fingerprint.to_le_bytes(),
        vehicle: vehicle_index,
        round: ShotVocabulary::named(&vocabulary.rounds, &report.round, "round")?,
        weapon: ShotVocabulary::named(&vocabulary.weapons, &report.projectile.name, "weapon")?,
        servos,
        origin: Mm::of(report.origin)?,
        muzzle: report.muzzle.map(Mm::of).transpose()?,
        caliber_mm: report.projectile.caliber_mm,
        mass_kg: report.projectile.mass_kg,
        speed_mps: report.projectile.speed_mps,
        impact: WireImpact {
            point: Mm::of(report.impact.point)?,
            normal: Oct::of(report.impact.normal)?,
            incoming: Oct::of(report.impact.incoming)?,
            incidence_deg: report.impact.incidence_deg,
            verdict: report.impact.verdict,
        },
        plates,
        bounces,
        path,
        cones,
        burst,
        end,
        doses,
        components,
        killed: report.killed,
    })
}

fn encode_dose(
    vehicle: &VehicleWords,
    report: &PenetrationReport,
    dose: &BoxDose,
) -> Result<WireDose, CodecError> {
    let volume = vehicle.component(&dose.volume)?;
    let (_, _, _, function) = vehicle.component_at(volume)?;
    let mut emptied = None;
    for consequence in report
        .consequences
        .iter()
        .filter(|consequence| consequence.volume == dose.volume)
    {
        if emptied.is_some() || consequence.function.as_deref() != function {
            return refuse(format!(
                "{}: a second consequence, or a function the vocabulary does not hold",
                dose.volume
            ));
        }
        emptied = Some((micros(consequence.t)?, consequence.state));
    }
    Ok(WireDose {
        volume,
        line: dose.line,
        field: dose.field,
        taken: dose.taken,
        shown_taken: dose.shown_taken,
        entered: dose.entered.map(Mm::of).transpose()?,
        emptied,
    })
}

/// The roll (rad) about `axis` that places the cone's strata nearest its ray ends: the turn from
/// the shape's lateral axis to the throw's, each ray weighted by its reach.
fn fitted_roll(spread: &Spread, axis: Vec3, cone: &SpallCone) -> f32 {
    let x = ConeShape::lateral_axis(axis);
    let y = axis.cross(x);
    let origin = Vec3::from(cone.origin);
    let (mut sine, mut cosine) = (0.0f64, 0.0f64);
    for (fragment, [_, l0, l1, _]) in cone.fragments.iter().zip(spread.strata()) {
        let span = Vec3::from(fragment.end) - origin;
        let (a, b) = (span.dot(x) as f64, span.dot(y) as f64);
        let (l0, l1) = (l0 as f64, l1 as f64);
        sine += b * l0 - a * l1;
        cosine += a * l0 + b * l1;
    }
    sine.atan2(cosine) as f32
}

/// `cone` on the wire, its rays thrown in `shape`; refused where its rays are not `shape`'s under
/// its field.
fn encode_cone(
    shape: &ConeShape,
    vehicle: &VehicleWords,
    cone: &SpallCone,
) -> Result<WireCone, CodecError> {
    let spread = match shape.spread(cone.field) {
        Ok(spread) => spread,
        Err(error) => return refuse(format!("a cone's field is outside its law: {error}")),
    };
    if cone.fragments.len() != spread.directions_count()
        || cone.half_angle_deg != cone.field.half_angle().to_degrees()
    {
        return refuse(format!(
            "a cone of {} rays at {}° is not the vocabulary's shape under its field {:?}",
            cone.fragments.len(),
            cone.half_angle_deg,
            cone.field
        ));
    }
    for (fragment, [.., weight]) in cone.fragments.iter().zip(spread.strata()) {
        if fragment.strength.to_bits() != (cone.strength * weight).to_bits() {
            return refuse("a ray's strength is not its weight under the cone's field times S");
        }
    }
    let axis = Oct::of(cone.axis)?;
    let roll = Turn::of_radians(fitted_roll(&spread, axis.direction(), cone))?;
    let origin = Vec3::from(cone.origin);
    let mut reaches = Vec::with_capacity(cone.fragments.len());
    let mut overflow = Vec::new();
    let mut doses: Vec<f32> = Vec::new();
    let mut hits = Vec::new();
    let mut previous = 0;
    for (ray, fragment) in cone.fragments.iter().enumerate() {
        let reach = (Vec3::from(fragment.end) - origin).length();
        let reach_mm = (reach as f64 * 1000.0).round();
        if !(0.0..=u32::MAX as f64).contains(&reach_mm) {
            return refuse(format!("a ray's reach {reach} m is outside u32 mm"));
        }
        match u16::try_from(reach_mm as u32) {
            Ok(mm) if mm != REACH_OVERFLOW => reaches.push(mm),
            _ => {
                reaches.push(REACH_OVERFLOW);
                overflow.push(reach_mm as u32);
            }
        }
        for hit in &fragment.hits {
            let dose = match doses
                .iter()
                .position(|known| known.to_bits() == hit.damage.to_bits())
            {
                Some(at) => at,
                None => {
                    doses.push(hit.damage);
                    doses.len() - 1
                }
            };
            hits.push(WireHit {
                skip: index_u16(ray - previous, "ray")?,
                volume: vehicle.component(&hit.volume)?,
                dose: index_u16(dose, "dose")?,
            });
            previous = ray;
        }
    }
    Ok(WireCone {
        t: micros(cone.t)?,
        origin: Mm::of(cone.origin)?,
        axis,
        roll,
        field: cone.field,
        strength: cone.strength,
        reaches,
        overflow,
        doses,
        hits,
    })
}

// --- Decode ---------------------------------------------------------------------------------------

/// Every `f32` of `record` is finite.
fn finite(record: &Record) -> Result<(), CodecError> {
    let values = [
        record.caliber_mm,
        record.mass_kg,
        record.speed_mps,
        record.impact.incidence_deg,
    ]
    .into_iter()
    .chain(
        record
            .plates
            .iter()
            .flat_map(|plate| [plate.chord_mm, plate.cost_mm]),
    )
    .chain(
        record
            .cones
            .iter()
            .chain(record.burst.iter().filter_map(|burst| burst.field.as_ref()))
            .flat_map(|cone| {
                [cone.field.ratio, cone.strength]
                    .into_iter()
                    .chain(cone.doses.iter().copied())
            }),
    )
    .chain(
        record
            .burst
            .iter()
            .flat_map(|burst| [burst.speed_mps, burst.sideways_mps, burst.nose_kg]),
    )
    .chain(record.doses.iter().flat_map(|dose| [dose.line, dose.field]));
    for value in values {
        if !value.is_finite() {
            return refuse(format!("a number of the record is {value}"));
        }
    }
    Ok(())
}

fn decode(vocabulary: &ShotVocabulary, record: &Record) -> Result<PenetrationReport, CodecError> {
    finite(record)?;
    let fingerprint = u64::from_le_bytes(record.vocabulary);
    if fingerprint != vocabulary.fingerprint {
        return refuse(format!(
            "the explanation's vocabulary {fingerprint:#018x} is not this client's {:#018x}",
            vocabulary.fingerprint
        ));
    }
    let vehicle = vocabulary.vehicle_at(record.vehicle)?;
    if record.servos.len() != vehicle.servos.len() {
        return refuse("the servo count is not the vehicle's");
    }
    let servo_deg: BTreeMap<String, f32> = vehicle
        .servos
        .iter()
        .zip(&record.servos)
        .map(|(name, turn)| (name.clone(), turn.degrees()))
        .collect();
    let name = |index: u16| vehicle.volume_at(index).map(|(name, _)| name.clone());
    let plates = record
        .plates
        .iter()
        .map(|plate| {
            let (name, word) = vehicle.volume_at(plate.volume)?;
            let VolumeWord::Armour(class) = word else {
                return refuse(format!("plate {name} is a component"));
            };
            let chord = plate.chord_mm / 1000.0;
            Ok(Plate {
                name: name.clone(),
                t: seconds(plate.t),
                entry: plate.entry.point(),
                exit: plate.exit.map(Mm::point),
                chord_mm: plate.chord_mm,
                steel_equiv_mm: plate.cost_mm,
                cost_mm: plate.cost_mm,
                overmatched: plate.overmatched,
                class: *class,
                factor: if chord > 0.0 {
                    plate.cost_mm / chord
                } else {
                    0.0
                },
            })
        })
        .collect::<Result<_, CodecError>>()?;
    let ricochets = record
        .bounces
        .iter()
        .map(|bounce| {
            Ok(Ricochet {
                t: seconds(bounce.t),
                point: bounce.point.point(),
                normal: bounce.normal.direction().to_array(),
                incoming: bounce.incoming.direction().to_array(),
                outgoing: bounce.outgoing.direction().to_array(),
                volume: name(bounce.volume)?,
            })
        })
        .collect::<Result<_, CodecError>>()?;
    let spall = record
        .cones
        .iter()
        .map(|cone| decode_cone(&vocabulary.cone, vehicle, cone))
        .collect::<Result<_, _>>()?;
    let burst = record
        .burst
        .as_ref()
        .map(|burst| {
            Ok::<_, CodecError>(Burst {
                t: seconds(burst.t),
                point: burst.point.point(),
                speed_mps: burst.speed_mps,
                sideways_mps: burst.sideways_mps,
                nose_kg: burst.nose_kg,
                field: burst
                    .field
                    .as_ref()
                    .map(|field| {
                        super::check_burst_field(field.field)
                            .map_err(|why| CodecError(why.into()))?;
                        decode_cone(vocabulary.burst.shape(), vehicle, field)
                    })
                    .transpose()?,
            })
        })
        .transpose()?;
    let end = match &record.end {
        WireEnd::Rest(None) => RoundEnd::Rest {
            volume: String::new(),
        },
        WireEnd::Rest(Some(volume)) => RoundEnd::Rest {
            volume: name(*volume)?,
        },
        WireEnd::Left(at) => RoundEnd::Left { at: at.point() },
    };
    let mut doses = Vec::with_capacity(record.doses.len());
    let mut consequences = Vec::new();
    for dose in &record.doses {
        let (name, _, _, function) = vehicle.component_at(dose.volume)?;
        doses.push(BoxDose {
            volume: name.to_owned(),
            line: dose.line,
            field: dose.field,
            taken: dose.taken,
            shown_taken: dose.shown_taken,
            entered: dose.entered.map(Mm::point),
        });
        if let Some((t, state)) = dose.emptied {
            consequences.push(Consequence {
                t: seconds(t),
                volume: name.to_owned(),
                state,
                function: function.map(str::to_owned),
            });
        }
    }
    consequences.sort_by(|a, b| a.t.total_cmp(&b.t));
    let components = record
        .components
        .iter()
        .map(|component| {
            let (name, role, count, _) = vehicle.component_at(component.volume)?;
            Ok(ComponentState {
                name: name.to_owned(),
                role,
                count,
                held: component.held,
            })
        })
        .collect::<Result<_, CodecError>>()?;
    Ok(PenetrationReport {
        synthetic: false,
        vehicle: Some(vehicle.file.clone()),
        servo_deg,
        round: ShotVocabulary::name_at(&vocabulary.rounds, record.round, "round")?.to_owned(),
        shooter: None,
        origin: record.origin.point(),
        muzzle: record.muzzle.map(Mm::point),
        projectile: Projectile {
            name: ShotVocabulary::name_at(&vocabulary.weapons, record.weapon, "weapon")?.to_owned(),
            caliber_mm: record.caliber_mm,
            mass_kg: record.mass_kg,
            speed_mps: record.speed_mps,
        },
        impact: Impact {
            point: record.impact.point.point(),
            normal: record.impact.normal.direction().to_array(),
            incoming: record.impact.incoming.direction().to_array(),
            incidence_deg: record.impact.incidence_deg,
            verdict: record.impact.verdict,
        },
        plates,
        ricochets,
        path: record
            .path
            .iter()
            .map(|&(t, point)| PathPoint {
                t: seconds(t),
                point: point.point(),
            })
            .collect(),
        spall,
        burst,
        end,
        doses,
        consequences,
        components,
        killed: record.killed,
    })
}

/// `cone` off the wire, its rays rebuilt from `shape` spread by its field.
fn decode_cone(
    shape: &ConeShape,
    vehicle: &VehicleWords,
    cone: &WireCone,
) -> Result<SpallCone, CodecError> {
    let spread = match shape.spread(cone.field) {
        Ok(spread) => spread,
        Err(error) => return refuse(format!("a cone's field is outside its law: {error}")),
    };
    if cone.reaches.len() != spread.directions_count() {
        return refuse("the cone's ray count is not the vocabulary's shape");
    }
    let axis = cone.axis.direction();
    let Ok(dir) = Dir3::new(axis) else {
        return refuse("the cone has no axis");
    };
    let (sin, cos) = cone.roll.radians().sin_cos();
    let x = ConeShape::lateral_axis(axis);
    let lateral = x * cos + axis.cross(x) * sin;
    let origin = Vec3::from(cone.origin.point());
    let mut overflow = cone.overflow.iter();
    let mut fragments = Vec::with_capacity(cone.reaches.len());
    for (&reach, (direction, profile)) in cone
        .reaches
        .iter()
        .zip(spread.directions_about(dir, lateral))
    {
        let mm = if reach == REACH_OVERFLOW {
            *overflow
                .next()
                .map_or_else(|| refuse("a reach overflows with no overflow left"), Ok)?
        } else {
            reach as u32
        };
        let reach = (mm as f64 / 1000.0) as f32;
        fragments.push(Fragment {
            end: (origin + Vec3::from(direction) * reach).to_array(),
            strength: cone.strength * profile,
            hits: Vec::new(),
        });
    }
    let mut ray = 0;
    for hit in &cone.hits {
        ray += hit.skip as usize;
        let Some(fragment) = fragments.get_mut(ray) else {
            return refuse("a hit past the cone's last ray");
        };
        let (name, ..) = vehicle.component_at(hit.volume)?;
        let Some(&damage) = cone.doses.get(hit.dose as usize) else {
            return refuse("a hit's dose is past the cone's doses");
        };
        fragment.hits.push(FragmentHit {
            volume: name.to_owned(),
            damage,
        });
    }
    Ok(SpallCone {
        t: seconds(cone.t),
        origin: origin.to_array(),
        axis: axis.to_array(),
        field: cone.field,
        half_angle_deg: cone.field.half_angle().to_degrees(),
        strength: cone.strength,
        fragments,
    })
}

#[cfg(test)]
mod tests;
