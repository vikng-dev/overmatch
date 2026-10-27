//! Gate: every crew station's HP pool is the one shared crew pool, and every other HP pool a tank
//! file authors is its sturdiness class's HP per m² times the box's measured mean face, within
//! [`TOLERANCE`] (`assets/ballistics/box_classes.ron`, design doc §6).
//!
//! The mean face is the mean of the three axis-aligned face areas of the box's bounds, read from
//! the sim geometry the bake extracts from every tank asset under `assets/` (the tank file and its
//! sim artifact, at rest). The class is the one the box's facets choose ([`BoxClass::of`]).
//!
//! `OVERMATCH_BOX_POOLS_SHOW=1` with `--nocapture` prints every pooled box's class, mean and
//! smallest face, pool and class × mean face: the table a tank file's pools are authored from.

use std::collections::BTreeMap;
use std::path::Path;

use crate::ballistics::box_classes::{BoxClass, BoxClasses};
use crate::damage_cam::shapes::TankAsset;

/// How far an authored pool may sit from its class × mean face, as a share of the latter: room
/// for rounding to whole points and for grouping like boxes under one value, never for a box
/// sturdier than its class.
const TOLERANCE: f32 = 0.25;

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

/// The three axis-aligned face areas (m²) of the bounds `lo`..`hi`.
fn faces((lo, hi): (bevy::math::Vec3, bevy::math::Vec3)) -> [f32; 3] {
    let e = hi - lo;
    [e.x * e.y, e.y * e.z, e.z * e.x]
}

/// INVARIANT: in every tank asset, every crew station's pool is the shared crew pool
/// ([`BoxClasses::crew_hp`]) and is exempt from class × face, and every other box with an HP pool
/// holds a pool within ±[`TOLERANCE`] of its class's HP per m² times its mean face. A crewman is one
/// entity across tanks, and the field's calibration (`assets/ballistics/spall_cone.ron`) and the
/// crew line rate (the round files, `assets/rounds/`) are set against the shared pool. The pen cam's
/// potency bands are calibrated in HP per m² of face and promise that a box of its class inside a
/// band is knocked out; a pool above its class breaks that promise for the box (it survives a band
/// that names its loss). A mutant pool, a crew pool that is not the shared one, a box whose
/// geometry no longer resolves, or a class read from the wrong facet fails here, naming the box.
#[test]
fn every_pool_is_the_crew_pool_or_its_class_times_its_mean_face() {
    let root = crate::assets::asset_root();
    let mut files = Vec::new();
    tank_files(&root, &root, &mut files);
    assert!(!files.is_empty(), "no tank asset under {}", root.display());
    let classes = BoxClasses::shipped();
    let show = std::env::var_os("OVERMATCH_BOX_POOLS_SHOW").is_some();
    let mut wrong = Vec::new();
    let mut pooled = 0;
    for file in &files {
        let asset = TankAsset::load(file).unwrap_or_else(|e| panic!("{file}: {e}"));
        let shapes = asset.shapes(&BTreeMap::new());
        for (name, volume) in &asset.spec().volumes {
            let Some(component) = volume.component() else {
                continue;
            };
            pooled += 1;
            let Some(class) = BoxClass::of(component) else {
                if show {
                    println!("{file} {name}: crew, hp {}", component.hp);
                }
                if component.hp != classes.crew_hp() {
                    wrong.push(format!(
                        "{file} {name}: crew hp {} is not the shared crew pool {}",
                        component.hp,
                        classes.crew_hp()
                    ));
                }
                continue;
            };
            let Some(shape) = shapes.get(name) else {
                wrong.push(format!("{file} {name}: no sim geometry"));
                continue;
            };
            let faces = faces(shape.bounds());
            let mean = faces.iter().sum::<f32>() / 3.0;
            let least = faces.iter().copied().fold(f32::INFINITY, f32::min);
            let expected = classes.hp_per_square_metre(class) as f32 * mean;
            if show {
                println!(
                    "{file} {name}: {class:?}, mean face {mean:.4} m², least face {least:.4} m², \
                     hp {}, class × mean face {expected:.1}",
                    component.hp
                );
            }
            if !(expected > 0.0 && (component.hp as f32 - expected).abs() <= TOLERANCE * expected) {
                wrong.push(format!(
                    "{file} {name}: hp {} is not {class:?} × {mean:.4} m² = {expected:.1} ± {:.0} %",
                    component.hp,
                    TOLERANCE * 100.0
                ));
            }
        }
    }
    assert!(pooled > 0, "no tank asset authors an HP pool");
    assert!(
        wrong.is_empty(),
        "pools off the crew pool or their class × mean face:\n{}",
        wrong.join("\n")
    );
}
