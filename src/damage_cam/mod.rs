//! The damage cam's record: [`PenetrationReport`], one per (shot, struck tank).
//!
//! The report is plain serde data — no entities, handles or meshes — so any source can fill it (a
//! hand-authored RON clip, or [`record`] reading a shot the sim resolved). It does not ride the
//! wire itself: [`wire`] carries it as indices and quanta, and [`delivery`] hands each recipient
//! its own.
//!
//! The record carries no geometry. It holds the shot's inputs (round, origin, shooter, struck
//! vehicle, every servo's angle), the struck tank's state at the shot (each component's hit points,
//! so a part already lost reads zero), and the authority's outcome against that one tank (path,
//! verdict, how the round's flight ended, dose and fate per component), every volume named. A
//! component's shape is the tank asset's, resolved by name and posed by the record's servo angles
//! ([`shapes`]).
//!
//! Frame: hull-local — origin at the hull root node, the glb's axes (`-Z` forward, `+Y` up, `+X`
//! right). Units: metres for positions, unit vectors for directions, seconds for `t` measured from
//! first contact (the approach is negative), degrees for angles, millimetres for armour, reference
//! millimetres for doses, strengths and counts, kilograms and metres per second for the projectile.
//! `synthetic` marks a report whose numbers were authored, not simulated; [`wire`] never sends one.

pub(crate) mod delivery;
pub mod pen_cam;
pub mod record;
pub(crate) mod recorder;
pub mod shapes;
pub(crate) mod wire;

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The pen cam's verdict on a whole shot against one tank, read from the path the authority
/// resolved: its crossings, its bounces and the HP they cost the tank's components. The first
/// variant whose rule holds is the verdict, in declaration order. A pen cam exists only for a hit on
/// a player's tank.
///
/// Armour is a volume that is neither a component (an hp pool) nor external armour
/// ([`crate::spec::VolumeSpec::External`]): perforating a skirt or a road wheel is not perforating
/// the tank, and crossing a component is a dose, not a perforation.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotVerdict {
    /// The round perforated armour, whatever happened after: it stopped in a far plate, left the
    /// tank, or cost no HP.
    Penetration,
    /// No penetration, and a component lost HP: its dose from the line, the spall cones and a
    /// bounce's shock, rounded once per box per shot, came to at least one point. An exposed
    /// component, or a barrel clipped on the way into armour.
    Damaged,
    /// No penetration, no HP lost, and the round bounced away.
    Ricochet,
    /// Everything else: stopped by armour or external armour, or through external armour alone
    /// and flown on.
    NonPen,
}

/// How the pen cam frames a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenCamFrame {
    /// The hull drawn see-through around the path inside it.
    SeeThroughHull,
    /// The struck plate cut along the path.
    PlateSlice,
    /// The tank from outside; a damaged component is tinted.
    Outside,
}

/// One event of a shot's path, as the verdict reads it. Crossings and bounces are in path order;
/// [`Self::Dosed`]'s place among them is not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStep {
    /// Deflected off a face without entering it.
    Bounce,
    /// Entered the material: `perforated_armour` when the whole round came out of an armour volume,
    /// `rested` when it came to rest inside this crossing.
    Crossing {
        perforated_armour: bool,
        rested: bool,
    },
    /// A component of the struck tank lost HP to the shot: its dose rounded once per box per shot
    /// ([`crate::ballistics::ShotDose`]) came to at least one point and its pool had it. A dose
    /// that rounds to none changed nothing in the game and is not this step.
    Dosed,
    /// A walk that failed closed: an error, never a pen cam.
    Failed,
}

impl ShotVerdict {
    /// The verdict a path earns, or `None` when it opens no pen cam: no event at all (a ground
    /// hit), or any walk that failed closed.
    pub fn of_path(steps: &[PathStep]) -> Option<Self> {
        if steps.is_empty() || steps.contains(&PathStep::Failed) {
            return None;
        }
        let perforated = steps.iter().any(|step| {
            matches!(
                step,
                PathStep::Crossing {
                    perforated_armour: true,
                    ..
                }
            )
        });
        let rested = steps
            .iter()
            .any(|step| matches!(step, PathStep::Crossing { rested: true, .. }));
        Some(if perforated {
            Self::Penetration
        } else if steps.contains(&PathStep::Dosed) {
            Self::Damaged
        } else if !rested && steps.contains(&PathStep::Bounce) {
            Self::Ricochet
        } else {
            Self::NonPen
        })
    }

    /// The frame the pen cam opens on for this verdict.
    pub fn frame(self) -> PenCamFrame {
        match self {
            Self::Penetration => PenCamFrame::SeeThroughHull,
            Self::NonPen => PenCamFrame::PlateSlice,
            Self::Ricochet | Self::Damaged => PenCamFrame::Outside,
        }
    }
}

/// A finished shot against one tank, in causal order.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PenetrationReport {
    pub synthetic: bool,
    /// The struck vehicle's tank file, relative to the asset root; `None` on a hand-authored clip.
    #[serde(default)]
    pub vehicle: Option<String>,
    /// Every servo of the struck tank at the shot, by its node name in the tank file: the sim's own
    /// servo angle (degrees), the turn about the servo's axis from its authored rest rotation,
    /// composed as [`crate::tank::servo_pose`] composes it. A servo the record does not name is
    /// at rest; a hand-authored clip names none.
    pub servo_deg: BTreeMap<String, f32>,
    /// The round fired: a key in the round registry (`assets/ballistics/rounds.ron`). A synthetic
    /// clip may name a round the registry does not hold.
    pub round: String,
    /// Who fired; `None` on a hand-authored clip.
    #[serde(default)]
    pub shooter: Option<Shooter>,
    /// Where the round started: the path's first point.
    pub origin: [f32; 3],
    /// Where the shooter's gun fired it from: the muzzle at fire time, read off the shooter's rig
    /// ([`record::muzzle_of`]). `None` where no shooter's rig fired it.
    #[serde(default)]
    pub muzzle: Option<[f32; 3]>,
    pub projectile: Projectile,
    pub impact: Impact,
    /// Armour crossings in path order: one per armour volume the line put a chord into.
    pub plates: Vec<Plate>,
    /// Bounces in path order.
    #[serde(default)]
    pub ricochets: Vec<Ricochet>,
    /// Timed shell path, approach included; a ricochet's deflection leg continues it.
    pub path: Vec<PathPoint>,
    pub spall: Vec<SpallCone>,
    /// How the round's flight ended.
    pub end: RoundEnd,
    /// What the shot dosed each component with, in [`Self::components`] order; a component it never
    /// reached is absent.
    #[serde(default)]
    pub doses: Vec<BoxDose>,
    pub consequences: Vec<Consequence>,
    /// The struck tank's components (crew and module volumes) as they stood at the shot.
    pub components: Vec<ComponentState>,
    /// The knockout this shot caused: the struck tank died, in the tick of the shot, of a box this
    /// shot's own deposition emptied (the game's recorder attributes it; it is never read back from
    /// HP). `None` otherwise, and on a hand-authored clip or a harness record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killed: Option<crate::damage::KnockoutReason>,
}

/// Who fired the shot.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Shooter {
    /// The player's name.
    pub name: String,
    /// The shooter's tank file, relative to the asset root.
    pub vehicle: String,
}

/// How the round's flight ended, as the authority marched it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum RoundEnd {
    /// It came to rest inside `volume` (armour or a component), at the path's last point.
    Rest { volume: String },
    /// It left the struck tank at `at`, a point of the path: the far end of its last crossing, or
    /// the face of its last bounce. The path runs on past it.
    Left { at: [f32; 3] },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Projectile {
    pub name: String,
    pub caliber_mm: f32,
    pub mass_kg: f32,
    /// Speed at contact.
    pub speed_mps: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Impact {
    pub point: [f32; 3],
    /// Outward surface normal at `point`.
    pub normal: [f32; 3],
    /// Direction of travel at contact.
    pub incoming: [f32; 3],
    /// Angle between `-incoming` and `normal`.
    pub incidence_deg: f32,
    pub verdict: ShotVerdict,
}

/// Which armour a plate is: `Main` is armour ([`ShotVerdict`]), the only class whose perforation
/// is a penetration.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlateClass {
    #[default]
    Main,
    External,
}

/// One armour volume's crossing. Thickness is three numbers: the geometric chord along the path,
/// its normal steel equivalent, and the cost actually charged against the shell's capability.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Plate {
    pub name: String,
    pub t: f32,
    pub entry: [f32; 3],
    /// `None` when the shell did not come out the far side: it rests inside, at the path's end.
    pub exit: Option<[f32; 3]>,
    pub chord_mm: f32,
    pub steel_equiv_mm: f32,
    pub cost_mm: f32,
    pub overmatched: bool,
    #[serde(default)]
    pub class: PlateClass,
    /// Reference-mm per metre of chord: the substance's factor. Zero on a hand-authored clip.
    #[serde(default)]
    pub factor: f32,
}

/// One bounce off a face.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Ricochet {
    pub t: f32,
    pub point: [f32; 3],
    /// Outward surface normal at `point`.
    pub normal: [f32; 3],
    pub incoming: [f32; 3],
    pub outgoing: [f32; 3],
    /// The volume owning the struck face.
    pub volume: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct PathPoint {
    pub t: f32,
    pub point: [f32; 3],
}

/// One spall cone — the field a body exit throws (design doc §5) — and the directions it was
/// marched by.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SpallCone {
    pub t: f32,
    pub origin: [f32; 3],
    /// Unit cone axis.
    pub axis: [f32; 3],
    pub half_angle_deg: f32,
    /// The on-axis strength `S` (reference-mm). Zero on a hand-authored clip.
    #[serde(default)]
    pub strength: f32,
    pub fragments: Vec<Fragment>,
}

/// One direction of a spall cone.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Fragment {
    /// Where the direction's reach ended: inside the volume that exhausted it, or where it left
    /// the struck tank.
    pub end: [f32; 3],
    /// The strength it was thrown with (reference-mm). Zero on a hand-authored clip.
    #[serde(default)]
    pub strength: f32,
    /// Boxes it dosed, in flight order; empty for a direction that dosed nothing.
    pub hits: Vec<FragmentHit>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FragmentHit {
    /// A [`ComponentState::name`].
    pub volume: String,
    /// Dose, reference-mm.
    pub damage: f32,
}

/// One box's dose from the shot (reference-mm): the line's, a bounce's shock included, and the
/// spall cones'; and the HP points it lost, the sum of the two rounded once, the field's at its own
/// HP per reference-mm ([`crate::ballistics::field_units`]), and capped at the HP it had.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BoxDose {
    /// A [`ComponentState::name`].
    pub volume: String,
    pub line: f32,
    pub field: f32,
    /// HP points the shot took from it.
    pub lost: u32,
    /// Where the line first reached it: its entry, or the face it bounced off. `None` when only a
    /// spall cone did.
    #[serde(default)]
    pub entered: Option<[f32; 3]>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Consequence {
    pub t: f32,
    /// A [`ComponentState::name`].
    pub volume: String,
    pub state: ConsequenceState,
    /// The capability this cost the tank, if any.
    pub function: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsequenceState {
    Damaged,
    Lost,
    Cookoff,
}

/// One component of the struck tank (a crew station or module volume) as it stood at the shot, by
/// its node name in the tank asset.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ComponentState {
    pub name: String,
    pub role: Role,
    /// Its HP pool, in whole HP points: the points that disable it.
    pub count: u32,
    /// Its HP points at the shot; zero for a part already lost.
    pub hp: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Crew,
    Module,
    Ammo,
    Engine,
}

impl PenetrationReport {
    pub fn from_ron(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(text)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn component(&self, name: &str) -> Option<&ComponentState> {
        self.components
            .iter()
            .find(|component| component.name == name)
    }

    /// Every outcome the record states without the reference that places it: a line dose with no
    /// [`BoxDose::entered`], a field dose no spall cone's fragment names, a name no component of
    /// the record holds. Empty for every record the recorder writes; a synthetic clip may lack the
    /// places, and its picture reaches such a component at contact.
    pub fn unresolved(&self) -> Vec<String> {
        let mut out = Vec::new();
        for dose in &self.doses {
            if self.component(&dose.volume).is_none() {
                out.push(format!("a dose names no component: {}", dose.volume));
            }
            if dose.line > 0.0 && dose.entered.is_none() {
                out.push(format!("{}: a line dose with no entry", dose.volume));
            }
            let thrown = self.spall.iter().flat_map(|cone| &cone.fragments).any(|f| {
                f.hits
                    .iter()
                    .any(|hit| hit.volume == dose.volume && hit.damage > 0.0)
            });
            if dose.field > 0.0 && !thrown {
                out.push(format!("{}: a field dose no fragment names", dose.volume));
            }
        }
        out
    }

    /// Whether the round came to rest in the struck tank.
    pub fn rested(&self) -> bool {
        matches!(self.end, RoundEnd::Rest { .. })
    }

    /// The record's path as the verdict reads it: its plates and bounces in time order, each
    /// plate a crossing of its own, then one [`PathStep::Dosed`] when any box lost HP. A plate
    /// without an exit is where the round rested.
    pub fn steps(&self) -> Vec<PathStep> {
        let mut steps: Vec<(f32, PathStep)> = self
            .plates
            .iter()
            .map(|plate| {
                let step = PathStep::Crossing {
                    perforated_armour: plate.class == PlateClass::Main && plate.exit.is_some(),
                    rested: plate.exit.is_none(),
                };
                (plate.t, step)
            })
            .chain(
                self.ricochets
                    .iter()
                    .map(|bounce| (bounce.t, PathStep::Bounce)),
            )
            .collect();
        steps.sort_by(|a, b| a.0.total_cmp(&b.0));
        let dosed = self.doses.iter().any(|dose| dose.lost > 0);
        steps
            .into_iter()
            .map(|(_, step)| step)
            .chain(dosed.then_some(PathStep::Dosed))
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const ARMOUR_THROUGH: PathStep = PathStep::Crossing {
        perforated_armour: true,
        rested: false,
    };
    const ARMOUR_RESTED: PathStep = PathStep::Crossing {
        perforated_armour: false,
        rested: true,
    };
    /// Through external armour or a component only: nothing perforated counts, nothing rested.
    const NOTHING_PERFORATED: PathStep = PathStep::Crossing {
        perforated_armour: false,
        rested: false,
    };

    /// The verdict reads the whole path, first rule wins: perforating armour anywhere is a
    /// penetration, whatever follows and whether or not any HP was lost; short of that any HP lost
    /// is damage, wherever it sits among the path's events; short of that a round that bounces
    /// away is a ricochet and any other a non-pen. No event, or any failed walk, opens no pen cam.
    #[test]
    fn the_shot_verdict_reads_the_whole_path() {
        use PathStep::{Bounce, Dosed, Failed};
        let cases: [(&[PathStep], Option<ShotVerdict>); 21] = [
            (&[], None),
            (&[Failed], None),
            (&[ARMOUR_THROUGH, Failed], None),
            (&[ARMOUR_THROUGH, Dosed, Failed], None),
            (&[Bounce], Some(ShotVerdict::Ricochet)),
            (&[ARMOUR_RESTED], Some(ShotVerdict::NonPen)),
            (&[ARMOUR_THROUGH], Some(ShotVerdict::Penetration)),
            (
                &[ARMOUR_THROUGH, ARMOUR_RESTED],
                Some(ShotVerdict::Penetration),
            ),
            (
                &[NOTHING_PERFORATED, ARMOUR_RESTED],
                Some(ShotVerdict::NonPen),
            ),
            (&[NOTHING_PERFORATED, Bounce], Some(ShotVerdict::Ricochet)),
            (&[Bounce, ARMOUR_RESTED], Some(ShotVerdict::NonPen)),
            (&[Bounce, ARMOUR_THROUGH], Some(ShotVerdict::Penetration)),
            (&[NOTHING_PERFORATED], Some(ShotVerdict::NonPen)),
            // A dose never outranks a penetration.
            (&[ARMOUR_THROUGH, Dosed], Some(ShotVerdict::Penetration)),
            (
                &[Dosed, Bounce, ARMOUR_THROUGH],
                Some(ShotVerdict::Penetration),
            ),
            // An exposed component dosed with no armour crossed.
            (&[NOTHING_PERFORATED, Dosed], Some(ShotVerdict::Damaged)),
            (&[Dosed], Some(ShotVerdict::Damaged)),
            // A bounce that shocks a component is damage, not a ricochet.
            (&[Bounce, Dosed], Some(ShotVerdict::Damaged)),
            // A barrel clipped, then the round stopped in armour.
            (
                &[NOTHING_PERFORATED, Dosed, ARMOUR_RESTED],
                Some(ShotVerdict::Damaged),
            ),
            (&[ARMOUR_RESTED, Dosed], Some(ShotVerdict::Damaged)),
            // A dose from external armour's spall cone, the round flying on.
            (
                &[NOTHING_PERFORATED, Dosed, Bounce],
                Some(ShotVerdict::Damaged),
            ),
        ];
        for (steps, verdict) in cases {
            assert_eq!(ShotVerdict::of_path(steps), verdict, "{steps:?}");
        }
    }

    /// A record's path is dosed only where a box lost HP: a dose that rounded to no point leaves
    /// the box's record but no [`PathStep::Dosed`]. Fixture doses (reference-mm).
    #[test]
    fn a_record_is_dosed_only_where_hp_was_lost() {
        let mut report = delivery::tests::report("s1_through_crew");
        report.plates.clear();
        report.ricochets.clear();
        let dose = |lost| BoxDose {
            volume: "fixture box".to_owned(),
            line: 0.04,
            field: 0.0,
            lost,
            entered: None,
        };
        report.doses = vec![dose(0)];
        assert_eq!(report.steps(), Vec::new());
        assert_eq!(ShotVerdict::of_path(&report.steps()), None);
        report.doses.push(dose(1));
        assert_eq!(report.steps(), vec![PathStep::Dosed]);
        assert_eq!(
            ShotVerdict::of_path(&report.steps()),
            Some(ShotVerdict::Damaged)
        );
    }

    /// A recorded report survives RON serialization field for field: the recorder writes its files
    /// with this serializer, and [`PenetrationReport::load`] reads them back.
    #[test]
    fn report_round_trips_through_ron() {
        let report = delivery::tests::report("s1_through_crew");
        let text = ron::ser::to_string_pretty(&report, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(PenetrationReport::from_ron(&text).unwrap(), report);
    }

    /// Every name a report's hits and consequences cite resolves to one of its internals, every
    /// timed list is in time order, and the path ends in or past the last plate: the sim marches a
    /// perforating shell on until it embeds or leaves the vehicle, so an embed holds the path's
    /// end and a perforated last plate has it beyond the exit. A deflection leg that ends the path
    /// (a ricochet's, or a bounce after the last plate) is unconstrained.
    pub(crate) fn assert_self_consistent(file: &str, report: &PenetrationReport) {
        let cited = report
            .spall
            .iter()
            .flat_map(|burst| &burst.fragments)
            .flat_map(|fragment| &fragment.hits)
            .map(|hit| hit.volume.as_str())
            .chain(report.doses.iter().map(|d| d.volume.as_str()))
            .chain(report.consequences.iter().map(|c| c.volume.as_str()));
        for name in cited {
            assert!(
                report.component(name).is_some(),
                "{file}: unknown volume {name}"
            );
        }
        match &report.end {
            RoundEnd::Rest { volume } => {
                let in_plate = report
                    .plates
                    .last()
                    .is_some_and(|plate| plate.exit.is_none() && plate.name == *volume);
                assert!(
                    in_plate || report.component(volume).is_some(),
                    "{file}: rested in {volume}, neither its last plate without an exit nor a \
                     component"
                );
            }
            RoundEnd::Left { at } => {
                // A hand-authored ricochet lists the face it glanced off as a plate it never left.
                assert!(
                    report.impact.verdict == ShotVerdict::Ricochet
                        || report.plates.iter().all(|plate| plate.exit.is_some()),
                    "{file}: left the tank, yet a plate holds it"
                );
                assert!(
                    report.path.iter().any(|p| distance(p.point, *at) < 1.0e-5),
                    "{file}: where it left, {at:?}, is not a point of its path"
                );
            }
        }
        assert_eq!(
            report.origin, report.path[0].point,
            "{file}: the path starts at the origin"
        );
        let sorted = |ts: Vec<f32>| ts.windows(2).all(|w| w[0] <= w[1]);
        assert!(sorted(report.path.iter().map(|p| p.t).collect()), "{file}");
        assert!(
            sorted(report.consequences.iter().map(|c| c.t).collect()),
            "{file}"
        );
        assert!(report.path.len() >= 2, "{file}: path needs a segment");
        let last = report.path.last().unwrap();
        for plate in &report.plates {
            assert!(
                last.t >= plate.t,
                "{file}: the path ends before {}",
                plate.name
            );
        }
        if let Some(index) = report.plates.iter().position(|p| p.exit.is_none()) {
            assert_eq!(
                index + 1,
                report.plates.len(),
                "{file}: a plate without an exit is the last plate"
            );
        }
        let bounced_last = report.impact.verdict == ShotVerdict::Ricochet
            || report
                .ricochets
                .last()
                .is_some_and(|bounce| report.plates.last().is_none_or(|plate| bounce.t >= plate.t));
        if !bounced_last && let Some(plate) = report.plates.last() {
            match plate.exit {
                None => {
                    let gap = distance(last.point, plate.entry);
                    assert!(
                        gap <= 0.25,
                        "{file}: the path ends {gap:.2} m from {}",
                        plate.name
                    );
                }
                Some(exit) => {
                    let along: f32 = (0..3)
                        .map(|i| (last.point[i] - exit[i]) * (exit[i] - plate.entry[i]))
                        .sum();
                    assert!(
                        along >= 0.0,
                        "{file}: the path ends behind the exit of {}",
                        plate.name
                    );
                }
            }
        }
    }

    fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}
