//! The damage cam's record: [`PenetrationReport`], one per (shot, struck tank), and its player.
//!
//! The report is plain serde data — no entities, handles or meshes — so any source can fill it (a
//! hand-authored RON clip, or [`record`] reading a shot the sim resolved) and [`player`] consumes
//! it unchanged. It is not on the wire.
//!
//! Frame: hull-local — origin at the hull root node, the glb's axes (`-Z` forward, `+Y` up, `+X`
//! right). Units: metres for positions, unit vectors for directions, seconds for `t` measured from
//! first contact (the approach is negative), degrees for angles, millimetres for armour, reference
//! millimetres for doses, strengths and counts, kilograms and metres per second for the projectile.
//! `synthetic` marks a report whose numbers were authored, not simulated; the player prints it on
//! screen.

pub mod player;
pub mod record;

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The pen cam's verdict on a whole shot against one tank, read from the path the authority
/// resolved, never from damage. A pen cam exists only for a hit on a player's tank.
///
/// External armour ([`crate::spec::VolumeSpec::External`]) does not count: perforating a skirt
/// or a road wheel is not perforating the tank.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotVerdict {
    /// The round perforated main armour, whatever happened after: it stopped in a far plate or
    /// left the tank.
    Penetration,
    /// It bounced away without perforating main armour.
    Ricochet,
    /// It came to rest without perforating main armour. A round that crossed only external armour
    /// or exterior boxes and flew on is one too: it neither bounced nor got through the tank.
    NonPen,
}

/// One armour event of a shot's path, as the verdict reads it, in path order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStep {
    /// Deflected off a face without entering it.
    Bounce,
    /// Entered the material: `perforated_main` when the whole round came out of a main armour
    /// volume, `rested` when it came to rest inside this crossing.
    Crossing { perforated_main: bool, rested: bool },
    /// A walk that failed closed: an error, never a pen cam.
    Failed,
}

impl ShotVerdict {
    /// The verdict a path earns, or `None` when it opens no pen cam: no armour event at all (a
    /// ground hit), or any walk that failed closed.
    pub fn of_path(steps: &[PathStep]) -> Option<Self> {
        if steps.is_empty() || steps.contains(&PathStep::Failed) {
            return None;
        }
        let perforated = steps.iter().any(|step| {
            matches!(
                step,
                PathStep::Crossing {
                    perforated_main: true,
                    ..
                }
            )
        });
        let rested = matches!(steps.last(), Some(PathStep::Crossing { rested: true, .. }));
        Some(if perforated {
            Self::Penetration
        } else if !rested && steps.contains(&PathStep::Bounce) {
            Self::Ricochet
        } else {
            Self::NonPen
        })
    }
}

/// A finished shot against one tank, in causal order.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PenetrationReport {
    pub synthetic: bool,
    /// The struck vehicle's tank file, relative to the asset root; `None` on a hand-authored clip.
    #[serde(default)]
    pub vehicle: Option<String>,
    /// The struck turret's traverse from its spawn pose at the shot (degrees, about `+Y`,
    /// positive from `-Z` toward `-X`).
    #[serde(default)]
    pub turret_yaw_deg: f32,
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
    /// What the shot dosed each box with, in [`Self::internals`] order; a box it never reached is
    /// absent.
    #[serde(default)]
    pub doses: Vec<BoxDose>,
    pub consequences: Vec<Consequence>,
    /// The struck tank's boxes (crew and module volumes) at the time of the shot.
    pub internals: Vec<Internal>,
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

/// Which armour a plate is: the shot verdict counts perforating main armour only.
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
    /// An [`Internal::name`].
    pub volume: String,
    /// Dose, reference-mm.
    pub damage: f32,
}

/// One box's dose from the shot (reference-mm): the line's, a bounce's shock included, and the
/// spall cones'.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BoxDose {
    /// An [`Internal::name`].
    pub volume: String,
    pub line: f32,
    pub field: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Consequence {
    pub t: f32,
    /// An [`Internal::name`].
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

/// A box standing in for a crew station or module volume: its bounds in its own frame, placed in
/// the record's frame.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Internal {
    pub name: String,
    pub role: Role,
    pub center: [f32; 3],
    pub half_extents: [f32; 3],
    /// The box's orientation (`x, y, z, w` quaternion); identity is axis-aligned.
    #[serde(default = "identity")]
    pub rotation: [f32; 4],
    /// The dose that disables it (reference-mm): its HP pool. Zero on a hand-authored clip.
    #[serde(default)]
    pub count: f32,
}

fn identity() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
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

    pub fn internal(&self, name: &str) -> Option<&Internal> {
        self.internals.iter().find(|internal| internal.name == name)
    }

    /// The record's path as the verdict reads it: its plates and bounces in time order, each
    /// plate a crossing of its own. A plate without an exit is where the round rested.
    pub fn steps(&self) -> Vec<PathStep> {
        let mut steps: Vec<(f32, PathStep)> = self
            .plates
            .iter()
            .map(|plate| {
                let step = PathStep::Crossing {
                    perforated_main: plate.class == PlateClass::Main && plate.exit.is_some(),
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
        steps.into_iter().map(|(_, step)| step).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const MAIN_THROUGH: PathStep = PathStep::Crossing {
        perforated_main: true,
        rested: false,
    };
    const MAIN_RESTED: PathStep = PathStep::Crossing {
        perforated_main: false,
        rested: true,
    };
    const EXTERNAL_THROUGH: PathStep = PathStep::Crossing {
        perforated_main: false,
        rested: false,
    };

    /// The verdict reads the whole path: perforating main armour anywhere is a penetration,
    /// whatever follows; short of that a round that comes to rest is a non-pen and one that
    /// bounces away is a ricochet. No armour event, or any failed walk, opens no pen cam.
    #[test]
    fn the_shot_verdict_reads_the_whole_path() {
        use PathStep::{Bounce, Failed};
        let cases: [(&[PathStep], Option<ShotVerdict>); 12] = [
            (&[], None),
            (&[Failed], None),
            (&[MAIN_THROUGH, Failed], None),
            (&[Bounce], Some(ShotVerdict::Ricochet)),
            (&[MAIN_RESTED], Some(ShotVerdict::NonPen)),
            (&[MAIN_THROUGH], Some(ShotVerdict::Penetration)),
            (&[MAIN_THROUGH, MAIN_RESTED], Some(ShotVerdict::Penetration)),
            (&[EXTERNAL_THROUGH, MAIN_RESTED], Some(ShotVerdict::NonPen)),
            (&[EXTERNAL_THROUGH, Bounce], Some(ShotVerdict::Ricochet)),
            (&[Bounce, MAIN_RESTED], Some(ShotVerdict::NonPen)),
            (&[Bounce, MAIN_THROUGH], Some(ShotVerdict::Penetration)),
            (&[EXTERNAL_THROUGH], Some(ShotVerdict::NonPen)),
        ];
        for (steps, verdict) in cases {
            assert_eq!(ShotVerdict::of_path(steps), verdict, "{steps:?}");
        }
    }

    fn scenario_dir() -> std::path::PathBuf {
        crate::assets::asset_root().join(player::SCENARIO_DIR)
    }

    #[test]
    fn report_round_trips_through_ron() {
        let report = PenetrationReport::load(&scenario_dir().join("through.ron")).unwrap();
        let text = ron::ser::to_string_pretty(&report, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(PenetrationReport::from_ron(&text).unwrap(), report);
    }

    /// Every name a scenario's hits and consequences cite resolves to one of its internals, every
    /// timed list is in time order, and the path ends in or past the last plate: the sim marches a
    /// perforating shell on until it embeds or leaves the vehicle, so an embed holds the path's
    /// end and a perforated last plate has it beyond the exit. A ricochet's deflection leg is
    /// unconstrained.
    #[test]
    fn scenarios_are_self_consistent() {
        for (_, file) in player::SCENARIOS {
            let report = PenetrationReport::load(&scenario_dir().join(file)).unwrap();
            assert!(report.synthetic, "{file}: demo clips are synthetic");
            assert_self_consistent(file, &report);
        }
    }

    /// See [`scenarios_are_self_consistent`]; shared with the recorded scenarios.
    pub(crate) fn assert_self_consistent(file: &str, report: &PenetrationReport) {
        let cited = report
            .spall
            .iter()
            .flat_map(|burst| &burst.fragments)
            .flat_map(|fragment| &fragment.hits)
            .map(|hit| hit.volume.as_str())
            .chain(report.consequences.iter().map(|c| c.volume.as_str()));
        for name in cited {
            assert!(
                report.internal(name).is_some(),
                "{file}: unknown volume {name}"
            );
        }
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
        if report.impact.verdict != ShotVerdict::Ricochet {
            let plate = report.plates.last().expect("a plate");
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
