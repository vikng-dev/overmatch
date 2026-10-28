//! Gate: within every tank asset, every crew station's box presents one mean face, and it is the
//! face the pen cam's potency bands for that tank are read from ([`TankShapes::crew_face`], design
//! doc §5, "Potency colour").
//!
//! A box's mean face is the mean of the three axis-aligned face areas of its bounds
//! ([`ComponentShape::mean_face`]) at rest, read from the sim geometry the bake extracts from every
//! tank asset under `assets/` (the tank file and its sim artifact).

use std::collections::BTreeMap;
use std::path::Path;

use crate::damage_cam::shapes::{TankAsset, TankShapes};

/// How far a station's mean face may sit from its tank's crew face, as a share of it: room for the
/// f32 rounding of the bounds' products and the mean (DERIVED: a few ulps, under 10⁻⁶), never for a
/// second box size.
const TOLERANCE: f32 = 1.0e-5;

/// The angle (deg) every servo is turned to for the crew face, so a face read off the rest pose
/// differs from the stations' faces at rest: fixture data, large enough that a turret's box bounds
/// grow visibly when turned.
const POSE_DEG: f32 = 30.0;

/// Every `*.tank.ron` under `dir`, relative to `root`, sorted.
fn tank_files(root: &Path, dir: &Path, found: &mut Vec<String>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry reads").path();
        if path.is_dir() {
            tank_files(root, &path, found);
        } else if path.to_string_lossy().ends_with(".tank.ron") {
            let relative = path.strip_prefix(root).expect("under the asset root");
            found.push(relative.to_string_lossy().into_owned());
        }
    }
    found.sort();
}

/// INVARIANT: in every tank asset with a crew station, every station's box at rest presents the
/// tank's crew face ([`TankShapes::crew_face`], read here with every servo turned) within
/// [`TOLERANCE`]. The pen cam's bands part at
/// the crew pool and the least counted HP point over that one face and promise that a station
/// whose face lies wholly in knockout is knocked out and one wholly in nothing takes no HP point:
/// a station of a smaller face would survive a knockout band, one of a larger take a point in
/// nothing. A crew box resized alone, a crew face read from other boxes or off the rest pose, or
/// a station whose geometry no longer resolves fails here, naming the station.
#[test]
fn every_crew_box_of_a_tank_presents_the_tanks_crew_face() {
    let root = crate::assets::asset_root();
    let mut files = Vec::new();
    tank_files(&root, &root, &mut files);
    assert!(!files.is_empty(), "no tank asset under {}", root.display());
    let mut wrong = Vec::new();
    let mut stations = 0;
    for file in &files {
        let asset = TankAsset::load(file).unwrap_or_else(|e| panic!("{file}: {e}"));
        let at_rest: TankShapes = asset.shapes(&BTreeMap::new());
        let turned = asset.spec().servos.keys().map(|n| (n.clone(), POSE_DEG));
        let posed = asset.shapes(&turned.collect());
        let crew: Vec<&str> = asset
            .spec()
            .volumes
            .iter()
            .filter(|(_, volume)| volume.component().is_some_and(|c| c.crew.is_some()))
            .map(|(name, _)| name.as_str())
            .collect();
        if crew.is_empty() {
            continue;
        }
        let Some(face) = posed.crew_face() else {
            wrong.push(format!("{file}: crew stations but no crew face"));
            continue;
        };
        for name in crew {
            stations += 1;
            let Some(shape) = at_rest.get(name) else {
                wrong.push(format!("{file} {name}: no sim geometry"));
                continue;
            };
            let mean = shape.mean_face();
            if (mean - face).abs() > TOLERANCE * face || mean.is_nan() {
                wrong.push(format!(
                    "{file} {name}: mean face {mean:.6} m² is not the crew face {face:.6} m²"
                ));
            }
        }
    }
    assert!(stations > 0, "no tank asset has a crew station");
    assert!(
        wrong.is_empty(),
        "crew boxes off their tank's crew face:\n{}",
        wrong.join("\n")
    );
}
