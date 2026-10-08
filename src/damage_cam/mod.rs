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
    pub spall: Vec<SpallBurst>,
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
    pub verdict: Verdict,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Through,
    Stopped,
    Ricochet,
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SpallBurst {
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

    fn scenario_dir() -> std::path::PathBuf {
        crate::assets::asset_root().join(player::SCENARIO_DIR)
    }

    #[test]
    fn report_round_trips_through_ron() {
        let report = PenetrationReport::load(&scenario_dir().join("through.ron")).unwrap();
        let text = ron::ser::to_string_pretty(&report, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(PenetrationReport::from_ron(&text).unwrap(), report);
    }

    /// Every name a scenario's hits and consequences cite resolves to one of its internals, and
    /// every timed list is in time order.
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
        }
    }
}
