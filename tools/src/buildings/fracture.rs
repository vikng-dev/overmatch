//! The fracture bake's cut rule: every element into convex chunks of about 20 cm, which tile it.
//!
//! # Masonry: rows on bed joints, cuts that prefer head joints
//!
//! A wall is laid in running bond: courses `course_m` tall from the foundation plane, each course's
//! head joints every `module_m` along the wall from its first `u`, every other course offset by half
//! a module. The cut follows that bond:
//!
//! 1. **Rows.** Row boundaries lie on bed joints, every `row_courses` courses counted from the
//!    foundation in the whole wall, so rows line up across the wall's elements. An element's top
//!    row shorter than half a row (a gable's apex) joins the row below.
//! 2. **Cuts.** Each row's candidate cuts are the head joints of its bottom course, kept where the
//!    chunk on either side stays at least `min_width_m` wide at the row's mid-height. A
//!    row of three courses then cuts its top course on joints too and its middle course through
//!    half a brick, and the rows' cuts alternate by half a module, so the chunks themselves lie in
//!    a running bond.
//! 3. **Cracks.** Each candidate draws from a stream seeded by the asset, the element (whose name
//!    carries its wall) and its row and joint: with `crack_chance` it leaves the joint, each end
//!    moving up to `crack_shift_m`, as a straight crack through the bricks. A crack that would leave
//!    a chunk narrower than `min_width_m` stays on its joint.
//!
//! A chunk is the element's profile between two bed joints and two cut lines, extruded through the
//! wall: a convex prism, the intersection of half-planes.
//!
//! # Timber: boards, then pieces along the grain
//!
//! A roof plane is cut into its boards across the slope, then each board, and each wall plate, into
//! pieces along the grain whose lengths a stream seeded by the asset, the element and the board
//! draws within the spec's range.

use glam::DVec2;

use super::geom::{Prism, clip, span_at};
use super::house::{Cutter, ElementPlan};
use super::spec::Cuts;

/// A deterministic stream: splitmix64 over a seed folded from text by FNV-1a, so the same names
/// draw the same numbers on every platform.
pub struct Stream(u64);

impl Stream {
    /// The stream named by `parts`, each part's bytes then a separator folded in.
    pub fn new(parts: &[&str]) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for part in parts {
            for &byte in part.as_bytes().iter().chain(&[0xff]) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        Self(hash)
    }

    /// The next draw, uniform on `[0, 1)` with 53 bits.
    pub fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// How the masonry cuts fell: on head joints, or as cracks through bricks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CutCount {
    pub joints: usize,
    pub cracks: usize,
}

/// One cut through a row: its `u` at the row's bottom and at its top.
#[derive(Debug, Clone, Copy)]
struct Cut {
    bottom: f64,
    top: f64,
}

/// The chunks of `plan`, as prisms in its own frame, in row-major order (masonry: rows bottom-up,
/// then along `u`; timber: boards, then along the depth). Adds the masonry cuts to `count`.
pub fn chunks(plan: &ElementPlan, asset: &str, rule: &Cuts, count: &mut CutCount) -> Vec<Prism> {
    let prism = &plan.prism;
    let with = |profile: Vec<DVec2>, near: f64, far: f64| Prism {
        frame: prism.frame,
        profile,
        near,
        far,
    };
    match plan.cutter {
        Cutter::Masonry {
            base_y,
            wall_start,
            course,
            module,
        } => {
            let bond = Bond {
                base_y,
                wall_start,
                course,
                module,
            };
            masonry(plan, asset, rule, &bond, count)
                .into_iter()
                .map(|profile| with(profile, prism.near, prism.far))
                .collect()
        }
        Cutter::Timber { boards, piece } => {
            let profiles = match boards {
                None => vec![prism.profile.clone()],
                Some((axis, width, n)) => (0..n)
                    .map(|i| {
                        let mut profile = prism.profile.clone();
                        if i > 0 {
                            profile = clip(&profile, -axis, -f64::from(i) * width);
                        }
                        if i + 1 < n {
                            profile = clip(&profile, axis, f64::from(i + 1) * width);
                        }
                        profile
                    })
                    .collect(),
            };
            let mut out = Vec::new();
            for (i, profile) in profiles.into_iter().enumerate() {
                let mut stream = Stream::new(&[asset, &plan.name, "board", &i.to_string()]);
                for (near, far) in pieces(prism.near, prism.far, piece, &mut stream) {
                    out.push(with(profile.clone(), near, far));
                }
            }
            out
        }
    }
}

/// `[near, far]` split into pieces drawn within `(shortest, longest)`: the last piece takes what
/// is left once it fits, and a draw that would leave less than `shortest` splits the rest in two.
fn pieces(
    near: f64,
    far: f64,
    (shortest, longest): (f64, f64),
    stream: &mut Stream,
) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut at = near;
    loop {
        let rest = far - at;
        if rest <= longest {
            out.push((at, far));
            return out;
        }
        let mut length = shortest + (longest - shortest) * stream.unit();
        if rest - length < shortest {
            length = 0.5 * rest;
        }
        out.push((at, at + length));
        at += length;
    }
}

/// The masonry chunks' profiles of `plan` (module doc).
fn masonry(
    plan: &ElementPlan,
    asset: &str,
    rule: &Cuts,
    bond: &Bond,
    count: &mut CutCount,
) -> Vec<Vec<DVec2>> {
    let profile = &plan.prism.profile;
    let Bond {
        base_y,
        wall_start,
        course,
        module,
    } = *bond;
    let (y_min, y_max) = profile
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p.y), hi.max(p.y))
        });
    let row = f64::from(rule.row_courses) * course;
    let eps = 1.0e-9;
    let mut bounds = vec![y_min];
    let first = ((y_min - base_y) / row).floor() as i64 + 1;
    let mut m = first;
    loop {
        let y = base_y + m as f64 * row;
        if y >= y_max - eps {
            break;
        }
        if y > y_min + eps {
            bounds.push(y);
        }
        m += 1;
    }
    bounds.push(y_max);
    if bounds.len() > 2 && y_max - bounds[bounds.len() - 2] < 0.5 * row {
        bounds.remove(bounds.len() - 2);
    }
    let mut out = Vec::new();
    for (r, pair) in bounds.windows(2).enumerate() {
        let (y0, y1) = (pair[0], pair[1]);
        let band = clip(&clip(profile, DVec2::NEG_Y, -y0), DVec2::Y, y1);
        let Some((lo, hi)) = span_at(profile, 0.5 * (y0 + y1)) else {
            continue;
        };
        let (lo, hi) = (lo + rule.min_width_m, hi - rule.min_width_m);
        let parity = (((y0 - base_y) / course).round() as i64).rem_euclid(2) as f64;
        let mut cuts: Vec<Cut> = Vec::new();
        let first = ((lo - wall_start) / module - 0.5 * parity).ceil() as i64;
        let last = ((hi - wall_start) / module - 0.5 * parity).floor() as i64;
        for j in first..=last {
            let joint = wall_start + (j as f64 + 0.5 * parity) * module;
            let mut stream = Stream::new(&[
                asset,
                &plan.name,
                "row",
                &r.to_string(),
                "joint",
                &j.to_string(),
            ]);
            let fits = |cut: Cut, cuts: &[Cut]| {
                let clear = cuts.last().is_none_or(|before| {
                    cut.bottom - before.bottom >= rule.min_width_m
                        && cut.top - before.top >= rule.min_width_m
                });
                clear && [cut.bottom, cut.top].iter().all(|&u| u >= lo && u <= hi)
            };
            let seam = Cut {
                bottom: joint,
                top: joint,
            };
            let cut = if stream.unit() < rule.crack_chance {
                let shift = |stream: &mut Stream| (2.0 * stream.unit() - 1.0) * rule.crack_shift_m;
                let crack = Cut {
                    bottom: joint + shift(&mut stream),
                    top: joint + shift(&mut stream),
                };
                fits(crack, &cuts).then_some(crack)
            } else {
                None
            };
            match cut {
                Some(crack) => {
                    count.cracks += 1;
                    cuts.push(crack);
                }
                None if fits(seam, &cuts) => {
                    count.joints += 1;
                    cuts.push(seam);
                }
                None => {}
            }
        }
        // Each cut line runs from (bottom, y0) to (top, y1); its left side is `n · p ≤ n · B`.
        let side = |cut: &Cut| {
            let (b, t) = (DVec2::new(cut.bottom, y0), DVec2::new(cut.top, y1));
            let d = t - b;
            let n = DVec2::new(d.y, -d.x);
            (n, n.dot(b))
        };
        for k in 0..=cuts.len() {
            let mut piece = band.clone();
            if k > 0 {
                let (n, c) = side(&cuts[k - 1]);
                piece = clip(&piece, -n, -c);
            }
            if k < cuts.len() {
                let (n, c) = side(&cuts[k]);
                piece = clip(&piece, n, c);
            }
            if !piece.is_empty() {
                out.push(piece);
            }
        }
    }
    out
}

/// A wall's running bond ([`Cutter::Masonry`]).
#[derive(Clone, Copy)]
struct Bond {
    base_y: f64,
    wall_start: f64,
    course: f64,
    module: f64,
}
