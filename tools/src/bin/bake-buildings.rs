//! The building bake: every spec under `tools/buildings/` to its
//! `assets/buildings/<id>/<id>.building.ron` (`overmatch_tools::buildings`). Writes only a file
//! whose bytes change, and prints what each bake made.

use overmatch_tools::buildings;

fn main() {
    let root = overmatch_tools::repo_root();
    let specs = buildings::specs(&root).unwrap_or_else(|err| panic!("bake-buildings: {err}"));
    for (spec, house) in specs {
        let baked = buildings::bake(&house)
            .unwrap_or_else(|err| panic!("bake-buildings: {}: {err}", spec.display()));
        let text = buildings::write(&baked.asset);
        let path = buildings::asset_path(&root, &house.id);
        let unchanged = std::fs::read_to_string(&path).is_ok_and(|old| old == text);
        if !unchanged {
            let dir = path.parent().expect("an asset path has a folder");
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|err| panic!("bake-buildings: {}: {err}", dir.display()));
            std::fs::write(&path, &text)
                .unwrap_or_else(|err| panic!("bake-buildings: {}: {err}", path.display()));
        }
        let asset = &baked.asset;
        println!(
            "{}: {} elements, {} chunks, {} joints, {} anchors; {} cuts on head joints, {} \
             cracks; {} bytes{}",
            house.id,
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
