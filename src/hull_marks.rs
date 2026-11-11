//! Hull marks, the visual layer of the game's damage model: the scratch a ricochet leaves on a
//! vehicle, the dent where its armour stopped a round, the hole where a round went in. A mark is
//! drawn on the hull and read by nobody else; the simulation never reads one (ADR-0014).
//!
//! # The record
//!
//! One [`HullMarks`] per vehicle root: server-owned, public to every client (ADR-0039: a mark on a
//! hull is seen by anyone with eyes on it), born empty in the root's spawn bundle and replicated
//! whole on every change. It holds at most [`MARK_CAP`] marks; recording one more evicts the
//! oldest mark with the smallest physical footprint. It lasts the root's life.
//!
//! A [`HullMark`] is packed plain data, built from the authority's [`ShellStrike`]: its kind
//! ([`MarkKind::of`] the strike's verdict), the struck part (an index into the root's
//! [`SimParts`] names, sorted), the impact segment in that part's frame (the round came along
//! `along` and met the face at `at`), the face normal in the same frame, the round's calibre and a
//! roll seed hashed from the shot. Marks ride the part they were made on, so a turret mark turns
//! with the turret. Every positive calibre uses the same bounded record.
//!
//! # Who mounts what
//!
//! Whichever app is the authority, the dedicated server or the single-player composition, mounts
//! [`plugin`], which builds each mark with the [`MarkBuilder`] and writes it to the component;
//! replication carries the record to clients. The view — decals, textures, the client's diff — is
//! `vfx::hull_marks`, mounted by every windowed composition.

pub(crate) mod demo;

use std::collections::VecDeque;

use avian3d::prelude::{Position, Rotation};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ballistics::{ShellStrike, Verdict};
use crate::damage::VolumeOf;
use crate::tank::{SimParts, rig_world_pose};

/// Resolution: bound per-vehicle record storage and decal work to the owner's about-50 budget.
pub(crate) const MARK_CAP: usize = 50;

fn stretch_from_ron(text: &str) -> Result<f32, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Look {
        max_stretch: crate::provenance::Sourced<f32>,
    }
    let look: Look = ron::from_str(text).map_err(|err| {
        format!("max_stretch: {err}; author (ratio, Taste(\"appearance reason\"))")
    })?;
    let stretch = look.max_stretch.0;
    if !stretch.is_finite() || stretch < 1.0 {
        return Err(
            "max_stretch must be finite and at least one; author a grazing-mark stretch ratio"
                .to_string(),
        );
    }
    Ok(stretch)
}

/// The game's damage-model view's grazing-mark Taste, admitted before recording or drawing.
pub(crate) fn max_stretch() -> f32 {
    static STRETCH: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
        crate::provenance::shipped("assets/vfx/hull_marks.ron", || {
            stretch_from_ron(include_str!("../assets/vfx/hull_marks.ron"))
        })
        .unwrap_or_else(|err| panic!("assets/vfx/hull_marks.ron: {err}"))
    });
    *STRETCH
}

/// Which mark a strike leaves.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MarkKind {
    /// A gouge along the bounce.
    Scratch,
    /// A bowl with crazed paint, no core.
    Dent,
    /// A dark core, a lip and an exposed-substrate ring.
    Hole,
}

impl MarkKind {
    /// Every kind, in declaration order.
    pub(crate) const ALL: [Self; 3] = [Self::Scratch, Self::Dent, Self::Hole];

    /// The mark a strike leaves, from its public verdict at the face alone, so every observer's
    /// hull agrees with the outcome every observer was told (the public flight outcome).
    ///
    /// A round that came to rest inside the tank after going through the face (lodged) has the
    /// face verdict `Through`, and leaves the same hole as one that left the tank. `Struck` is a
    /// terrain stop with no crossing resolved and claims nothing about an armour face, so it
    /// leaves nothing. An armour walk contradiction is a bug and panics.
    pub(crate) fn of(verdict: Verdict) -> Option<Self> {
        match verdict {
            Verdict::Ricochet => Some(Self::Scratch),
            Verdict::Stopped => Some(Self::Dent),
            Verdict::Through => Some(Self::Hole),
            Verdict::Struck => None,
        }
    }
}

/// How far a mark stretches along the shot's in-plane direction: 1 / cos θ, θ the obliquity
/// between the round's travel `along` and the face normal `normal` (both unit), clamped at
/// [`max_stretch`] — which also answers a travel parallel to the face, where 1 / cos θ has no
/// value.
pub(crate) fn stretch(along: Vec3, normal: Vec3) -> f32 {
    let max = max_stretch();
    let cos = along.dot(normal).abs();
    if cos * max <= 1.0 { max } else { 1.0 / cos }
}

/// The shot's direction across the face: `along` with its component on `normal` removed. A
/// strike square to the face has none, and its mark is round; any direction on the face is
/// returned then.
pub(crate) fn in_plane(along: Vec3, normal: Vec3) -> Vec3 {
    (along - normal * along.dot(normal))
        .try_normalize()
        .unwrap_or_else(|| normal.any_orthonormal_vector())
}

/// The game's damage-model view: an exact, finite positive calibre in metres. The bits give
/// records reflexive equality while the serde conversion refuses invalid outside input.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(try_from = "f32", into = "f32")]
pub(crate) struct MarkCaliber(u32);

#[derive(Debug)]
pub(crate) enum MarkCaliberError {
    Invalid(f32),
}

impl std::fmt::Display for MarkCaliberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(value) => {
                write!(f, "mark calibre must be finite and positive, got {value}")
            }
        }
    }
}

impl TryFrom<f32> for MarkCaliber {
    type Error = MarkCaliberError;
    fn try_from(value: f32) -> Result<Self, Self::Error> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value.to_bits()))
        } else {
            Err(MarkCaliberError::Invalid(value))
        }
    }
}

impl From<MarkCaliber> for f32 {
    fn from(value: MarkCaliber) -> Self {
        Self::from_bits(value.0)
    }
}

/// The game's damage-model view: a nonzero snorm8 direction, checked at its input edge.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(try_from = "[i8; 3]", into = "[i8; 3]")]
pub(crate) struct PackedDirection([i8; 3]);

#[derive(Debug)]
pub(crate) enum PackedDirectionError {
    Zero,
}

impl std::fmt::Display for PackedDirectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zero => write!(f, "a packed mark direction must be nonzero"),
        }
    }
}

impl TryFrom<[i8; 3]> for PackedDirection {
    type Error = PackedDirectionError;
    fn try_from(value: [i8; 3]) -> Result<Self, Self::Error> {
        if value == [0; 3] {
            Err(PackedDirectionError::Zero)
        } else {
            Ok(Self(value))
        }
    }
}

impl From<PackedDirection> for [i8; 3] {
    fn from(value: PackedDirection) -> Self {
        value.0
    }
}

impl PackedDirection {
    fn direction(self) -> Vec3 {
        Vec3::from_array(self.0.map(f32::from)).normalize()
    }
}

/// One mark, packed for the wire. Positions are in the struck part's frame, so the mark rides the
/// part; the view re-derives everything else (size, stretch, roll, material) from these fields.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HullMark {
    pub(crate) feedback: crate::substances::SurfaceFeedback,
    pub(crate) kind: MarkKind,
    /// The struck part: its index in the root's [`SimParts`] names, sorted. Both ends build the
    /// part table from the same tank data, so they number it the same.
    pub(crate) part: u16,
    /// Where the round met the face, part frame, millimetres.
    pub(crate) at: [i16; 3],
    /// The round's direction of travel as it met the face, part frame, unit vector in snorm8.
    pub(crate) along: PackedDirection,
    /// The face's outward normal, part frame, unit vector in snorm8.
    pub(crate) normal: PackedDirection,
    /// The round's exact calibre, metres; no presentation-size ceiling or invented minimum.
    pub(crate) caliber: MarkCaliber,
    /// Hashed from the shot and the strike's ordinal in it; the view takes the mark's roll and
    /// texture variant from it.
    pub(crate) seed: u16,
}

/// The resolved face in the struck part's frame; packing preserves these three geometric facts.
pub(crate) struct MarkFace {
    pub at: Vec3,
    pub along: Vec3,
    pub normal: Vec3,
}

impl HullMark {
    /// Pack a strike already expressed in its part's frame. `None` when the point does not fit
    /// the wire's ±32.767 m, a direction has no length, or calibre is nonfinite/nonpositive.
    pub(crate) fn pack(
        kind: MarkKind,
        part: u16,
        face: MarkFace,
        caliber: f32,
        seed: u16,
        feedback: crate::substances::SurfaceFeedback,
    ) -> Option<Self> {
        let MarkFace { at, along, normal } = face;
        let mm = |metres: f32| {
            let mm = (metres * 1000.0).round();
            (mm.is_finite() && mm.abs() <= i16::MAX as f32).then_some(mm as i16)
        };
        let snorm = |direction: Vec3| {
            let unit = direction.try_normalize()?;
            Some(
                PackedDirection::try_from(unit.to_array().map(|c| (c * 127.0).round() as i8))
                    .expect("a unit vector quantizes to a nonzero snorm8 direction"),
            )
        };
        Some(Self {
            feedback,
            kind,
            part,
            at: [mm(at.x)?, mm(at.y)?, mm(at.z)?],
            along: snorm(along)?,
            normal: snorm(normal)?,
            caliber: MarkCaliber::try_from(caliber).ok()?,
            seed,
        })
    }

    /// Where the round met the face, part frame, metres.
    pub(crate) fn at(&self) -> Vec3 {
        Vec3::from_array(self.at.map(f32::from)) * 0.001
    }

    /// The round's direction of travel, part frame, unit.
    pub(crate) fn along(&self) -> Vec3 {
        self.along.direction()
    }

    /// The face's outward normal, part frame, unit.
    pub(crate) fn normal(&self) -> Vec3 {
        self.normal.direction()
    }

    /// The round's calibre, metres.
    pub(crate) fn caliber(&self) -> f32 {
        self.caliber.into()
    }
}

/// The game's damage-model view keeps marks in arrival order, with stable serials even when
/// a smaller mark between two larger ones is evicted. Serial ordering assumes fewer than 2³²
/// strikes per vehicle lifetime.
#[derive(Component, Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct HullMarks {
    marks: VecDeque<(u32, HullMark)>,
    /// Marks ever recorded on this vehicle, the dropped included.
    recorded: u32,
}

impl HullMarks {
    /// The game's damage-model view evicts the oldest of the smallest held footprints before
    /// appending. Even a full record admits the new strike; a smaller flood then replaces itself.
    /// Physical footprint is calibre² / cos θ, with the same grazing clamp the view uses.
    pub(crate) fn record(&mut self, mark: HullMark) {
        if self.marks.len() == MARK_CAP {
            let footprint =
                |mark: &HullMark| mark.caliber().powi(2) * stretch(mark.along(), mark.normal());
            let smallest = self
                .marks
                .iter()
                .enumerate()
                .min_by(|(_, (_, a)), (_, (_, b))| footprint(a).total_cmp(&footprint(b)))
                .map(|(index, _)| index)
                .expect("a full mark record is nonempty");
            self.marks.remove(smallest);
        }
        self.marks.push_back((self.recorded, mark));
        self.recorded = self.recorded.wrapping_add(1);
    }

    /// Marks ever recorded on this vehicle, the dropped included.
    pub(crate) fn recorded(&self) -> u32 {
        self.recorded
    }

    /// Every held mark with its serial, oldest first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, &HullMark)> {
        self.marks.iter().map(|(serial, mark)| (*serial, mark))
    }

    /// How many marks are held.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.marks.len()
    }
}

/// The index [`HullMark::part`] gives the part named `name`: its place among `parts`' names,
/// sorted. `None` for a name the table does not hold, or past `u16`.
pub(crate) fn part_index(parts: &SimParts, name: &str) -> Option<u16> {
    let mut names: Vec<&str> = parts.0.keys().map(String::as_str).collect();
    names.sort_unstable();
    let index = names.binary_search(&name).ok()?;
    u16::try_from(index).ok()
}

/// The part entities in [`HullMark::part`] order: `parts` sorted by name.
pub(crate) fn parts_in_index_order(parts: &SimParts) -> Vec<Entity> {
    let mut named: Vec<(&str, Entity)> = parts
        .0
        .iter()
        .map(|(name, &entity)| (name.as_str(), entity))
        .collect();
    named.sort_unstable_by_key(|&(name, _)| name);
    named.into_iter().map(|(_, entity)| entity).collect()
}

/// Everything the authority reads to turn a [`ShellStrike`] into a [`HullMark`]: the struck
/// volume's tank, the part the volume belongs to, and that part's tick-truth pose. Read-only on
/// every simulation component.
#[derive(SystemParam)]
pub(crate) struct MarkBuilder<'w, 's> {
    owners: Query<'w, 's, &'static VolumeOf>,
    marked_roots: Query<'w, 's, (), With<HullMarks>>,
    parents: Query<'w, 's, &'static ChildOf>,
    locals: Query<'w, 's, &'static Transform>,
    names: Query<'w, 's, &'static Name>,
    roots:
        Query<'w, 's, (&'static SimParts, &'static Position, &'static Rotation), With<HullMarks>>,
}

impl MarkBuilder<'_, '_> {
    /// The vehicle root a strike marks, and its mark. `None` when the verdict leaves no mark or
    /// the struck volume is not a part of a root that keeps marks (a tree, a turret thrown clear of its hull). Packing failure violates the
    /// bake's coordinate bound or the resolved direction invariants and is a bug.
    pub(crate) fn mark(&self, strike: &ShellStrike) -> Option<(Entity, HullMark)> {
        let kind = MarkKind::of(strike.verdict)?;
        let Ok(owner) = self.owners.get(strike.struck) else {
            return None; // Standalone environment volumes have no vehicle ownership or hull record.
        };
        let root = owner.tank();
        if !self.marked_roots.contains(root) {
            return None; // Trees, structures and independently simulated detached parts keep no hull record.
        }
        let (parts, position, rotation) = self
            .roots
            .get(root)
            .expect("a mark-keeping tank root has its complete physics pose");
        let mut current = strike.struck;
        let mut marked_part = None;
        while current != root {
            if marked_part.is_none()
                && let Ok(name) = self.names.get(current)
                && parts.0.get(name.as_str()) == Some(&current)
            {
                marked_part = Some((current, name));
            }
            let Ok(parent) = self.parents.get(current) else {
                return None; // A thrown-clear part no longer belongs to the recorded hull's chain.
            };
            current = parent.parent();
        }
        let (part, name) = marked_part
            .expect("every struck volume under a tank belongs to its spawn-built part table");
        let index = part_index(parts, name.as_str())
            .expect("the part exists and the bake bounds the table to the mark wire's index");
        let (origin, turn) = rig_world_pose(
            part,
            root,
            position.0,
            rotation.0,
            &self.parents,
            &self.locals,
        )
        .expect("the verified ancestry has every spawn-time local pose");
        let to_part = turn.inverse();
        let mark = HullMark::pack(
            kind,
            index,
            MarkFace {
                at: to_part * (strike.at - origin),
                along: to_part * strike.incoming,
                normal: to_part * strike.normal,
            },
            strike.contact_caliber,
            seed(strike),
            strike.feedback,
        )
        .expect(
            "the bake bounds mark coordinates, resolved directions are finite and nonzero, and \
             the round registry refuses a calibre that is not finite and positive",
        );
        Some((root, mark))
    }
}

/// The strike's seed: its shot and its ordinal in the shot, hashed, so it is a pure function of
/// the strike and two strikes of one shot hash apart.
fn seed(strike: &ShellStrike) -> u16 {
    let shot = strike.shot;
    let mut bytes = [0u8; 17];
    bytes[..8].copy_from_slice(&shot.combatant.0.to_le_bytes());
    bytes[8] = shot.weapon;
    bytes[9..13].copy_from_slice(&shot.fire_tick.to_le_bytes());
    bytes[13..].copy_from_slice(&strike.sequence.to_le_bytes());
    let hash = crate::fingerprint::fnv1a_64(0xcbf2_9ce4_8422_2325, &bytes);
    (hash ^ (hash >> 16) ^ (hash >> 32) ^ (hash >> 48)) as u16
}

/// The authority's recorder: each strike's mark is written to the struck root's record.
pub(crate) fn plugin(app: &mut App) {
    max_stretch();
    app.add_observer(record);
}

fn record(strike: On<ShellStrike>, builder: MarkBuilder, mut records: Query<&mut HullMarks>) {
    if let Some((root, mark)) = builder.mark(&strike) {
        records
            .get_mut(root)
            .expect("MarkBuilder::mark returns only a root that keeps HullMarks")
            .record(mark);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{CombatantId, ShotId};

    /// The game's damage-model view refuses a missing provenance, nonfinite or shrinking
    /// grazing clamp before play. The law and record both rely on a finite ratio at least one.
    #[test]
    fn the_mark_stretch_taste_is_required_and_admitted_before_play() {
        assert_eq!(
            stretch_from_ron(r#"(max_stretch: (4.0, Fixture("fixture")))"#),
            Ok(4.0)
        );
        for text in [
            "(max_stretch: 4.0)",
            "(max_stretch: (0.5, Fixture(\"fixture\")))",
            "(max_stretch: (NaN, Fixture(\"fixture\")))",
            "(max_stretch: (inf, Fixture(\"fixture\")))",
        ] {
            let error = stretch_from_ron(text).unwrap_err();
            assert!(error.contains("max_stretch"), "{error}");
        }
    }

    /// The decal reader needs the authored physical size, including values beyond a byte of
    /// millimetres. Serde must also refuse nonpositive or nonfinite network values at its edge.
    #[test]
    fn mark_calibre_round_trips_without_saturation_and_refuses_invalid_input() {
        for calibre in [0.020_f32, 0.088, 0.280, 1.25] {
            // FIXTURE DATA, metres.
            let mut mark = numbered(0);
            mark.caliber = MarkCaliber::try_from(calibre).unwrap();
            let encoded = ron::ser::to_string(&mark).unwrap();
            let decoded: HullMark = ron::from_str(&encoded).unwrap();
            assert_eq!(decoded, mark);
            assert_eq!(decoded.caliber().to_bits(), calibre.to_bits());
        }
        for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(MarkCaliber::try_from(value).is_err());
        }
        for encoded in ["0.0", "-1.0", "NaN", "inf"] {
            assert!(ron::from_str::<MarkCaliber>(encoded).is_err());
        }
    }

    /// A mark whose fields all differ from its neighbours', told apart by `n`.
    pub(crate) fn numbered(n: u16) -> HullMark {
        HullMark {
            feedback: crate::substances::SurfaceFeedback::fixture(),
            kind: MarkKind::ALL[n as usize % 3],
            part: n % 3,
            at: [n as i16, -(n as i16), 7],
            along: PackedDirection::try_from([0, 0, -127]).unwrap(),
            normal: PackedDirection::try_from([0, 127, 0]).unwrap(),
            caliber: MarkCaliber::try_from(0.088).expect("positive finite fixture calibre"),
            seed: n.wrapping_mul(7919),
        }
    }

    /// A struck tank as the recorder sees it: a root posed in the world, a turret part turned on
    /// it, and a plate under the turret whose struck volume is a collider child of its node, as a
    /// mixed-substance node's is.
    pub(crate) struct Fixture {
        pub(crate) root: Entity,
        pub(crate) turret: Entity,
        pub(crate) plate: Entity,
        pub(crate) collider: Entity,
    }

    /// Fixture data: the root's pose and the turret's turn, arbitrary and non-trivial so a frame
    /// mistake moves the answer.
    const ROOT_AT: Vec3 = Vec3::new(14.0, 2.0, -9.0);
    const ROOT_YAW: f32 = 0.7;

    /// Spawn the fixture into `world`.
    pub(crate) fn fixture_in(world: &mut World) -> Fixture {
        let root = world
            .spawn((
                Position(ROOT_AT),
                Rotation(Quat::from_rotation_y(ROOT_YAW)),
                HullMarks::default(),
            ))
            .id();
        let turret = world
            .spawn((
                Name::new("Turret"),
                Transform::from_xyz(0.0, 1.7, 0.2).with_rotation(Quat::from_rotation_y(-1.1)),
                ChildOf(root),
            ))
            .id();
        let plate = world
            .spawn((
                Name::new("Turret_Front"),
                Transform::from_xyz(0.0, 0.3, -1.0),
                ChildOf(turret),
                VolumeOf(root),
            ))
            .id();
        // A mixed-substance node hands the volume role to each substance's collider, which then
        // names its tank itself.
        let collider = world
            .spawn((Transform::IDENTITY, ChildOf(plate), VolumeOf(root)))
            .id();
        let hull = world
            .spawn((Name::new("Hull"), Transform::IDENTITY, ChildOf(root)))
            .id();
        let parts: HashMap<String, Entity> =
            [("Turret", turret), ("Turret_Front", plate), ("Hull", hull)]
                .into_iter()
                .map(|(name, entity)| (name.to_owned(), entity))
                .collect();
        world.entity_mut(root).insert(SimParts(parts));
        Fixture {
            root,
            turret,
            plate,
            collider,
        }
    }

    /// The plate's world pose, composed by hand from the fixture's transforms.
    pub(crate) fn plate_in_world(world: &World, fixture: &Fixture) -> Transform {
        let root =
            Transform::from_translation(ROOT_AT).with_rotation(Quat::from_rotation_y(ROOT_YAW));
        let turret = *world.get::<Transform>(fixture.turret).unwrap();
        let plate = *world.get::<Transform>(fixture.plate).unwrap();
        root.mul_transform(turret).mul_transform(plate)
    }

    pub(crate) fn strike(struck: Entity, at: Vec3, verdict: Verdict) -> ShellStrike {
        ShellStrike {
            feedback: crate::substances::SurfaceFeedback::fixture(),
            shot: ShotId {
                combatant: CombatantId(3),
                weapon: 1,
                fire_tick: 900,
            },
            sequence: 0,
            struck,
            at,
            normal: Vec3::new(0.2, 0.1, -1.0).normalize(),
            incoming: Vec3::new(0.5, -0.2, 0.84).normalize(),
            caliber: 0.088,
            contact_caliber: 0.088,
            verdict,
        }
    }

    /// The mark kind is a function of the public verdict at the face, and of nothing else. Every
    /// observer's view draws the kind from the record, and the record must agree with the outcome
    /// the impact confirm told them (ADR-0039): a mutant that marks a stop as a hole, a bounce as
    /// anything but a scratch, or invents a mark for a strike that resolved no crossing, breaks it.
    #[test]
    fn the_verdict_picks_the_mark() {
        assert_eq!(MarkKind::of(Verdict::Ricochet), Some(MarkKind::Scratch));
        assert_eq!(MarkKind::of(Verdict::Stopped), Some(MarkKind::Dent));
        assert_eq!(MarkKind::of(Verdict::Through), Some(MarkKind::Hole));
        assert_eq!(MarkKind::of(Verdict::Struck), None);
    }

    /// A mark stretches by 1 / cos θ along the shot, and never past [`max_stretch`] — including a
    /// travel parallel to the face, where 1 / cos θ has no value. The view sizes every mark by it;
    /// a mutant that drops the clamp smears a grazing mark metres down the plate (or divides by
    /// zero), one that inverts it shrinks the oblique marks.
    #[test]
    fn the_stretch_follows_obliquity_up_to_its_clamp() {
        let normal = Vec3::Y;
        let at = |degrees: f32| {
            let theta = degrees.to_radians();
            stretch(Vec3::new(theta.sin(), -theta.cos(), 0.0), normal)
        };
        assert!((at(0.0) - 1.0).abs() < 1e-6, "square on: round");
        assert!(
            (at(60.0) - 2.0_f32.min(max_stretch())).abs() < 1e-4,
            "{}",
            at(60.0)
        );
        assert!((at(75.0) - (1.0 / 75f32.to_radians().cos()).min(max_stretch())).abs() < 1e-4);
        assert_eq!(
            stretch(Vec3::X, normal),
            max_stretch(),
            "a travel along the face"
        );
        let mut last = 0.0;
        for degrees in 0..=90 {
            let value = at(degrees as f32);
            assert!(
                value >= last && value <= max_stretch(),
                "{degrees}°: {value}"
            );
            last = value;
        }
    }

    /// Past [`MARK_CAP`], equal-footprint marks evict the oldest, and serials keep counting, so a
    /// mark keeps its serial for as long as it is held. The view keys its decals on that serial:
    /// a mutant that drops the newest, or that renumbers the held marks from zero, leaves a client
    /// drawing marks the server no longer holds.
    #[test]
    fn the_cap_drops_the_oldest() {
        let mut record = HullMarks::default();
        let extra = 7;
        for n in 0..(MARK_CAP + extra) as u16 {
            record.record(numbered(n));
        }
        assert_eq!(record.len(), MARK_CAP);
        assert_eq!(record.recorded(), (MARK_CAP + extra) as u32);
        let held: Vec<(u32, HullMark)> = record.iter().map(|(s, m)| (s, *m)).collect();
        assert_eq!(held.first(), Some(&(extra as u32, numbered(extra as u16))));
        let last = (MARK_CAP + extra - 1) as u16;
        assert_eq!(held.last(), Some(&(u32::from(last), numbered(last))));
        assert!(
            held.iter()
                .all(|&(serial, mark)| mark == numbered(serial as u16))
        );
    }

    /// The game's damage-model view keeps larger evidence when sustained smaller strikes
    /// fill the budget, while still showing the belt's own marks. Fixture data: two sizes.
    #[test]
    fn a_belt_keeps_every_larger_mark_and_some_of_its_own() {
        let mut record = HullMarks::default();
        let larger = MARK_CAP / 2;
        for n in 0..larger {
            record.record(numbered(n as u16));
        }
        for n in larger..MARK_CAP * 5 {
            let mut mark = numbered(n as u16);
            mark.caliber = MarkCaliber::try_from(0.0079).unwrap();
            record.record(mark);
        }
        assert_eq!(record.len(), MARK_CAP);
        for n in 0..larger {
            assert!(
                record
                    .iter()
                    .any(|(serial, mark)| serial == n as u32 && *mark == numbered(n as u16))
            );
        }
        assert!(record.iter().any(|(_, mark)| mark.caliber() == 0.0079));
    }

    /// A strike is recorded where it struck in the struck PART's frame, whatever the root's and
    /// the turret's poses: the mark then rides the part it was made on. Read through the plate's
    /// collider child, as the walk names a mixed-substance volume. A mutant that records the world
    /// point, or the root's frame, leaves turret marks behind when the turret turns.
    #[test]
    fn a_strike_is_recorded_in_the_struck_parts_frame() {
        let mut world = World::new();
        let fixture = fixture_in(&mut world);
        let plate_world = plate_in_world(&world, &fixture);
        let local = Vec3::new(0.12, -0.05, -0.08);
        let mut hit = strike(
            fixture.collider,
            plate_world.transform_point(local),
            Verdict::Through,
        );
        hit.feedback = crate::substances::SurfaceFeedback {
            exposed: [90, 40, 20],
            metallic: 0,
            sparks: false,
            ..hit.feedback
        };
        let expected_feedback = hit.feedback;
        let root = fixture.root;

        let built = world
            .run_system_once(move |builder: MarkBuilder| builder.mark(&hit))
            .unwrap();
        let (marked, mark) = built.expect("a full-calibre strike through a part leaves a mark");
        assert_eq!(marked, root);
        assert_eq!(
            mark.feedback, expected_feedback,
            "the recorded mark preserves its struck substance"
        );
        assert_eq!(mark.kind, MarkKind::Hole);
        let parts = world.get::<SimParts>(root).unwrap();
        assert_eq!(Some(mark.part), part_index(parts, "Turret_Front"));
        assert!(
            mark.at().distance(local) < 0.001,
            "{:?} vs {local:?}",
            mark.at()
        );
        let to_plate = plate_world.rotation.inverse();
        assert!(mark.normal().angle_between(to_plate * hit.normal) < 0.01);
        assert!(mark.along().angle_between(to_plate * hit.incoming) < 0.01);
        assert_eq!(mark.caliber(), 0.088);
    }

    /// The game's damage-model view: a mark's footprint is what marked the face. A sub-calibre
    /// perforation's hole is its penetrator's; its bounce or stop is the whole flying assembly's.
    /// FIXTURE DATA: an 88 mm kept carrier with a 35.7 mm core. A mutant that
    /// sizes the mark off the flying body draws a full-calibre hole no penetrator cut.
    #[test]
    fn a_sub_calibre_marks_its_hole_with_the_penetrator() {
        const FLYING: f32 = 0.088;
        const CORE: f32 = 0.0357;
        for (verdict, footprint) in [
            (Verdict::Through, CORE),
            (Verdict::Stopped, FLYING),
            (Verdict::Ricochet, FLYING),
        ] {
            let mut world = World::new();
            let fixture = fixture_in(&mut world);
            let plate_world = plate_in_world(&world, &fixture);
            let core = crate::ballistics::RoundBody {
                mass: 1.93,
                caliber: CORE,
                nose_factor: 7.0 / 24.0,
            };
            let flying = crate::ballistics::RoundBody {
                mass: 7.3,
                caliber: FLYING,
                ..core
            };
            let mut hit =
                crate::ballistics::march_tests::first_contact_strike(flying, core, verdict);
            assert_eq!(
                hit.caliber, FLYING,
                "the event remembers the flying body at contact"
            );
            hit.struck = fixture.collider;
            hit.at = plate_world.transform_point(Vec3::ZERO);
            let (_, mark) = world
                .run_system_once(move |builder: MarkBuilder| builder.mark(&hit))
                .unwrap()
                .expect("a main-gun strike on a part leaves a mark");
            assert_eq!(mark.caliber(), footprint, "{verdict:?}");
        }
    }

    /// The game's damage-model view uses one storage policy for every calibre. A small strike
    /// leaves its actual footprint in the bounded record.
    #[test]
    fn every_positive_calibre_can_leave_a_surface_mark() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let fixture = fixture_in(app.world_mut());
        let at = plate_in_world(app.world(), &fixture).transform_point(Vec3::ZERO);
        // Fixture data: three positive sizes, each preserved without a threshold.
        for caliber in [0.0079, 0.019, 0.020] {
            for verdict in [Verdict::Ricochet, Verdict::Stopped, Verdict::Through] {
                let mut hit = strike(fixture.collider, at, verdict);
                hit.caliber = caliber;
                hit.contact_caliber = caliber;
                app.world_mut().trigger(hit);
            }
        }
        let record = app.world().get::<HullMarks>(fixture.root).unwrap();
        let marks: Vec<_> = record
            .iter()
            .map(|(_, mark)| (mark.kind, mark.caliber()))
            .collect();
        assert_eq!(marks.len(), 9);
        for (actual, caliber) in marks.as_chunks::<3>().0.iter().zip([0.0079, 0.019, 0.020]) {
            assert_eq!(
                actual,
                &[
                    (MarkKind::Scratch, caliber),
                    (MarkKind::Dent, caliber),
                    (MarkKind::Hole, caliber)
                ]
            );
        }
    }
    /// The game's damage-model record refuses outside input with no direction; the view never
    /// invents an upward face. Each packed direction's deserializer owns this boundary.
    #[test]
    fn packed_mark_directions_refuse_zero_at_the_input_edge() {
        for field in ["along", "normal"] {
            let mut value = serde_json::to_value(numbered(0)).unwrap();
            value[field] = serde_json::json!([0, 0, 0]);
            assert!(
                serde_json::from_value::<HullMark>(value).is_err(),
                "{field} must refuse zero"
            );
        }
    }
    /// The game's damage-model wire keeps a checked direction in the same three snorm8 bytes.
    /// Rejecting zero must not alter a valid mark's representation on the network.
    #[test]
    fn checked_directions_keep_the_packed_wire_representation() {
        for packed in [[0, 0, -127], [-128, 127, 1]] {
            let direction = PackedDirection::try_from(packed).unwrap();
            let encoded = postcard::to_allocvec(&direction).unwrap();
            assert_eq!(encoded, postcard::to_allocvec(&packed).unwrap());
            assert_eq!(encoded.len(), 3);
        }
    }
}
