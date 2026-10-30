//! The house generator: a [`House`] spec to its elements, the split tree's terminals (CGA's
//! mass → walls → piers, openings, lintels, sills; Müller et al. 2006). Each element is a right
//! prism with its material and the rule that will cut it ([`Cutter`]).
//!
//! # The house's frame and parts
//!
//! The building frame of [`overmatch::buildings::schema`]: `+Y` up, the origin on the ground point,
//! the ridge along `Z`. The two long walls (east at `+X`, west at `−X`) run the full length; the
//! two gable walls (north at `+Z`, south at `−Z`) stand between them. Every wall rises from the
//! foundation plane to the eaves and is split by its openings into piers (full height), sills
//! (under an opening) and lintels (over it). Above the eaves each gable wall carries its gable, a
//! pentagon whose sloped top is the roof's underside. A timber wall plate lies along each long
//! wall's top, its own top bevelled to the roof's slope, and each roof plane is a slab of boards
//! resting on the plates and the gables, mitred where the two meet at the ridge.
//!
//! Every slope is the spec's pitch as a ratio (rise over run, a Pythagorean pair's hypotenuse taken
//! by square root), so no angle and no trigonometry enters the bake.

use glam::{DVec2, DVec3};
use overmatch::buildings::schema::ElementKind;

use super::geom::{Frame, Prism};
use super::spec::{House, Opening};

/// What an element is made of, as a key of `assets/materials/materials.ron`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Material {
    Brick,
    Wood,
}

impl Material {
    pub fn key(self) -> &'static str {
        match self {
            Self::Brick => "Brick",
            Self::Wood => "Wood",
        }
    }
}

/// How the fracture bake cuts an element ([`super::fracture`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Cutter {
    /// Masonry in running bond: courses `course` tall from the foundation plane `base_y`, head
    /// joints every `module` from the wall's first `u`, `wall_start`.
    Masonry {
        base_y: f64,
        wall_start: f64,
        course: f64,
        module: f64,
    },
    /// Timber: optional boards across the profile (`axis · p` in steps of `width`, `count` of
    /// them), then pieces along the depth, the grain, within `piece` metres.
    Timber {
        boards: Option<(DVec2, f64, u32)>,
        piece: (f64, f64),
    },
}

/// One element as the generator lays it out.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementPlan {
    pub name: String,
    pub kind: ElementKind,
    pub material: Material,
    pub prism: Prism,
    pub cutter: Cutter,
}

/// The rectangle `[u0, u1] × [y0, y1]`, counter-clockwise.
fn rectangle(u0: f64, u1: f64, y0: f64, y1: f64) -> Vec<DVec2> {
    vec![
        DVec2::new(u0, y0),
        DVec2::new(u1, y0),
        DVec2::new(u1, y1),
        DVec2::new(u0, y1),
    ]
}

/// One wall: its name, its frame (`v` up, `w` outward), its `u` range and its depth range.
struct Wall {
    name: &'static str,
    frame: Frame,
    u: (f64, f64),
    depth: (f64, f64),
    openings: Vec<Opening>,
    gable: bool,
}

/// Every element of `house`, in emission order: the walls (east, west, north, south; along each
/// wall's `u`, a pier and then each opening's sill and lintel, a gable last), the wall plates, the
/// roof planes.
pub fn elements(house: &House) -> Vec<ElementPlan> {
    let h = house.course_m;
    let base_y = -f64::from(house.foundation_courses) * h;
    let course_y = |k: u32| base_y + f64::from(k) * h;
    let eaves_y = course_y(house.eaves_course);
    let (half_w, half_l, t) = (
        0.5 * house.width_m,
        0.5 * house.length_m,
        house.wall_thickness_m,
    );
    let inner_w = half_w - t;
    let (rise, run) = (f64::from(house.pitch.0), f64::from(house.pitch.1));
    let slope = rise / run;
    let hyp = (rise * rise + run * run).sqrt();
    let (cos, sin) = (run / hyp, rise / hyp);
    // The roof's underside: `plate_y` over the long walls' outer faces, rising `slope` per metre
    // toward the ridge.
    let plate_y = eaves_y + house.plate_height_m;
    let underside = |x: f64| plate_y + slope * (half_w - x.abs());
    let ridge_y = underside(0.0);
    let frame = |u: DVec3| Frame {
        origin: DVec3::ZERO,
        u,
        v: DVec3::Y,
    };
    let walls = [
        (
            "east",
            frame(DVec3::NEG_Z),
            (-half_l, half_l),
            (inner_w, half_w),
        ),
        (
            "west",
            frame(DVec3::Z),
            (-half_l, half_l),
            (inner_w, half_w),
        ),
        (
            "north",
            frame(DVec3::X),
            (-inner_w, inner_w),
            (half_l - t, half_l),
        ),
        (
            "south",
            frame(DVec3::NEG_X),
            (-inner_w, inner_w),
            (half_l - t, half_l),
        ),
    ];
    let mut plans = Vec::new();
    for ((name, frame, u, depth), (_, openings)) in walls.into_iter().zip(house.walls.iter()) {
        let mut openings = openings.to_vec();
        openings.sort_by(|a, b| a.u.0.total_cmp(&b.u.0));
        let wall = Wall {
            name,
            frame,
            u,
            depth,
            openings,
            gable: matches!(name, "north" | "south"),
        };
        let gable = wall.gable.then(|| {
            vec![
                DVec2::new(-inner_w, eaves_y),
                DVec2::new(inner_w, eaves_y),
                DVec2::new(inner_w, underside(inner_w)),
                DVec2::new(0.0, ridge_y),
                DVec2::new(-inner_w, underside(inner_w)),
            ]
        });
        wall_elements(&wall, house, base_y, eaves_y, gable, &mut plans);
    }
    let timber = |piece| Cutter::Timber {
        boards: None,
        piece,
    };
    for (name, u) in [("east", DVec3::X), ("west", DVec3::NEG_X)] {
        plans.push(ElementPlan {
            name: format!("{name}/plate"),
            kind: ElementKind::WallPlate,
            material: Material::Wood,
            prism: Prism {
                frame: frame(u),
                profile: vec![
                    DVec2::new(inner_w, eaves_y),
                    DVec2::new(half_w, eaves_y),
                    DVec2::new(half_w, plate_y),
                    DVec2::new(inner_w, underside(inner_w)),
                ],
                near: -half_l,
                far: half_l,
            },
            cutter: timber(house.cuts.plate_piece_m),
        });
    }
    let slab = f64::from(house.boards_per_slope) * house.board_width_m;
    let thick = house.board_thickness_m;
    for (name, side) in [("east", 1.0), ("west", -1.0)] {
        plans.push(ElementPlan {
            name: format!("{name}/roof"),
            kind: ElementKind::Roof,
            material: Material::Wood,
            prism: Prism {
                frame: Frame {
                    origin: DVec3::new(0.0, ridge_y, 0.0),
                    u: DVec3::new(side * cos, -sin, 0.0),
                    v: DVec3::new(side * sin, cos, 0.0),
                },
                // Down the slope from the ridge line on the underside; the top corner at the ridge
                // is mitred onto the ridge's vertical plane, `s cos + t sin = 0`.
                profile: vec![
                    DVec2::new(0.0, 0.0),
                    DVec2::new(slab, 0.0),
                    DVec2::new(slab, thick),
                    DVec2::new(-thick * slope, thick),
                ],
                near: -half_l - house.gable_overhang_m,
                far: half_l + house.gable_overhang_m,
            },
            cutter: Cutter::Timber {
                boards: Some((DVec2::X, house.board_width_m, house.boards_per_slope)),
                piece: house.cuts.board_piece_m,
            },
        });
    }
    plans
}

/// One wall's piers, sills and lintels along its `u`, then its gable (`gable`, the pentagon's
/// profile) where it carries one.
fn wall_elements(
    wall: &Wall,
    house: &House,
    base_y: f64,
    eaves_y: f64,
    gable: Option<Vec<DVec2>>,
    plans: &mut Vec<ElementPlan>,
) {
    let course_y = |k: u32| base_y + f64::from(k) * house.course_m;
    let cutter = Cutter::Masonry {
        base_y,
        wall_start: wall.u.0,
        course: house.course_m,
        module: house.module_m,
    };
    let mut push = |kind: ElementKind, index: usize, profile: Vec<DVec2>| {
        let label = match kind {
            ElementKind::Pier => "pier",
            ElementKind::Sill => "sill",
            ElementKind::Lintel => "lintel",
            ElementKind::Gable => "gable",
            ElementKind::WallPlate | ElementKind::Roof => unreachable!("a wall is masonry"),
        };
        plans.push(ElementPlan {
            name: if kind == ElementKind::Gable {
                format!("{}/{label}", wall.name)
            } else {
                format!("{}/{label}_{index}", wall.name)
            },
            kind,
            material: Material::Brick,
            prism: Prism {
                frame: wall.frame,
                profile,
                near: wall.depth.0,
                far: wall.depth.1,
            },
            cutter: cutter.clone(),
        });
    };
    let mut from = wall.u.0;
    for (index, opening) in wall.openings.iter().enumerate() {
        push(
            ElementKind::Pier,
            index,
            rectangle(from, opening.u.0, base_y, eaves_y),
        );
        let (bottom, head) = opening.courses;
        if bottom > 0 {
            push(
                ElementKind::Sill,
                index,
                rectangle(opening.u.0, opening.u.1, base_y, course_y(bottom)),
            );
        }
        if head < house.eaves_course {
            push(
                ElementKind::Lintel,
                index,
                rectangle(opening.u.0, opening.u.1, course_y(head), eaves_y),
            );
        }
        from = opening.u.1;
    }
    push(
        ElementKind::Pier,
        wall.openings.len(),
        rectangle(from, wall.u.1, base_y, eaves_y),
    );
    if let Some(profile) = gable {
        push(ElementKind::Gable, 0, profile);
    }
}
