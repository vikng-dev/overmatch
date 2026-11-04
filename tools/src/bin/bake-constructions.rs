//! The construction bake, the one entry point for every construction: each spec under
//! `tools/constructions/` (a house generator's `<id>.house.ron` or an explicit `<id>.pieces.ron`)
//! to its `assets/constructions/<id>/<id>.construction.ron` (`overmatch_tools::construction`). Writes only a
//! file whose bytes change and the game's load gate admits (`overmatch::construction::admit`), and
//! prints what each bake made.

use overmatch_tools::construction;

fn main() {
    let root = overmatch_tools::repo_root();
    let specs =
        construction::specs(&root).unwrap_or_else(|err| panic!("bake-constructions: {err}"));
    for spec in specs {
        let id = &spec.construction.id;
        let construction::Written {
            baked,
            bytes,
            unchanged,
        } = construction::bake_to(&root, &spec)
            .unwrap_or_else(|err| panic!("bake-constructions: {err}"));
        let asset = &baked.asset;
        let volume: f64 = asset.chunks.iter().map(|c| f64::from(c.volume_m3)).sum();
        let mass: f64 = asset.chunks.iter().map(|c| f64::from(c.mass_kg)).sum();
        let joined: f64 = asset.joints.iter().map(|j| f64::from(j.area_m2)).sum();
        println!(
            "{id}: {} elements, {} chunks ({volume:.6} m³, {mass:.3} kg), {} joints \
             ({joined:.6} m²), {} anchors; {} cuts on head joints, {} cracks; {} bytes{}",
            asset.elements.len(),
            asset.chunks.len(),
            asset.joints.len(),
            asset.anchors.len(),
            baked.cuts.joints,
            baked.cuts.cracks,
            bytes,
            if unchanged { " (unchanged)" } else { "" },
        );
    }
}
