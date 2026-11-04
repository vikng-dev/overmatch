//! The construction bake, the one entry point for every construction: each spec under
//! `tools/constructions/` (a house generator's `<id>.house.ron` or an explicit `<id>.pieces.ron`)
//! to its `assets/constructions/<id>/<id>.construction.ron` (`overmatch_tools::construction`). Writes only a
//! file whose bytes change and the game's load gate admits (`overmatch::construction::admit`), and
//! prints what each bake made.

use overmatch::construction::schema;
use overmatch_tools::construction;

fn main() {
    let root = overmatch_tools::repo_root();
    let specs =
        construction::specs(&root).unwrap_or_else(|err| panic!("bake-constructions: {err}"));
    for spec in specs {
        let id = &spec.construction.id;
        let baked = construction::bake(&spec.construction, schema::substance_density)
            .unwrap_or_else(|err| panic!("bake-constructions: {}: {err}", spec.path));
        let text = construction::admitted(&baked.asset, &spec.path)
            .unwrap_or_else(|err| panic!("bake-constructions: {err}"));
        let path = construction::asset_path(&root, id);
        let unchanged = std::fs::read_to_string(&path).is_ok_and(|old| old == text);
        if !unchanged {
            let dir = path.parent().expect("an asset path has a folder");
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|err| panic!("bake-constructions: {}: {err}", dir.display()));
            std::fs::write(&path, &text)
                .unwrap_or_else(|err| panic!("bake-constructions: {}: {err}", path.display()));
        }
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
            text.len(),
            if unchanged { " (unchanged)" } else { "" },
        );
    }
}
