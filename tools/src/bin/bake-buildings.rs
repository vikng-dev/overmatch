//! The building bake, the one entry point for every construction: each spec under
//! `tools/buildings/` (a house generator's `<id>.house.ron` or an explicit `<id>.construction.ron`)
//! to its `assets/buildings/<id>/<id>.building.ron` (`overmatch_tools::buildings`). Writes only a
//! file whose bytes change and the game's load gate admits (`overmatch::buildings::admit`), and
//! prints what each bake made.

use overmatch::buildings::schema;
use overmatch_tools::buildings;

fn main() {
    let root = overmatch_tools::repo_root();
    let specs = buildings::specs(&root).unwrap_or_else(|err| panic!("bake-buildings: {err}"));
    for spec in specs {
        let id = &spec.construction.id;
        let baked = buildings::bake(&spec.construction, schema::substance_density)
            .unwrap_or_else(|err| panic!("bake-buildings: {}: {err}", spec.path));
        let text = buildings::write(&baked.asset, &spec.path);
        let path = buildings::asset_path(&root, id);
        overmatch::buildings::admit(id, &text).unwrap_or_else(|err| {
            panic!(
                "bake-buildings: {}: the game's load gate refuses {}: {err}",
                spec.path,
                path.display()
            )
        });
        let unchanged = std::fs::read_to_string(&path).is_ok_and(|old| old == text);
        if !unchanged {
            let dir = path.parent().expect("an asset path has a folder");
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|err| panic!("bake-buildings: {}: {err}", dir.display()));
            std::fs::write(&path, &text)
                .unwrap_or_else(|err| panic!("bake-buildings: {}: {err}", path.display()));
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
