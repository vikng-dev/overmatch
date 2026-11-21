//! The house generator: a [`House`] spec to its [`Construction`], whose pieces are the split
//! tree's terminals (CGA's mass → walls → piers, openings, lintels, sills; Müller et al. 2006).
//! Each piece is a right prism carrying its part's material, surface frame and cut; the generator
//! supplies only a cut's depth axis.
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
//! Interior cross walls stand between the long walls, each carrying a gable as the gable walls do.
//! With a roof frame the boards lie on rafters instead: a rafter pair over each gable-carrying wall's
//! centre line and at even centres between, each rafter bearing on its wall plate, on the ridge beam
//! and, over a wall, on its gable, the pair meeting plumb at the ridge; the ridge beam, one segment
//! per bay between gable-carrying walls, sits in a notch at each gable's apex, which splits the gable
//! into three convex elements: the masonry under the beam's seat and a wedge beside each of its
//! faces.
//!
//! Every slope is the spec's pitch as a ratio (rise over run, a Pythagorean pair's hypotenuse taken
//! by square root), so no angle and no trigonometry enters the bake.

use glam::{DVec2, DVec3};
use overmatch::construction::schema::SurfaceFrame;

use super::fracture::{Pattern, Planar};
use super::geom::{Frame, Prism};
use super::spec::{self, Construction, House, Opening, Part, PartCut, Piece, RoofFrame};

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
struct Wall<'a> {
    name: &'a str,
    frame: Frame,
    u: (f64, f64),
    depth: (f64, f64),
    openings: Vec<Opening>,
    gable: bool,
}

/// One element of `part`, its planar cut running through the prism's depth.
fn piece(part: &Part, (name, label, component): (String, &str, String), prism: &Prism) -> Piece {
    Piece {
        name,
        part: label.to_owned(),
        component,
        material: part.material.clone(),
        surface_frame: part.surface_frame,
        solid: prism.solid(),
        pattern: match &part.cut {
            PartCut::Whole => Pattern::Whole,
            PartCut::Planar(rule) => Pattern::Planar(Planar {
                depth: prism.frame.w(),
                rule: rule.clone(),
            }),
            PartCut::Cells(cells) => Pattern::Cells(cells.clone()),
        },
    }
}

/// `house` as a construction. Its pieces in emission order: the walls (east, west, north, south,
/// then the cross walls; along each wall's `u`, a pier and then each opening's sill and lintel, its
/// gable's elements last), the wall plates, the roof frame's members, the roof planes' boards
/// (east, west; each from the ridge down). Refuses what [`spec::check_disjoint`] refuses.
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
    // A gable's profile in its wall's `u`, split around the ridge beam's seat when framed.
    let gable = || match &house.roof_frame {
        None => vec![vec![
            DVec2::new(-inner_w, eaves_y),
            DVec2::new(inner_w, eaves_y),
            DVec2::new(inner_w, underside(inner_w)),
            DVec2::new(0.0, ridge_y),
            DVec2::new(-inner_w, underside(inner_w)),
        ]],
        Some(f) => {
            let r = f.ridge_width_m / 2.0;
            let seat = underside(r) - f.ridge_depth_m;
            // Where the roof's underside passes the seat's height.
            let reach = half_w - (seat - plate_y) / slope;
            vec![
                vec![
                    DVec2::new(-inner_w, eaves_y),
                    DVec2::new(inner_w, eaves_y),
                    DVec2::new(inner_w, underside(inner_w)),
                    DVec2::new(reach, seat),
                    DVec2::new(-reach, seat),
                    DVec2::new(-inner_w, underside(inner_w)),
                ],
                vec![
                    DVec2::new(-reach, seat),
                    DVec2::new(-r, seat),
                    DVec2::new(-r, underside(r)),
                ],
                vec![
                    DVec2::new(r, seat),
                    DVec2::new(reach, seat),
                    DVec2::new(r, underside(r)),
                ],
            ]
        }
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
        let gables = if wall.gable { gable() } else { Vec::new() };
        wall_elements(&wall, house, base_y, eaves_y, gables, &mut pieces);
    }
    for cross in &house.cross_walls {
        let mut openings = cross.openings.clone();
        openings.sort_by(|a, b| a.u.0.total_cmp(&b.u.0));
        let wall = Wall {
            name: &cross.name,
            frame: frame(DVec3::X),
            u: (-inner_w, inner_w),
            depth: (
                cross.at_z_m - cross.thickness_m / 2.0,
                cross.at_z_m + cross.thickness_m / 2.0,
            ),
            openings,
            gable: true,
        };
        wall_elements(&wall, house, base_y, eaves_y, gable(), &mut pieces);
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
        pieces.push(piece(&house.parts.plates, names, &prism));
    }
    // The boards' underside lies the rafters' depth above the roof's underside.
    let lift = house.roof_frame.as_ref().map_or(0.0, |f| f.rafter_depth_m);
    if let Some(f) = &house.roof_frame {
        let walls = [-half_l + t / 2.0, half_l - t / 2.0]
            .into_iter()
            .chain(house.cross_walls.iter().map(|w| w.at_z_m));
        framing(
            house,
            f,
            walls.collect(),
            underside,
            (ridge_y, cos, sin),
            &mut pieces,
        );
    }
    let (width, thick) = (house.board_width_m, house.board_thickness_m);
    for (name, side) in [("east", 1.0), ("west", -1.0)] {
        let plane = Frame {
            origin: DVec3::new(0.0, ridge_y, 0.0),
            u: DVec3::new(side * cos, -sin, 0.0),
            v: DVec3::new(side * sin, cos, 0.0),
        };
        // Board `board` spans `[s0, s1]` down the slope on its underside, `lift` above the roof's
        // underside, from the ridge's vertical plane, `s cos + t sin = 0`, where the top board's
        // corners are mitred.
        let start = -lift * slope;
        for board in 0..house.boards_per_slope {
            let (s0, s1) = (
                start + f64::from(board) * width,
                start + f64::from(board + 1) * width,
            );
            let top = if board == 0 {
                -(lift + thick) * slope
            } else {
                s0
            };
            let profile = vec![
                DVec2::new(s0, lift),
                DVec2::new(s1, lift),
                DVec2::new(s1, lift + thick),
                DVec2::new(top, lift + thick),
            ];
            let prism = Prism {
                frame: plane,
                profile,
                near: -half_l - house.gable_overhang_m,
                far: half_l + house.gable_overhang_m,
            };
            let board = format!("{name}/board_{board}");
            let names = (board.clone(), "roof", board);
            pieces.push(piece(&house.parts.roof, names, &prism));
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

/// The roof frame's members ([`RoofFrame`]): a rafter pair at each of `walls`' centre `z` and at
/// even centres between, then each bay's ridge segment. `underside` is the roof's underside height
/// at `x`, `ridge_y` its apex, `cos` and `sin` the slope's.
fn framing(
    house: &House,
    f: &RoofFrame,
    mut walls: Vec<f64>,
    underside: impl Fn(f64) -> f64,
    (ridge_y, cos, sin): (f64, f64, f64),
    pieces: &mut Vec<Piece>,
) {
    walls.sort_by(f64::total_cmp);
    let (half_w, half_l) = (0.5 * house.width_m, 0.5 * house.length_m);
    let across = Frame {
        origin: DVec3::ZERO,
        u: DVec3::X,
        v: DVec3::Y,
    };
    // A member's grain runs along its length.
    let mut push = |name: String, label: &str, length: DVec3, prism: Prism| {
        let part = Part {
            material: f.material.clone(),
            surface_frame: SurfaceFrame::Grain(length.as_vec3().to_array()),
            cut: f.cut.clone(),
        };
        pieces.push(piece(&part, (name.clone(), label, name), &prism));
    };
    let mut pairs = Vec::new();
    for (k, &z) in walls.iter().enumerate() {
        pairs.push(z);
        if let Some(&next) = walls.get(k + 1) {
            let bays = ((next - z) / f.rafter_spacing_m).ceil() as usize;
            pairs.extend((1..bays).map(|i| z + (next - z) * i as f64 / bays as f64));
        }
    }
    // Each rafter runs from the ridge's plumb plane to its plumb cut past the eaves; `h` is its
    // depth measured plumb.
    let h = f.rafter_depth_m / cos;
    let eaves = half_w + f.eaves_overhang_m;
    let (low, high) = (underside(eaves), underside(eaves) + h);
    for (k, &z) in pairs.iter().enumerate() {
        for (side, x) in [("east", 1.0), ("west", -1.0)] {
            let mut profile = vec![
                DVec2::new(0.0, ridge_y),
                DVec2::new(x * eaves, low),
                DVec2::new(x * eaves, high),
                DVec2::new(0.0, ridge_y + h),
            ];
            if x < 0.0 {
                profile.reverse(); // Counter-clockwise.
            }
            let prism = Prism {
                frame: across,
                profile,
                near: z - f.rafter_width_m / 2.0,
                far: z + f.rafter_width_m / 2.0,
            };
            push(
                format!("{side}/rafter_{k}"),
                "rafter",
                DVec3::new(x * cos, -sin, 0.0),
                prism,
            );
        }
    }
    // One ridge segment per bay: its ends on the gable walls' outer faces and the cross walls'
    // centre lines.
    let r = f.ridge_width_m / 2.0;
    let seat = underside(r) - f.ridge_depth_m;
    let mut ends = walls;
    *ends.first_mut().expect("two gable walls") = -half_l;
    *ends.last_mut().expect("two gable walls") = half_l;
    for (k, span) in ends.windows(2).enumerate() {
        let prism = Prism {
            frame: across,
            profile: vec![
                DVec2::new(-r, seat),
                DVec2::new(r, seat),
                DVec2::new(r, underside(r)),
                DVec2::new(0.0, ridge_y),
                DVec2::new(-r, underside(r)),
            ],
            near: span[0],
            far: span[1],
        };
        push(format!("ridge_{k}"), "ridge", DVec3::Z, prism);
    }
}

/// One wall's piers, sills and lintels along its `u`, then its gable's elements (`gables`, convex
/// profiles) where it carries one.
fn wall_elements(
    wall: &Wall,
    house: &House,
    base_y: f64,
    eaves_y: f64,
    gables: Vec<Vec<DVec2>>,
    pieces: &mut Vec<Piece>,
) {
    let course_y = |k: u32| base_y + f64::from(k) * house.course_m;
    let lone = gables.len() == 1;
    let mut push = |part: &str, index: usize, profile: Vec<DVec2>| {
        let name = if part == "gable" && lone {
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
        pieces.push(piece(&house.parts.walls, names, &prism));
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
    for (index, profile) in gables.into_iter().enumerate() {
        push("gable", index, profile);
    }
}
