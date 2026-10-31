//! The building stage of the map compiler: a house spec (`tools/buildings/<id>.house.ron`) to the
//! building the game spawns (`assets/buildings/<id>/<id>.building.ron`,
//! [`overmatch::buildings::schema`]).
//!
//! Three steps: the generator lays out the house's elements ([`house`]), the fracture bake cuts
//! each into chunks ([`fracture`]), and the contact search finds the joints between chunks and
//! the chunks the ground holds ([`contact`]); what holds each joint between two components is the
//! attachment kind the spec's join for their elements' kinds names ([`spec::Join`]), written by
//! name: the strengths stay in the shared list, which the game reads. The output is canonical
//! ([`write`]): every number rounded to a micrometre (or its unit's millionth) and printed by the
//! same formatter, every list in a fixed order, so two bakes of one spec are the same bytes.

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

/// Bake `house` (module doc). Refuses a house whose materials the shipped registry cannot weigh,
/// two components' parts touching where no join says what holds them, and a join no two parts
/// make.
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
    let mut components: Vec<String> = Vec::new();
    for plan in &plans {
        if !components.contains(&plan.component) {
            components.push(plan.component.clone());
        }
    }
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
            component: u16::try_from(
                components
                    .iter()
                    .position(|c| *c == plan.component)
                    .expect("every plan's component is in the table"),
            )
            .map_err(|_| "more components than a u16 names")?,
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
    let attachments = attachment_kinds(house);
    let mut used = vec![false; house.joins.len()];
    let joints = contact::joints(&prisms)
        .into_iter()
        .map(|joint| {
            Ok(schema::Joint {
                a: index32(joint.a),
                b: index32(joint.b),
                area_m2: canon(joint.area),
                centroid: canon3(joint.centroid),
                normal: canon3(joint.normal),
                bond: bond(
                    house,
                    &attachments,
                    &plans,
                    &chunks,
                    (joint.a, joint.b),
                    &mut used,
                )?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if let Some(k) = used.iter().position(|used| !used) {
        return Err(format!(
            "the join of {:?} holds no joint: no two parts of those kinds touch",
            house.joins[k].kinds
        ));
    }
    let asset = BuildingAsset {
        id: house.id.clone(),
        format: schema::FORMAT,
        foundation_y_m: canon(foundation_y),
        materials: materials.iter().map(|m| m.key().to_owned()).collect(),
        attachments,
        components,
        joints,
        elements,
        chunks,
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

/// The attachment kinds `house`'s joins name, each once, in name order: the asset's table.
fn attachment_kinds(house: &House) -> Vec<String> {
    let mut kinds: Vec<String> = house.joins.iter().map(|join| join.by.clone()).collect();
    kinds.sort();
    kinds.dedup();
    kinds
}

/// What holds the joint between chunks `a` and `b`: inside one component its seam; between two,
/// the kind in `attachments` that the house's join for their elements' kinds names
/// ([`spec::Join`]), marked in `used` (one flag per join). Refuses a pair of kinds no join holds.
fn bond(
    house: &House,
    attachments: &[String],
    plans: &[house::ElementPlan],
    chunks: &[schema::Chunk],
    (a, b): (usize, usize),
    used: &mut [bool],
) -> Result<schema::Bond, String> {
    let (pa, pb) = (
        &plans[usize::from(chunks[a].element)],
        &plans[usize::from(chunks[b].element)],
    );
    if pa.component == pb.component {
        return Ok(schema::Bond::Seam);
    }
    let k = house
        .joins
        .iter()
        .position(|join| join.joins((pa.kind, pb.kind)))
        .ok_or_else(|| {
            format!(
                "{} and {} touch, and no join holds a {:?} to a {:?}",
                pa.name, pb.name, pa.kind, pb.kind
            )
        })?;
    used[k] = true;
    let index = attachments
        .iter()
        .position(|kind| *kind == house.joins[k].by)
        .expect("the table holds every kind a join names");
    Ok(schema::Bond::Attached(
        u8::try_from(index).map_err(|_| "more attachment kinds than a u8 names")?,
    ))
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
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|err| format!("{}: {err}", dir.display()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
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
    /// dimension of a chunk a masonry cutter made past two and a half of that cutter's modules, a
    /// gable corner's widest). A cut that dropped a sliver, or let two chunks overlap, breaks the
    /// volume sum; a row that never split breaks the size.
    #[test]
    fn the_chunks_tile_each_element() {
        for (house, baked) in shipped() {
            let asset = &baked.asset;
            for (element, plan) in asset.elements.iter().zip(house::elements(&house)) {
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
                    if let house::Cutter::Masonry { module, .. } = plan.cutter {
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
                            size.max_element() <= 2.5 * module as f32,
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

    /// WHAT HOLDS TWO PARTS IS THE SPEC'S, NEVER THE BAKE'S: every joint between two components
    /// of a shipped house names the attachment kind its spec's join for the two elements' kinds
    /// names, and nothing more (the kind's strength is the shared list's, which the game reads),
    /// so the same house with its roof's join to the plates turned to "resting" bakes those joints
    /// resting and nothing else changes. A spec with a touching pair no join names, a join no pair
    /// makes, or a join naming a kind the shared list does not hold is refused naming it. A bake
    /// that chose a bond by element kind or material, or fell back to one where the spec is
    /// silent, fails here; a roof nailed to its plate and a beam laid loose on a wall differ only
    /// by it.
    #[test]
    fn every_bond_between_components_is_the_kind_its_spec_names() {
        use overmatch::buildings::attachments::ATTACHMENTS_PATH;
        use overmatch::buildings::schema::{Bond, ElementKind};
        // The kind a joint names, `None` for a component's own seam.
        let kind_of = |asset: &BuildingAsset, bond: Bond| match bond {
            Bond::Seam => None,
            Bond::Attached(k) => Some(asset.attachments[usize::from(k)].clone()),
        };
        for (house, baked) in shipped() {
            let asset = &baked.asset;
            let element = |c: u32| &asset.elements[usize::from(asset.chunks[c as usize].element)];
            let expected = |house: &House, a: u32, b: u32| {
                let (ea, eb) = (element(a), element(b));
                (ea.component != eb.component).then(|| {
                    house
                        .joins
                        .iter()
                        .find(|join| join.joins((ea.kind, eb.kind)))
                        .expect("a baked pair has its join")
                        .by
                        .clone()
                })
            };
            for joint in &asset.joints {
                assert_eq!(
                    kind_of(asset, joint.bond),
                    expected(&house, joint.a, joint.b),
                    "{joint:?}"
                );
            }
            let roof_on_plate = (ElementKind::WallPlate, ElementKind::Roof);
            let mut loose = house.clone();
            let k = loose
                .joins
                .iter()
                .position(|join| join.joins(roof_on_plate))
                .expect("the shipped house joins its roof to its plates");
            loose.joins[k].by = "resting".to_owned();
            let rebaked = bake(&loose).expect("the loose roof bakes");
            let mut changed = 0;
            for (was, now) in asset.joints.iter().zip(&rebaked.asset.joints) {
                assert_eq!((was.a, was.b), (now.a, now.b));
                let kind = kind_of(&rebaked.asset, now.bond);
                assert_eq!(kind, expected(&loose, now.a, now.b));
                changed += usize::from(kind_of(asset, was.bond) != kind);
            }
            assert!(
                changed > 0,
                "{}: the roof's joints to its plates rest",
                house.id
            );
            let mut silent = house.clone();
            silent.joins.remove(k);
            let err = bake(&silent).map(|_| ()).unwrap_err();
            assert!(err.contains("no join holds"), "{err}");
            let mut idle = house.clone();
            idle.joins.push(spec::Join {
                kinds: (ElementKind::Sill, ElementKind::Roof),
                by: "resting".to_owned(),
            });
            let err = bake(&idle).map(|_| ()).unwrap_err();
            assert!(err.contains("holds no joint"), "{err}");
        }
        for (path, _) in specs(&crate::repo_root()).expect("the shipped specs parse") {
            let text = std::fs::read_to_string(&path).unwrap();
            let glued = text.replacen("by: \"", "by: \"glued_", 1);
            assert_ne!(glued, text, "{}: a join names a kind", path.display());
            let err = House::from_ron(&glued).unwrap_err();
            for part in ["glued_", ATTACHMENTS_PATH, "name one of those"] {
                assert!(err.contains(part), "{part}: {err}");
            }
        }
    }

    /// A HOUSE THE GENERATOR CANNOT LAY OUT IS REFUSED BEFORE ANY GEOMETRY: the generator lays a
    /// pier before, between and after a wall's openings in `u` order, so an opening past the
    /// wall's face or one over the next leaves a negative pier, and a timber piece range with no
    /// finite top is a cut that never ends. Mutant: a check that drops the span, the order or the
    /// range's top passes one of these to the generator.
    #[test]
    fn a_house_the_generator_cannot_lay_out_is_refused() {
        for (_, house) in specs(&crate::repo_root()).expect("the shipped specs parse") {
            // FIXTURE DATA: constructed openings exercise the span/order gate regardless of
            // whether this shipped house has any openings on this wall.
            let mut house = house;
            house.walls.east = vec![spec::Opening {
                u: (-0.25 * house.length_m, 0.0),
                courses: (0, house.eaves_course),
            }];
            house
                .check()
                .expect("the constructed opening is inside the wall");
            let (_, end) = house.wall_span("east");
            let mut past = house.clone();
            past.walls
                .east
                .last_mut()
                .expect("the fixture authors an opening")
                .u
                .1 = end + 1.0;
            let mut over = house.clone();
            let first = over.walls.east[0].u;
            over.walls.east.push(spec::Opening {
                u: (first.1 - 0.1, first.1 + 0.1),
                courses: over.walls.east[0].courses,
            });
            let mut endless = house.clone();
            endless.cuts.plate_piece_m.1 = f64::INFINITY;
            for (case, broken, field) in [
                ("past the wall", past, "pier"),
                ("over the next", over, "pier"),
                ("an endless piece", endless, "plate_piece_m"),
            ] {
                let err = broken.check().unwrap_err();
                assert!(err.contains(field), "{}: {case}: {err}", house.id);
            }
        }
    }
}
