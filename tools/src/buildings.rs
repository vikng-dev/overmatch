//! The building stage of the map compiler: a house spec (`tools/buildings/<id>.house.ron`) to the
//! building the game spawns (`assets/buildings/<id>/<id>.building.ron`,
//! [`overmatch::buildings::schema`]).
//!
//! Three steps: the generator lays out the house's elements ([`house`]), the fracture bake cuts
//! each into chunks ([`fracture`]), and the contact search finds the joints between chunks and
//! the chunks the ground holds ([`contact`]). The output is canonical ([`write`]): every number
//! rounded to a micrometre (or its unit's millionth) and printed by the same formatter, every list
//! in a fixed order, so two bakes of one spec are the same bytes.

use std::path::{Path, PathBuf};

use glam::DVec3;
use overmatch::buildings::schema::{self, BuildingAsset};

pub mod contact;
pub mod fracture;
pub mod geom;
pub mod house;
pub mod spec;

use fracture::CutCount;
use house::Material;
use spec::House;

/// The specs' folder, under the repository root.
pub const SPECS_DIR: &str = "tools/buildings";

/// A spec's file suffix.
pub const SPEC_SUFFIX: &str = ".house.ron";

/// One bake's output and what its cut rule did.
pub struct Baked {
    pub asset: BuildingAsset,
    pub cuts: CutCount,
}

/// `value` rounded to a millionth of its unit, with `-0.0` made `0.0`, as f32: the canonical
/// form of every number the bake writes.
fn canon(value: f64) -> f32 {
    let rounded = (value * 1.0e6).round() / 1.0e6;
    if rounded == 0.0 { 0.0 } else { rounded as f32 }
}

fn canon3(v: DVec3) -> [f32; 3] {
    [canon(v.x), canon(v.y), canon(v.z)]
}

/// Bake `house` (module doc). Refuses a house whose materials the shipped registry cannot weigh.
pub fn bake(house: &House) -> Result<Baked, String> {
    let plans = house::elements(house);
    let mut materials: Vec<Material> = plans.iter().map(|plan| plan.material).collect();
    materials.sort();
    materials.dedup();
    let density = |material: Material| schema::substance_density(material.key());
    let index = |material: Material| {
        u8::try_from(
            materials
                .iter()
                .position(|&m| m == material)
                .expect("every plan's material is in the table"),
        )
        .expect("two materials")
    };
    let mut cuts = CutCount::default();
    let mut prisms = Vec::new();
    let mut elements = Vec::new();
    let mut chunks = Vec::new();
    for (e, plan) in plans.iter().enumerate() {
        let material = index(plan.material);
        let rho = density(plan.material)?;
        let first = u32::try_from(prisms.len()).expect("chunk count fits u32");
        for prism in fracture::chunks(plan, &house.id, &house.cuts, &mut cuts) {
            let volume = prism.volume();
            chunks.push(schema::Chunk {
                element: u16::try_from(e).map_err(|_| "more elements than a u16 names")?,
                material,
                hull: prism.corners().into_iter().map(canon3).collect(),
                volume_m3: canon(volume),
                centroid: canon3(prism.centroid()),
                mass_kg: canon(volume * rho),
            });
            prisms.push(prism);
        }
        let frame = plan.prism.frame;
        elements.push(schema::Element {
            name: plan.name.clone(),
            kind: plan.kind,
            material,
            frame: schema::Frame {
                origin: canon3(frame.origin),
                u: canon3(frame.u),
                v: canon3(frame.v),
            },
            profile: plan
                .prism
                .profile
                .iter()
                .map(|p| [canon(p.x), canon(p.y)])
                .collect(),
            depth: [canon(plan.prism.near), canon(plan.prism.far)],
            chunks: [
                first,
                u32::try_from(prisms.len()).expect("chunk count fits u32"),
            ],
        });
    }
    let foundation_y = -f64::from(house.foundation_courses) * house.course_m;
    let index32 = |k: usize| u32::try_from(k).expect("chunk count fits u32");
    let asset = BuildingAsset {
        id: house.id.clone(),
        format: schema::FORMAT,
        foundation_y_m: canon(foundation_y),
        materials: materials.iter().map(|m| m.key().to_owned()).collect(),
        elements,
        chunks,
        joints: contact::joints(&prisms)
            .into_iter()
            .map(|joint| schema::Joint {
                a: index32(joint.a),
                b: index32(joint.b),
                area_m2: canon(joint.area),
                centroid: canon3(joint.centroid),
                normal: canon3(joint.normal),
            })
            .collect(),
        anchors: contact::anchors(&prisms, foundation_y)
            .into_iter()
            .map(|anchor| schema::Anchor {
                chunk: index32(anchor.chunk),
                area_m2: canon(anchor.area),
                centroid: canon3(anchor.centroid),
            })
            .collect(),
    };
    asset.check()?;
    Ok(Baked { asset, cuts })
}

/// The canonical text of `asset`: RON, one element, chunk, joint or anchor per line.
pub fn write(asset: &BuildingAsset) -> String {
    let config = ron::ser::PrettyConfig::new()
        .depth_limit(2)
        .struct_names(true)
        .new_line("\n")
        .indentor("    ");
    let body = ron::ser::to_string_pretty(asset, config).expect("the schema serializes");
    format!(
        "// Written by the building bake (`tools/`, `bake-buildings`) from `{SPECS_DIR}/{}{SPEC_SUFFIX}`.\n\
         // Do not edit: change the spec or the bake and bake again.\n{body}\n",
        asset.id
    )
}

/// Where the game reads building `id` from, under the repository root.
pub fn asset_path(root: &Path, id: &str) -> PathBuf {
    root.join("assets/buildings")
        .join(id)
        .join(format!("{id}.building.ron"))
}

/// Every spec under the repository root's [`SPECS_DIR`], in name order.
pub fn specs(root: &Path) -> Result<Vec<(PathBuf, House)>, String> {
    let dir = root.join(SPECS_DIR);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(SPEC_SUFFIX))
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            let house =
                House::from_ron(&text).map_err(|err| format!("{}: {err}", path.display()))?;
            let stem = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(SPEC_SUFFIX));
            if stem != Some(house.id.as_str()) {
                return Err(format!(
                    "{}: id {:?} is not the file's stem",
                    path.display(),
                    house.id
                ));
            }
            Ok((path, house))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The shipped specs, baked once per test.
    fn shipped() -> Vec<(House, Baked)> {
        specs(&crate::repo_root())
            .expect("the shipped specs parse")
            .into_iter()
            .map(|(_, house)| {
                let baked = bake(&house).expect("a shipped spec bakes");
                (house, baked)
            })
            .collect()
    }

    /// THE BAKE IS CANONICAL, AND THE SHIPPED FILE IS ITS OUTPUT: two bakes of every shipped spec
    /// write the same bytes, the bytes parse back to the same building through the game's own
    /// reader, and they are the bytes `assets/` ships. Two peers read those bytes into the map
    /// digest, so a bake that drifted (a hash-ordered list, an unrounded float, a platform's own
    /// trigonometry) or a file left stale behind its spec splits the handshake.
    #[test]
    fn two_bakes_write_the_same_bytes_and_the_shipped_file_is_one() {
        let root = crate::repo_root();
        for (house, baked) in shipped() {
            let text = write(&baked.asset);
            let again = write(&bake(&house).unwrap().asset);
            assert_eq!(text, again, "{}: two bakes differ", house.id);
            assert_eq!(
                BuildingAsset::from_ron(&text).as_ref(),
                Ok(&baked.asset),
                "{}: the text reads back as written",
                house.id
            );
            let path = asset_path(&root, &house.id);
            let shipped = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("{}: {err} — run bake-buildings", path.display()));
            assert!(
                shipped == text,
                "{} is stale against its spec or the bake — run bake-buildings",
                path.display()
            );
        }
    }

    /// THE JOINT GRAPH HOLDS THE BUILDING TOGETHER: every joint and anchor has positive area and a
    /// unit normal, every chunk reaches an anchor through joints (one connected graph), and every
    /// anchor is a downward chunk face on the foundation plane, of a chunk that reaches that
    /// plane. The next slice's support law walks from the anchors over the joints; a chunk the
    /// graph cannot reach would fall the moment anything changed, and an anchor above the
    /// foundation would hold a wall up in mid-air.
    #[test]
    fn every_chunk_is_joined_to_the_ground_and_anchors_lie_on_the_foundation() {
        for (house, baked) in shipped() {
            let asset = &baked.asset;
            let n = asset.chunks.len();
            assert!(!asset.anchors.is_empty(), "{}: no anchor", house.id);
            let mut parent: Vec<usize> = (0..n).collect();
            fn root(parent: &mut [usize], mut k: usize) -> usize {
                while parent[k] != k {
                    parent[k] = parent[parent[k]];
                    k = parent[k];
                }
                k
            }
            for joint in &asset.joints {
                assert!(joint.area_m2 > 0.0, "{}: joint {joint:?}", house.id);
                let normal = glam::Vec3::from_array(joint.normal);
                assert!((normal.length() - 1.0).abs() < 1e-5, "{joint:?}");
                let (a, b) = (
                    root(&mut parent, joint.a as usize),
                    root(&mut parent, joint.b as usize),
                );
                parent[a] = b;
            }
            let ground: BTreeSet<usize> = asset
                .anchors
                .iter()
                .map(|anchor| root(&mut parent, anchor.chunk as usize))
                .collect();
            for k in 0..n {
                assert!(
                    ground.contains(&root(&mut parent, k)),
                    "{}: chunk {k} ({}) reaches no anchor",
                    house.id,
                    asset.elements[usize::from(asset.chunks[k].element)].name
                );
            }
            assert_eq!(ground.len(), 1, "{}: one connected building", house.id);
            for anchor in &asset.anchors {
                assert!(anchor.area_m2 > 0.0);
                assert_eq!(anchor.centroid[1], asset.foundation_y_m, "{anchor:?}");
                let lowest = asset.chunks[anchor.chunk as usize]
                    .hull
                    .iter()
                    .map(|p| p[1])
                    .fold(f32::INFINITY, f32::min);
                assert_eq!(lowest, asset.foundation_y_m, "{anchor:?}");
            }
        }
    }

    /// THE CHUNKS TILE THEIR ELEMENTS: each element's chunks sum to its volume, each chunk lies
    /// inside its element's bounds, and the chunks are about the size the rule aims at (no
    /// dimension of a masonry chunk past two and a half brick modules, a gable corner's widest).
    /// A cut that dropped a sliver, or let two chunks overlap, breaks the volume sum; a row that
    /// never split breaks the size.
    #[test]
    fn the_chunks_tile_each_element() {
        for (house, baked) in shipped() {
            let asset = &baked.asset;
            for element in &asset.elements {
                let [first, end] = element.chunks;
                let run = &asset.chunks[first as usize..end as usize];
                let sum: f32 = run.iter().map(|chunk| chunk.volume_m3).sum();
                let whole = element.volume();
                assert!(
                    (sum - whole).abs() <= 1e-4 * whole,
                    "{}: {} chunks sum to {sum} m³ of {whole}",
                    house.id,
                    element.name
                );
                let corners = element.points();
                let (lo, hi) = corners.iter().fold(
                    (glam::Vec3::INFINITY, glam::Vec3::NEG_INFINITY),
                    |(lo, hi), &p| (lo.min(p), hi.max(p)),
                );
                for chunk in run {
                    for &p in &chunk.hull {
                        let p = glam::Vec3::from_array(p);
                        assert!(
                            p.cmpge(lo - 1e-5).all() && p.cmple(hi + 1e-5).all(),
                            "{}: a chunk of {} leaves it",
                            house.id,
                            element.name
                        );
                    }
                    if asset.materials[usize::from(chunk.material)] == "Brick" {
                        let (clo, chi) = chunk.hull.iter().fold(
                            (glam::Vec3::INFINITY, glam::Vec3::NEG_INFINITY),
                            |(lo, hi), &p| {
                                (
                                    lo.min(glam::Vec3::from_array(p)),
                                    hi.max(glam::Vec3::from_array(p)),
                                )
                            },
                        );
                        let size = chi - clo;
                        assert!(
                            size.max_element() <= 2.5 * house.module_m as f32,
                            "{}: a chunk of {} spans {size}",
                            house.id,
                            element.name
                        );
                    }
                }
            }
        }
    }

    /// THE CUT PREFERS SEAMS AND SOMETIMES CRACKS: across a shipped house, most masonry cuts lie on
    /// head joints and some cross bricks. A rule that never cracked (the chance ignored) or always
    /// did (the joint ignored) fails here.
    #[test]
    fn most_cuts_follow_joints_and_some_crack_through_bricks() {
        for (house, baked) in shipped() {
            let CutCount { joints, cracks } = baked.cuts;
            let share = joints as f64 / (joints + cracks) as f64;
            assert!(
                cracks > 0 && share > 0.5 && share < 1.0,
                "{}: {joints} joint cuts, {cracks} cracks",
                house.id
            );
        }
    }

    /// THE STREAM IS NAMED, NOT SEQUENCED: the same names draw the same numbers, other names
    /// others, so one element's cuts never shift when another element is added or re-cut.
    #[test]
    fn the_cut_stream_is_a_function_of_its_names() {
        let draw = |parts: &[&str]| fracture::Stream::new(parts).unit();
        assert_eq!(
            draw(&["house_1", "east/pier_0"]),
            draw(&["house_1", "east/pier_0"])
        );
        assert_ne!(
            draw(&["house_1", "east/pier_0"]),
            draw(&["house_1", "east/pier_1"])
        );
        assert_ne!(
            draw(&["ab", "c"]),
            draw(&["a", "bc"]),
            "the separator counts"
        );
        let mut stream = fracture::Stream::new(&["x"]);
        assert!(
            (0..1000)
                .map(|_| stream.unit())
                .all(|u| (0.0..1.0).contains(&u))
        );
    }
}
