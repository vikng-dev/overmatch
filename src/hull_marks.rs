//! Hull marks, the visual layer of the game's damage model: the scratch a ricochet leaves on a
//! vehicle, the dent where its armour stopped a round, the hole where a round went in. A mark is
//! drawn on the hull and read by nobody else; the simulation never reads one (ADR-0014).
//!
//! # The record
//!
//! One [`HullMarks`] per vehicle root: server-owned, public to every client (ADR-0039: a mark on a
//! hull is seen by anyone with eyes on it), born empty in the root's spawn bundle and appended
//! through replicon's diff path, so a client that is caught up receives each new mark and a late
//! joiner the whole set at once. It holds the newest [`MARK_CAP`] marks; recording one more drops
//! the oldest. It lasts the root's life.
//!
//! A [`HullMark`] is packed plain data, built from the authority's [`ShellStrike`]: its kind
//! ([`MarkKind::of`] the strike's verdict), the struck part (an index into the root's
//! [`SimParts`] names, sorted), the impact segment in that part's frame (the round came along
//! `along` and met the face at `at`), the face normal in the same frame, the round's calibre and a
//! roll seed hashed from the shot. Marks ride the part they were made on, so a turret mark turns
//! with the turret. Rounds below [`MIN_MARK_CALIBER_MM`] leave no mark.
//!
//! # Who mounts what
//!
//! The dedicated server records through replicon's diff path (`net::hull_marks`, the wire half);
//! the single-player composition, where the local sim is the authority and nothing replicates,
//! mounts [`offline_plugin`], which writes the component directly. Both build marks with the one
//! [`MarkBuilder`]. The view — decals, textures, the client's diff — is `vfx::hull_marks`, mounted
//! by every windowed composition.

#[cfg(feature = "dev_tools")]
pub(crate) mod demo;

use std::collections::VecDeque;

use avian3d::prelude::{Position, Rotation};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ballistics::{ShellStrike, Verdict};
use crate::damage::VolumeOf;
use crate::tank::{SimParts, rig_world_pose};

/// The marks one vehicle keeps: the newest this many. ASSUMED: the owner's "about 50".
pub(crate) const MARK_CAP: usize = 50;

/// The smallest calibre that leaves a mark, whole millimetres as [`HullMark::caliber_mm`] holds
/// it. Machine-gun rounds, below it, leave none, because of [`MARK_CAP`]: one belt on a hull
/// would turn the whole record over in about 4 s (DERIVED: the shipped machine gun's 750 rpm
/// cyclic is 12.5 rounds a second, and 50 / 12.5 = 4), dropping every main-gun mark the vehicle
/// holds. Issue 144 brings them back under a cap of their own. The value matches the tracer
/// display's cutoff (`ballistics::TRACER_MAX_CALIBER`), but the cap sets this one: neither moves
/// with the other.
pub(crate) const MIN_MARK_CALIBER_MM: u8 = 20;

/// The longest a mark stretches along the shot, as a multiple of its round footprint. A cylinder
/// of radius r meeting a plate at obliquity θ cuts an ellipse r × r / cos θ (DERIVED: 2.0× at
/// 60°, 3.9× at 75°); the clamp keeps a near-grazing strike from smearing a mark down the plate.
pub(crate) const MAX_STRETCH: f32 = 4.0;

/// Which mark a strike leaves.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MarkKind {
    /// A gouge along the bounce.
    Scratch,
    /// A bowl with crazed paint, no core.
    Dent,
    /// A dark core, a lip and a bare-steel ring.
    Hole,
}

impl MarkKind {
    /// Every kind, in declaration order.
    pub(crate) const ALL: [Self; 3] = [Self::Scratch, Self::Dent, Self::Hole];

    /// The mark a strike leaves, from its public verdict at the face alone, so every observer's
    /// hull agrees with the outcome every observer was told (`ImpactConfirm`).
    ///
    /// A round that came to rest inside the tank after going through the face (lodged) has the
    /// face verdict `Through`, and leaves the same hole as one that left the tank. `Struck` is a
    /// stop with no crossing resolved — every terrain strike, and an armour contact whose walk
    /// failed closed — and claims nothing about the face, so it leaves nothing.
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
/// [`MAX_STRETCH`] — which also answers a travel parallel to the face, where 1 / cos θ has no
/// value.
pub(crate) fn stretch(along: Vec3, normal: Vec3) -> f32 {
    let cos = along.dot(normal).abs();
    if cos * MAX_STRETCH <= 1.0 {
        MAX_STRETCH
    } else {
        1.0 / cos
    }
}

/// The shot's direction across the face: `along` with its component on `normal` removed. A
/// strike square to the face has none, and its mark is round; any direction on the face is
/// returned then.
pub(crate) fn in_plane(along: Vec3, normal: Vec3) -> Vec3 {
    (along - normal * along.dot(normal))
        .try_normalize()
        .unwrap_or_else(|| normal.any_orthonormal_vector())
}

/// One mark, packed for the wire. Positions are in the struck part's frame, so the mark rides the
/// part; the view re-derives everything else (size, stretch, roll, material) from these fields.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HullMark {
    pub(crate) kind: MarkKind,
    /// The struck part: its index in the root's [`SimParts`] names, sorted. Both ends build the
    /// part table from the same tank data, so they number it the same.
    pub(crate) part: u16,
    /// Where the round met the face, part frame, millimetres.
    pub(crate) at: [i16; 3],
    /// The round's direction of travel as it met the face, part frame, unit vector in snorm8.
    pub(crate) along: [i8; 3],
    /// The face's outward normal, part frame, unit vector in snorm8.
    pub(crate) normal: [i8; 3],
    /// The round's calibre, whole millimetres (1–255).
    pub(crate) caliber_mm: u8,
    /// Hashed from the shot and the strike's ordinal in it; the view takes the mark's roll and
    /// texture variant from it.
    pub(crate) seed: u16,
}

impl HullMark {
    /// Pack a strike already expressed in its part's frame. `None` when the point does not fit
    /// the wire's ±32.767 m, or a direction has no length.
    pub(crate) fn pack(
        kind: MarkKind,
        part: u16,
        at: Vec3,
        along: Vec3,
        normal: Vec3,
        caliber: f32,
        seed: u16,
    ) -> Option<Self> {
        let mm = |metres: f32| {
            let mm = (metres * 1000.0).round();
            (mm.is_finite() && mm.abs() <= i16::MAX as f32).then_some(mm as i16)
        };
        let snorm = |direction: Vec3| {
            let unit = direction.try_normalize()?;
            Some(unit.to_array().map(|c| (c * 127.0).round() as i8))
        };
        Some(Self {
            kind,
            part,
            at: [mm(at.x)?, mm(at.y)?, mm(at.z)?],
            along: snorm(along)?,
            normal: snorm(normal)?,
            caliber_mm: caliber_mm(caliber),
            seed,
        })
    }

    /// Where the round met the face, part frame, metres.
    pub(crate) fn at(&self) -> Vec3 {
        Vec3::from_array(self.at.map(f32::from)) * 0.001
    }

    /// The round's direction of travel, part frame, unit.
    pub(crate) fn along(&self) -> Vec3 {
        unsnorm(self.along)
    }

    /// The face's outward normal, part frame, unit.
    pub(crate) fn normal(&self) -> Vec3 {
        unsnorm(self.normal)
    }

    /// The round's calibre, metres.
    pub(crate) fn caliber(&self) -> f32 {
        f32::from(self.caliber_mm) * 0.001
    }
}

/// A calibre in metres as [`HullMark::caliber_mm`] holds it: whole millimetres, 1–255.
fn caliber_mm(caliber: f32) -> u8 {
    (caliber * 1000.0).round().clamp(1.0, 255.0) as u8
}

/// A packed unit vector back to unit length. `pack` never writes a zero vector; `Y` stands in for
/// one a hand-built mark carries.
fn unsnorm(packed: [i8; 3]) -> Vec3 {
    Vec3::from_array(packed.map(f32::from))
        .try_normalize()
        .unwrap_or(Vec3::Y)
}

/// A vehicle's marks, oldest first: the newest [`MARK_CAP`] of every mark ever recorded on it.
///
/// A mark's serial is its place among every mark the vehicle has ever had, so the view keys its
/// decals by it: the first held mark's serial is `recorded − held`, and a mark dropped off the
/// front is one whose serial fell below that. The view orders serials as plain integers, which
/// holds while a vehicle records fewer than 2³² marks.
#[derive(Component, Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct HullMarks {
    marks: VecDeque<HullMark>,
    /// Marks ever recorded on this vehicle, the dropped included.
    recorded: u32,
}

impl HullMarks {
    /// Record one mark, dropping the oldest past [`MARK_CAP`].
    pub(crate) fn record(&mut self, mark: HullMark) {
        self.marks.push_back(mark);
        while self.marks.len() > MARK_CAP {
            self.marks.pop_front();
        }
        self.recorded = self.recorded.wrapping_add(1);
    }

    /// Marks ever recorded on this vehicle, the dropped included.
    pub(crate) fn recorded(&self) -> u32 {
        self.recorded
    }

    /// The serial of the oldest mark held.
    pub(crate) fn first_serial(&self) -> u32 {
        self.recorded.wrapping_sub(self.marks.len() as u32)
    }

    /// Every held mark with its serial, oldest first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, &HullMark)> {
        let first = self.first_serial();
        self.marks
            .iter()
            .enumerate()
            .map(move |(index, mark)| (first.wrapping_add(index as u32), mark))
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
    parents: Query<'w, 's, &'static ChildOf>,
    locals: Query<'w, 's, &'static Transform>,
    names: Query<'w, 's, &'static Name>,
    roots:
        Query<'w, 's, (&'static SimParts, &'static Position, &'static Rotation), With<HullMarks>>,
}

impl MarkBuilder<'_, '_> {
    /// The vehicle root a strike marks, and its mark. `None` when the verdict leaves no mark, the
    /// round is below [`MIN_MARK_CALIBER_MM`], the struck volume is not a part of a root that
    /// keeps marks (a tree, a turret thrown clear of its hull), or the point does not pack.
    pub(crate) fn mark(&self, strike: &ShellStrike) -> Option<(Entity, HullMark)> {
        let kind = MarkKind::of(strike.verdict)?;
        if caliber_mm(strike.caliber) < MIN_MARK_CALIBER_MM {
            return None;
        }
        let root = self.owners.get(strike.struck).ok()?.tank();
        let (parts, position, rotation) = self.roots.get(root).ok()?;
        // The struck entity is the volume's node, or one substance's collider under it.
        let mut part = strike.struck;
        let name = loop {
            if let Ok(name) = self.names.get(part)
                && parts.0.get(name.as_str()) == Some(&part)
            {
                break name;
            }
            part = self.parents.get(part).ok()?.parent();
            if part == root {
                return None;
            }
        };
        let index = part_index(parts, name.as_str())?;
        let (origin, turn) = rig_world_pose(
            part,
            root,
            position.0,
            rotation.0,
            &self.parents,
            &self.locals,
        )?;
        let to_part = turn.inverse();
        HullMark::pack(
            kind,
            index,
            to_part * (strike.at - origin),
            to_part * strike.incoming,
            to_part * strike.normal,
            strike.caliber,
            seed(strike),
        )
        .map(|mark| (root, mark))
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

/// The single-player recorder: the local sim is the authority and nothing replicates, so the mark
/// is written to the component directly.
pub(crate) fn offline_plugin(app: &mut App) {
    app.add_observer(record_local);
}

fn record_local(strike: On<ShellStrike>, builder: MarkBuilder, mut records: Query<&mut HullMarks>) {
    if let Some((root, mark)) = builder.mark(&strike)
        && let Ok(mut record) = records.get_mut(root)
    {
        record.record(mark);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{CombatantId, ShotId};

    /// A mark whose fields all differ from its neighbours', told apart by `n`.
    pub(crate) fn numbered(n: u16) -> HullMark {
        HullMark {
            kind: MarkKind::ALL[n as usize % 3],
            part: n % 3,
            at: [n as i16, -(n as i16), 7],
            along: [0, 0, -127],
            normal: [0, 127, 0],
            caliber_mm: 88,
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

    /// A mark stretches by 1 / cos θ along the shot, and never past [`MAX_STRETCH`] — including a
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
        assert!((at(60.0) - 2.0).abs() < 1e-4, "{}", at(60.0));
        assert!((at(75.0) - 1.0 / 75f32.to_radians().cos()).abs() < 1e-4);
        assert!(at(75.0) < MAX_STRETCH);
        assert_eq!(at(80.0), MAX_STRETCH);
        assert_eq!(at(90.0), MAX_STRETCH, "a travel along the face");
        let mut last = 0.0;
        for degrees in 0..=90 {
            let value = at(degrees as f32);
            assert!(value >= last && value <= MAX_STRETCH, "{degrees}°: {value}");
            last = value;
        }
    }

    /// Past [`MARK_CAP`], recording a mark drops the OLDEST, and the serials keep counting, so a
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
        let hit = strike(
            fixture.collider,
            plate_world.transform_point(local),
            Verdict::Through,
        );
        let root = fixture.root;

        let built = world
            .run_system_once(move |builder: MarkBuilder| builder.mark(&hit))
            .unwrap();
        let (marked, mark) = built.expect("a full-calibre strike through a part leaves a mark");
        assert_eq!(marked, root);
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
        assert_eq!(mark.caliber_mm, 88);
    }

    /// A round below [`MIN_MARK_CALIBER_MM`] leaves no mark whatever its verdict, and a 20 mm
    /// round leaves one. The cap relies on it: one machine-gun belt would otherwise turn the whole
    /// [`MARK_CAP`] over in about 4 s and drop every main-gun mark the vehicle holds. A mutant
    /// that drops the threshold lets a belt flood the record; one that raises it past 20 mm takes
    /// an autocannon's marks away.
    #[test]
    fn rounds_below_20_mm_leave_no_mark() {
        let mut app = App::new();
        app.add_plugins(offline_plugin);
        let fixture = fixture_in(app.world_mut());
        let at = plate_in_world(app.world(), &fixture).transform_point(Vec3::ZERO);
        // Fixture data: a rifle-calibre round, one just under the threshold, one at it.
        for caliber in [0.0079, 0.019, 0.020] {
            for verdict in [Verdict::Ricochet, Verdict::Stopped, Verdict::Through] {
                let mut hit = strike(fixture.collider, at, verdict);
                hit.caliber = caliber;
                app.world_mut().trigger(hit);
            }
        }
        let record = app.world().get::<HullMarks>(fixture.root).unwrap();
        let marks: Vec<_> = record
            .iter()
            .map(|(_, mark)| (mark.kind, mark.caliber_mm))
            .collect();
        assert_eq!(
            marks,
            [
                (MarkKind::Scratch, 20),
                (MarkKind::Dent, 20),
                (MarkKind::Hole, 20)
            ],
        );
    }
}
