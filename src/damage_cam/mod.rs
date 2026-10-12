//! The damage cam's record: [`PenetrationReport`], one per (shot, struck tank), and its player.
//!
//! The report is plain serde data — no entities, handles or meshes — so any source can fill it (a
//! hand-authored RON clip today, the sim's accumulator later) and [`player`] consumes it unchanged.
//!
//! Frame: hull-local — origin at the hull root node, the glb's axes (`-Z` forward, `+Y` up, `+X`
//! right). Units: metres for positions, unit vectors for directions, seconds for `t` measured from
//! first contact (the approach is negative), degrees for angles, millimetres for armour, kilograms
//! and metres per second for the projectile. `synthetic` marks a report whose numbers were authored,
//! not simulated; the player prints it on screen.
//!
//! v0: the sim-side fill will reshape it.

pub mod player;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use crate::ballistics::{ImpactSurface, Verdict};

/// What a pen cam shows the round did. A pen cam exists only for a hit on a player's tank, and
/// only these three verdicts open one: [`Verdict::Struck`] has no counterpart, so a record cannot
/// hold it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenCamVerdict {
    Through,
    Stopped,
    Ricochet,
}

impl PenCamVerdict {
    /// The round entered the material: it stopped inside it or went through.
    pub fn entered(self) -> bool {
        matches!(self, Self::Stopped | Self::Through)
    }

    /// The pen cam an impact opens, if any. A ground hit opens none; nor does an armour contact
    /// that resolved no crossing (`Struck` on armour: a fail-closed walk, which the march logs as
    /// an error where the walk fails).
    pub fn of_impact(surface: ImpactSurface, verdict: Verdict) -> Option<Self> {
        match (surface, verdict) {
            (ImpactSurface::Terrain, _) | (ImpactSurface::Armor, Verdict::Struck) => None,
            (ImpactSurface::Armor, Verdict::Through) => Some(Self::Through),
            (ImpactSurface::Armor, Verdict::Stopped) => Some(Self::Stopped),
            (ImpactSurface::Armor, Verdict::Ricochet) => Some(Self::Ricochet),
        }
    }
}

/// A finished shot against one tank, in causal order.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PenetrationReport {
    pub synthetic: bool,
    pub projectile: Projectile,
    pub impact: Impact,
    /// Shell crossings in path order.
    pub plates: Vec<Plate>,
    /// Timed shell path, approach included; a ricochet's deflection leg continues it.
    pub path: Vec<PathPoint>,
    pub spall: Vec<SpallCone>,
    pub consequences: Vec<Consequence>,
    /// The struck tank's crew and module volumes at the time of the shot.
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
    pub verdict: PenCamVerdict,
}

/// One plate crossing. Thickness is three numbers: the geometric chord along the path, its normal
/// steel equivalent, and the cost actually charged against the shell's capability.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Plate {
    pub name: String,
    pub t: f32,
    pub entry: [f32; 3],
    /// `None` when the shell did not come out the far side.
    pub exit: Option<[f32; 3]>,
    pub chord_mm: f32,
    pub steel_equiv_mm: f32,
    pub cost_mm: f32,
    pub overmatched: bool,
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
    pub fragments: Vec<Fragment>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Fragment {
    /// Where the fragment stopped.
    pub end: [f32; 3],
    /// Volumes it deposited into, in flight order; empty for a fragment that hit nothing alive.
    pub hits: Vec<FragmentHit>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FragmentHit {
    /// An [`Internal::name`].
    pub volume: String,
    pub damage: f32,
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

/// An axis-aligned box standing in for a crew station or module volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Internal {
    pub name: String,
    pub role: Role,
    pub center: [f32; 3],
    pub half_extents: [f32; 3],
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every armour verdict but `Struck` opens the pen cam of the same name; terrain opens none.
    #[test]
    fn only_a_resolved_armour_verdict_opens_a_pen_cam() {
        let all = [
            Verdict::Through,
            Verdict::Stopped,
            Verdict::Ricochet,
            Verdict::Struck,
        ];
        for verdict in all {
            assert_eq!(
                PenCamVerdict::of_impact(ImpactSurface::Terrain, verdict),
                None
            );
        }
        assert_eq!(
            all.map(|verdict| PenCamVerdict::of_impact(ImpactSurface::Armor, verdict)),
            [
                Some(PenCamVerdict::Through),
                Some(PenCamVerdict::Stopped),
                Some(PenCamVerdict::Ricochet),
                None,
            ]
        );
        for verdict in all {
            if let Some(opened) = PenCamVerdict::of_impact(ImpactSurface::Armor, verdict) {
                assert_eq!(opened.entered(), verdict.entered());
            }
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
            if report.impact.verdict.entered() {
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
    }

    fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}
