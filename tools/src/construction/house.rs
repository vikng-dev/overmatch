//! The house generator: a [`House`] spec to its [`Construction`], whose pieces are the split
//! tree's terminals (CGA's mass → walls → piers, openings, lintels, sills; Müller et al. 2006).
//! Each piece is a right prism carrying its part's material, surface frame and cut; the generator
//! supplies only a bond's frame: its datum and axes.
//!
//! # The house's frame and parts
//!
//! The construction frame of [`overmatch::construction::schema`]: `+Y` up, the origin on the ground point,
//! the ridge along `Z`. The two long walls (east at `+X`, west at `−X`) run the full length; the
//! two gable walls (north at `+Z`, south at `−Z`) stand between them. Every wall rises from the
//! foundation plane to the eaves and is split by its openings into piers (full height), sills
//! (under an opening) and lintels (over it). Above the eaves each gable wall carries its gable, a
//! pentagon whose sloped top is the roof's underside. A timber wall plate lies along each long
//! wall's top, its own top bevelled to the roof's slope, and each roof plane is its boards laid
//! side by side down the slope from the ridge, on the plates and the gables, the top board mitred
//! where the two planes meet. Each board is its own piece and its own component: its cut never
//! crosses into the next, and only the joins hold it to the next board, the plates and the gables;
//! a board's own seam holds only the cuts inside it. The generator lays the parts out; how each is
//! held to another it touches is the spec's ([`super::spec::Join`]).
//!
//! Every slope is the spec's pitch as a ratio (rise over run, a Pythagorean pair's hypotenuse taken
//! by square root), so no angle and no trigonometry enters the bake.

use glam::{DVec2, DVec3};

use super::fracture::{Courses, Pattern};
use super::geom::{Frame, Prism};
use super::spec::{self, Construction, House, Opening, Part, PartCut, Piece};

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
/// component's first `u` and lowest `v`), its axes the frame's `u` and `v`.
fn pattern(house: &House, part: &Part, frame: Frame, datum: DVec2) -> Pattern {
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
    }
}

/// One element of `part`.
fn piece(
    house: &House,
    part: &Part,
    (name, label, component): (String, &str, String),
    prism: &Prism,
    datum: DVec2,
) -> Piece {
    Piece {
        name,
        part: label.to_owned(),
        component,
        material: part.material.clone(),
        surface_frame: part.surface_frame,
        solid: prism.solid(),
        pattern: pattern(house, part, prism.frame, datum),
    }
}

/// The lowest `(u, v)` of `profile`: a lone element's datum.
fn lowest(profile: &[DVec2]) -> DVec2 {
    profile.iter().fold(DVec2::INFINITY, |lo, &p| lo.min(p))
}

/// `house` as a construction. Its pieces in emission order: the walls (east, west, north, south;
/// along each wall's `u`, a pier and then each opening's sill and lintel, a gable last), the wall
/// plates, the roof planes' boards (east, west; each from the ridge down). Refuses what
/// [`spec::check_disjoint`] refuses.
pub fn construction(house: &House) -> Result<Construction, String> {
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
            "wall_plate",
            format!("{name}/plate"),
        );
        let datum = lowest(&prism.profile);
        pieces.push(piece(house, &house.parts.plates, names, &prism, datum));
    }
    let (width, thick) = (house.board_width_m, house.board_thickness_m);
    for (name, side) in [("east", 1.0), ("west", -1.0)] {
        let plane = Frame {
            origin: DVec3::new(0.0, ridge_y, 0.0),
            u: DVec3::new(side * cos, -sin, 0.0),
            v: DVec3::new(side * sin, cos, 0.0),
        };
        // Board `board` spans `[s0, s1]` down the slope from the ridge line on the underside; the
        // top board's top corner is mitred onto the ridge's vertical plane, `s cos + t sin = 0`.
        for board in 0..house.boards_per_slope {
            let (s0, s1) = (f64::from(board) * width, f64::from(board + 1) * width);
            let top = if board == 0 { -thick * slope } else { s0 };
            let profile = vec![
                DVec2::new(s0, 0.0),
                DVec2::new(s1, 0.0),
                DVec2::new(s1, thick),
                DVec2::new(top, thick),
            ];
            let datum = lowest(&profile);
            let prism = Prism {
                frame: plane,
                profile,
                near: -half_l - house.gable_overhang_m,
                far: half_l + house.gable_overhang_m,
            };
            let board = format!("{name}/board_{board}");
            let names = (board.clone(), "roof", board);
            pieces.push(piece(house, &house.parts.roof, names, &prism, datum));
        }
    }
    spec::check_disjoint(&pieces)?;
    Ok(Construction {
        id: house.id.clone(),
        foundation_y: base_y,
        foundation_attachment: house.foundation_attachment.clone(),
        pieces,
        joins: house.joins.clone(),
    })
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
    let mut push = |part: &str, index: usize, profile: Vec<DVec2>| {
        let name = if part == "gable" {
            format!("{}/{part}", wall.name)
        } else {
            format!("{}/{part}_{index}", wall.name)
        };
        let prism = Prism {
            frame: wall.frame,
            profile,
            near: wall.depth.0,
            far: wall.depth.1,
        };
        let names = (name, part, wall.name.to_owned());
        pieces.push(piece(house, &house.parts.walls, names, &prism, datum));
    };
    let mut from = wall.u.0;
    for (index, opening) in wall.openings.iter().enumerate() {
        push("pier", index, rectangle(from, opening.u.0, base_y, eaves_y));
        let (bottom, head) = opening.courses;
        if bottom > 0 {
            push(
                "sill",
                index,
                rectangle(opening.u.0, opening.u.1, base_y, course_y(bottom)),
            );
        }
        if head < house.eaves_course {
            push(
                "lintel",
                index,
                rectangle(opening.u.0, opening.u.1, course_y(head), eaves_y),
            );
        }
        from = opening.u.1;
    }
    push(
        "pier",
        wall.openings.len(),
        rectangle(from, wall.u.1, base_y, eaves_y),
    );
    if let Some(profile) = gable {
        push("gable", 0, profile);
    }
}
