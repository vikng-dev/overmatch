//! The house generator: a [`House`] spec to its [`Construction`], whose pieces are the split
//! tree's terminals (CGA's mass → walls → piers, openings, lintels, sills; Müller et al. 2006).
//! Each piece is a right prism carrying its part's material, surface frame and cut; the generator
//! supplies only each cut's frame: a bond's datum and axes, a roof plane's board layout.
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
//! on the plates and the gables, mitred where the two meet at the ridge. The generator lays the
//! parts out; how each is held to another it touches is the spec's ([`super::spec::Join`]).
//!
//! Every slope is the spec's pitch as a ratio (rise over run, a Pythagorean pair's hypotenuse taken
//! by square root), so no angle and no trigonometry enters the bake.

use glam::{DVec2, DVec3};
use overmatch::buildings::schema::ElementKind;

use super::fracture::{Across, Boards, Courses, Pattern};
use super::geom::{Frame, Prism};
use super::spec::{Construction, House, Opening, Part, PartCut, Piece};

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

/// `part`'s cut of an element in `frame`: a bond's datum is the frame's point `datum` (its
/// component's first `u` and lowest `v`), its axes the frame's `u` and `v`; board pieces run along
/// the frame's `w`, across `boards` where the element is laid in boards.
fn pattern(
    house: &House,
    part: &Part,
    frame: Frame,
    datum: DVec2,
    boards: Option<Across>,
) -> Pattern {
    match &part.cut {
        PartCut::Whole => Pattern::Whole,
        PartCut::Courses(rule) => Pattern::Courses(Courses {
            origin: frame.point(datum, 0.0),
            along: frame.u,
            up: frame.v,
            course_m: house.course_m,
            module_m: house.module_m,
            rule: rule.clone(),
        }),
        PartCut::Cells(cells) => Pattern::Cells(cells.clone()),
        PartCut::Boards(range) => Pattern::Boards(Boards {
            across: boards,
            along: frame.w(),
            piece_m: range.0,
        }),
    }
}

/// One element of `part`.
fn piece(
    house: &House,
    part: &Part,
    (name, kind, component): (String, ElementKind, String),
    prism: &Prism,
    datum: DVec2,
    boards: Option<Across>,
) -> Piece {
    Piece {
        name,
        kind,
        component,
        material: part.material.clone(),
        surface_frame: part.surface_frame,
        solid: prism.solid(),
        pattern: pattern(house, part, prism.frame, datum, boards),
    }
}

/// The lowest `(u, v)` of `profile`: a lone element's datum.
fn lowest(profile: &[DVec2]) -> DVec2 {
    profile.iter().fold(DVec2::INFINITY, |lo, &p| lo.min(p))
}

/// `house` as a construction. Its pieces in emission order: the walls (east, west, north, south;
/// along each wall's `u`, a pier and then each opening's sill and lintel, a gable last), the wall
/// plates, the roof planes.
pub fn construction(house: &House) -> Construction {
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
            house.wall_span("east"),
            (inner_w, half_w),
        ),
        (
            "west",
            frame(DVec3::Z),
            house.wall_span("west"),
            (inner_w, half_w),
        ),
        (
            "north",
            frame(DVec3::X),
            house.wall_span("north"),
            (half_l - t, half_l),
        ),
        (
            "south",
            frame(DVec3::NEG_X),
            house.wall_span("south"),
            (half_l - t, half_l),
        ),
    ];
    let mut pieces = Vec::new();
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
        wall_elements(&wall, house, base_y, eaves_y, gable, &mut pieces);
    }
    for (name, u) in [("east", DVec3::X), ("west", DVec3::NEG_X)] {
        let prism = Prism {
            frame: frame(u),
            profile: vec![
                DVec2::new(inner_w, eaves_y),
                DVec2::new(half_w, eaves_y),
                DVec2::new(half_w, plate_y),
                DVec2::new(inner_w, underside(inner_w)),
            ],
            near: -half_l,
            far: half_l,
        };
        let names = (
            format!("{name}/plate"),
            ElementKind::WallPlate,
            format!("{name}/plate"),
        );
        let datum = lowest(&prism.profile);
        pieces.push(piece(
            house,
            &house.parts.plates,
            names,
            &prism,
            datum,
            None,
        ));
    }
    let slab = f64::from(house.boards_per_slope) * house.board_width_m;
    let thick = house.board_thickness_m;
    for (name, side) in [("east", 1.0), ("west", -1.0)] {
        let prism = Prism {
            frame: Frame {
                origin: DVec3::new(0.0, ridge_y, 0.0),
                u: DVec3::new(side * cos, -sin, 0.0),
                v: DVec3::new(side * sin, cos, 0.0),
            },
            // Down the slope from the ridge line on the underside; the top corner at the ridge is
            // mitred onto the ridge's vertical plane, `s cos + t sin = 0`.
            profile: vec![
                DVec2::new(0.0, 0.0),
                DVec2::new(slab, 0.0),
                DVec2::new(slab, thick),
                DVec2::new(-thick * slope, thick),
            ],
            near: -half_l - house.gable_overhang_m,
            far: half_l + house.gable_overhang_m,
        };
        // The boards lie side by side down the slope, from the ridge line.
        let boards = Across {
            normal: prism.frame.u,
            start: prism.frame.u.dot(prism.frame.origin),
            width: house.board_width_m,
            count: house.boards_per_slope,
        };
        let names = (
            format!("{name}/roof"),
            ElementKind::Roof,
            format!("{name}/roof"),
        );
        let datum = lowest(&prism.profile);
        pieces.push(piece(
            house,
            &house.parts.roof,
            names,
            &prism,
            datum,
            Some(boards),
        ));
    }
    Construction {
        id: house.id.clone(),
        foundation_y: base_y,
        foundation_attachment: house.foundation_attachment.clone(),
        pieces,
        joins: house.joins.clone(),
    }
}

/// One wall's piers, sills and lintels along its `u`, then its gable (`gable`, the pentagon's
/// profile) where it carries one.
fn wall_elements(
    wall: &Wall,
    house: &House,
    base_y: f64,
    eaves_y: f64,
    gable: Option<Vec<DVec2>>,
    pieces: &mut Vec<Piece>,
) {
    let course_y = |k: u32| base_y + f64::from(k) * house.course_m;
    // A wall's piers, sills, lintels and gable are one component, coursed in one bond from the
    // wall's first `u` on the foundation.
    let datum = DVec2::new(wall.u.0, base_y);
    let mut push = |kind: ElementKind, index: usize, profile: Vec<DVec2>| {
        let label = match kind {
            ElementKind::Pier => "pier",
            ElementKind::Sill => "sill",
            ElementKind::Lintel => "lintel",
            ElementKind::Gable => "gable",
            ElementKind::WallPlate | ElementKind::Roof | ElementKind::Trunk => {
                unreachable!("a wall lays no {kind:?}")
            }
        };
        let name = if kind == ElementKind::Gable {
            format!("{}/{label}", wall.name)
        } else {
            format!("{}/{label}_{index}", wall.name)
        };
        let prism = Prism {
            frame: wall.frame,
            profile,
            near: wall.depth.0,
            far: wall.depth.1,
        };
        let names = (name, kind, wall.name.to_owned());
        pieces.push(piece(house, &house.parts.walls, names, &prism, datum, None));
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
