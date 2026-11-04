//! The building stage of the map compiler: construction data (`tools/buildings/`) to the building
//! the game spawns (`assets/buildings/<id>/<id>.building.ron`, [`overmatch::buildings::schema`]).
//!
//! Three steps: a construction's pieces come from its spec ([`spec`]: a house generator's
//! parameters through [`house`], or convex pieces stated outright), the common cutter cuts each
//! piece into chunks by the pattern its data names ([`fracture`]), and the contact search finds the
//! joints between chunks and the chunks the ground holds ([`contact`]). What holds each joint
//! between two components is the attachment kind the spec's join for their elements' kinds names
//! ([`spec::Join`]), written by name: the strengths stay in the shared list, which the game reads;
//! each chunk's mass is its volume times its material row's density. The output is canonical
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
use spec::{Construction, ConstructionSpec, House};

/// The specs' folder, under the repository root.
pub const SPECS_DIR: &str = "tools/buildings";

/// A house generator's spec's file suffix ([`House`]).
pub const HOUSE_SUFFIX: &str = ".house.ron";

/// An explicit construction's file suffix ([`ConstructionSpec`]).
pub const CONSTRUCTION_SUFFIX: &str = ".construction.ron";

/// One bake's output and what its cut rule did.
pub struct Baked {
    pub asset: BuildingAsset,
    pub cuts: CutCount,
}

/// How far a piece's or a chunk's corner may sit off its face's plane, m (Resolution: the writer
/// rounds every coordinate to a micrometre, so a corner nearer its plane than that is on it in the
/// written file).
pub(crate) const PLANE_M: f64 = 1.0e-6;

/// `value` rounded to a millionth of its unit, with `-0.0` made `0.0`, as f32: the canonical
/// form of every number the bake writes.
fn canon(value: f64) -> f32 {
    let rounded = (value * 1.0e6).round() / 1.0e6;
    if rounded == 0.0 { 0.0 } else { rounded as f32 }
}

fn canon3(v: DVec3) -> [f32; 3] {
    [canon(v.x), canon(v.y), canon(v.z)]
}

/// Bake `construction` (module doc), weighing each chunk by `density` (kg/m³) of its piece's
/// material key: the shipped registry's through [`schema::substance_density`]. Refuses a material
/// `density` refuses, a chunk that is no closed convex cell, two components' parts touching where
/// no join says what holds them, and a join no two parts make.
pub fn bake(
    construction: &Construction,
    density: impl Fn(&str) -> Result<f64, String>,
) -> Result<Baked, String> {
    let pieces = &construction.pieces;
    let mut materials: Vec<&str> = pieces.iter().map(|piece| piece.material.as_str()).collect();
    materials.sort_unstable();
    materials.dedup();
    let index = |material: &str| {
        u8::try_from(
            materials
                .iter()
                .position(|&m| m == material)
                .expect("every piece's material is in the table"),
        )
        .map_err(|_| "more materials than a u8 names")
    };
    let mut components: Vec<String> = Vec::new();
    for piece in pieces {
        if !components.contains(&piece.component) {
            components.push(piece.component.clone());
        }
    }
    let mut cuts = CutCount::default();
    let mut solids = Vec::new();
    let mut elements = Vec::new();
    let mut chunks = Vec::new();
    let hull = |solid: &geom::Polyhedron| solid.corners.iter().map(|&p| canon3(p)).collect();
    for (e, piece) in pieces.iter().enumerate() {
        let material = index(&piece.material)?;
        let rho = density(&piece.material).map_err(|err| format!("{}: {err}", piece.name))?;
        let first = u32::try_from(solids.len()).expect("chunk count fits u32");
        for solid in fracture::chunks(piece, &construction.id, &mut cuts) {
            solid.certify(PLANE_M).map_err(|err| {
                format!(
                    "a chunk of {} is no closed convex cell ({err}); fix its pattern",
                    piece.name
                )
            })?;
            let volume = solid.volume();
            chunks.push(schema::Chunk {
                element: u16::try_from(e).map_err(|_| "more elements than a u16 names")?,
                material,
                hull: hull(&solid),
                volume_m3: canon(volume),
                centroid: canon3(solid.centroid()),
                mass_kg: canon(volume * rho),
            });
            solids.push(solid);
        }
        elements.push(schema::Element {
            name: piece.name.clone(),
            kind: piece.kind,
            component: u16::try_from(
                components
                    .iter()
                    .position(|c| *c == piece.component)
                    .expect("every piece's component is in the table"),
            )
            .map_err(|_| "more components than a u16 names")?,
            material,
            surface_frame: piece.surface_frame,
            hull: hull(&piece.solid),
            chunks: [
                first,
                u32::try_from(solids.len()).expect("chunk count fits u32"),
            ],
        });
    }
    let index32 = |k: usize| u32::try_from(k).expect("chunk count fits u32");
    let attachments = attachment_kinds(construction);
    let mut used = vec![false; construction.joins.len()];
    let joints = contact::joints(&solids)
        .into_iter()
        .map(|joint| {
            Ok(schema::Joint {
                a: index32(joint.a),
                b: index32(joint.b),
                area_m2: canon(joint.area),
                centroid: canon3(joint.centroid),
                normal: canon3(joint.normal),
                bond: bond(
                    construction,
                    &attachments,
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
            construction.joins[k].kinds
        ));
    }
    let foundation_attachment = u8::try_from(
        attachments
            .iter()
            .position(|kind| kind == &construction.foundation_attachment)
            .expect("the table holds the foundation kind"),
    )
    .map_err(|_| "more attachment kinds than a u8 names")?;
    let asset = BuildingAsset {
        id: construction.id.clone(),
        format: schema::FORMAT,
        foundation_y_m: canon(construction.foundation_y),
        materials: materials.iter().map(|&m| m.to_owned()).collect(),
        attachments,
        components,
        joints,
        elements,
        chunks,
        anchors: contact::anchors(&solids, construction.foundation_y)
            .into_iter()
            .map(|anchor| schema::Anchor {
                chunk: index32(anchor.chunk),
                attachment: foundation_attachment,
                area_m2: canon(anchor.area),
                centroid: canon3(anchor.centroid),
            })
            .collect(),
    };
    asset.check()?;
    Ok(Baked { asset, cuts })
}

/// The attachment kinds the joins and foundation name, each once, in name order.
fn attachment_kinds(construction: &Construction) -> Vec<String> {
    let mut kinds: Vec<String> = construction
        .joins
        .iter()
        .map(|join| join.by.clone())
        .collect();
    kinds.push(construction.foundation_attachment.clone());
    kinds.sort();
    kinds.dedup();
    kinds
}

/// What holds the joint between chunks `a` and `b`: inside one component its seam; between two,
/// the kind in `attachments` that the construction's join for their elements' kinds names
/// ([`spec::Join`]), marked in `used` (one flag per join). Refuses a pair of kinds no join holds.
fn bond(
    construction: &Construction,
    attachments: &[String],
    chunks: &[schema::Chunk],
    (a, b): (usize, usize),
    used: &mut [bool],
) -> Result<schema::Bond, String> {
    let (pa, pb) = (
        &construction.pieces[usize::from(chunks[a].element)],
        &construction.pieces[usize::from(chunks[b].element)],
    );
    if pa.component == pb.component {
        return Ok(schema::Bond::Seam);
    }
    let k = construction
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
        .position(|kind| *kind == construction.joins[k].by)
        .expect("the table holds every kind a join names");
    Ok(schema::Bond::Attached(
        u8::try_from(index).map_err(|_| "more attachment kinds than a u8 names")?,
    ))
}

/// The canonical text of `asset`, baked from the spec file `spec` (under the repository root):
/// RON, one element, chunk, joint or anchor per line.
pub fn write(asset: &BuildingAsset, spec: &str) -> String {
    let config = ron::ser::PrettyConfig::new()
        .depth_limit(2)
        .struct_names(true)
        .new_line("\n")
        .indentor("    ");
    let body = ron::ser::to_string_pretty(asset, config).expect("the schema serializes");
    format!(
        "// Written by the building bake (`tools/`, `bake-buildings`) from `{spec}`.\n\
         // Do not edit: change the spec or the bake and bake again.\n{body}\n"
    )
}

/// Where the game reads building `id` from, under the repository root.
pub fn asset_path(root: &Path, id: &str) -> PathBuf {
    root.join("assets/buildings")
        .join(id)
        .join(format!("{id}.building.ron"))
}

/// One spec under [`SPECS_DIR`]: its path under the repository root, and its construction.
pub struct Spec {
    pub path: String,
    pub construction: Construction,
}

/// Every spec under the repository root's [`SPECS_DIR`], house generators' and explicit
/// constructions' alike, in file name order. Refuses a file whose id is not its stem and two
/// specs of one id.
pub fn specs(root: &Path) -> Result<Vec<Spec>, String> {
    let _shipped = spec::Shipped::enter();
    let dir = root.join(SPECS_DIR);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|err| format!("{}: {err}", dir.display()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|name| name.ends_with(HOUSE_SUFFIX) || name.ends_with(CONSTRUCTION_SUFFIX))
        .collect();
    names.sort();
    let mut out: Vec<Spec> = Vec::with_capacity(names.len());
    for name in names {
        let path = format!("{SPECS_DIR}/{name}");
        let text =
            std::fs::read_to_string(root.join(&path)).map_err(|err| format!("{path}: {err}"))?;
        let (stem, construction) = match name.strip_suffix(HOUSE_SUFFIX) {
            Some(stem) => (
                stem,
                House::from_ron(&text).map(|house| house::construction(&house)),
            ),
            None => (
                name.strip_suffix(CONSTRUCTION_SUFFIX)
                    .expect("the filter keeps only the two suffixes"),
                ConstructionSpec::from_ron(&text),
            ),
        };
        let construction = construction.map_err(|err| format!("{path}: {err}"))?;
        if stem != construction.id {
            return Err(format!(
                "{path}: id {:?} is not the file's stem",
                construction.id
            ));
        }
        if out.iter().any(|spec| spec.construction.id == stem) {
            return Err(format!("{path}: another spec already bakes {stem}"));
        }
        out.push(Spec { path, construction });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
