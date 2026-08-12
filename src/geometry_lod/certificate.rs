//! The tank build's certificate, read as data — five fields, nothing derivable (ADR-0035) — and
//! the projection every switch distance comes out of (ADR-0033 §9).
//!
//! NO METRE DISTANCES SHIP. The certificate carries a chain's bounding `radius_m` and its rungs'
//! certified `deviation_mm`; the metres a [`VisibilityRange`] is written with are derived here,
//! against the active [`ViewProfile`], and are recomputed when that profile moves.
//!
//! # The failure law is map loading's (ADR-0011)
//!
//! A missing certificate, a malformed one, or a trio member whose bytes do not reproduce the
//! sha256 the certificate recorded is a panic in every build, naming the file. The certificate is
//! the seam ADR-0033 §8 demanded: measurements paired with the wrong bytes are a claim about a
//! surface that is not on screen, and the pairing is exactly what a staged publish can interrupt.
//!
//! EACH SIDE FINGERPRINTS WHAT IT READS. The dedicated server opens only `<id>.sim.glb`, so it
//! verifies `sim_glb_sha`; a client opens both artifacts and verifies both. [`TrioMember`] is that
//! choice, made by the composition root rather than probed for.
//!
//! A CHAIN ABSENT FROM THE CERTIFICATE IS NOT A FAILURE — that primitive renders at source detail.
//! Coverage is the build's own (`mesh_count` is its tripwire), and a runtime refusal there would
//! turn "this mesh earned no rung" into a crash.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------------------------

/// `<id>.lod.json` — the whole certificate. Field-for-field the document `scripts/tank/trio.py`
/// writes, with unknown fields refused so a schema that grows arrives as a named panic instead of
/// as a silently ignored half.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Certificate {
    /// Staleness of the trio against the current `.blend` + `.tank.ron`. Recorded, not enforced
    /// here: the source is untracked by policy and absent from a shipped install.
    #[allow(dead_code)]
    pub(crate) blend_digest: String,
    /// sha256 of `<id>.glb`.
    pub(crate) view_glb_sha: String,
    /// sha256 of `<id>.sim.glb`.
    pub(crate) sim_glb_sha: String,
    /// Where rung mesh records begin in the view glb: mesh indices `[0, mesh_count)` are the
    /// scene's own meshes, `[mesh_count, ..)` are the rungs.
    pub(crate) mesh_count: usize,
    /// Per unique source primitive, keyed `<meshName>#<primitiveIndex>`.
    pub(crate) chains: BTreeMap<String, Chain>,
}

/// One source primitive's ladder.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Chain {
    /// The primitive's bounding radius about its own origin, metres — the conservative slack every
    /// switch distance carries (ADR-0033 §9).
    pub(crate) radius_m: f32,
    /// Ordered, deviations strictly ascending. A rung record carries NO material: a rung wears its
    /// source primitive's material at bind time.
    pub(crate) rungs: Vec<Rung>,
}

/// One rung: the mesh record's name in the view glb, and the certified upper bound on its worst
/// deviation from the level it replaces.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rung {
    pub(crate) mesh: String,
    pub(crate) deviation_mm: f32,
}

// ---------------------------------------------------------------------------------------------
// The trio on disk
// ---------------------------------------------------------------------------------------------

/// A shipped artifact the certificate fingerprints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TrioMember {
    /// `<id>.glb` — scene, textures, every rung. The render path's file.
    View,
    /// `<id>.sim.glb` — LOD0 geometry and material names, no textures, no UVs, no rungs. The
    /// ballistic walk's file, on the server and on the client alike.
    Sim,
}

impl TrioMember {
    /// The artifact's file name suffix under `assets/<id>/<id>`.
    pub(crate) const fn suffix(self) -> &'static str {
        match self {
            Self::View => ".glb",
            Self::Sim => ".sim.glb",
        }
    }

    /// The certificate field this member's bytes must reproduce.
    fn recorded_sha(self, certificate: &Certificate) -> &str {
        match self {
            Self::View => &certificate.view_glb_sha,
            Self::Sim => &certificate.sim_glb_sha,
        }
    }
}

/// `assets/<id>/<id>.lod.json`.
pub(crate) fn certificate_path(root: &Path, id: &str) -> PathBuf {
    root.join(id).join(format!("{id}.lod.json"))
}

/// `assets/<id>/<id><suffix>`.
pub(crate) fn member_path(root: &Path, id: &str, member: TrioMember) -> PathBuf {
    root.join(id).join(format!("{id}{}", member.suffix()))
}

/// Read and parse `<id>.lod.json`, or abort naming it.
pub(crate) fn load(root: &Path, id: &str) -> Certificate {
    let path = certificate_path(root, id);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "geometry_lod: the certificate {} could not be read ({err}). The trio `<id>.glb`, \
             `<id>.sim.glb`, `<id>.lod.json` ships together — rebuild it with \
             `python3 scripts/tank/build.py build assets/{id}/{id}.blend`",
            path.display(),
        )
    });
    parse(&text, &path)
}

/// The pure half of [`load`]: text in, certificate out, panic naming the file on anything else.
pub(crate) fn parse(text: &str, path: &Path) -> Certificate {
    let certificate: Certificate = serde_json::from_str(text).unwrap_or_else(|err| {
        panic!(
            "geometry_lod: the certificate {} is malformed ({err}). It carries exactly \
             blend_digest, view_glb_sha, sim_glb_sha, mesh_count and chains — rebuild the trio \
             rather than editing it",
            path.display(),
        )
    });
    validate(&certificate, path);
    certificate
}

/// The shape [`Chain::bands`] is only meaningful over: every number finite and positive, every
/// chain carrying at least one rung, and deviations STRICTLY ASCENDING.
///
/// Parsing is not enough. A document can deserialize, reproduce both recorded hashes and still be
/// nonsense — descending deviations produce a band that ends before it starts, a repeated deviation
/// produces an empty one, a negative radius pulls a switch inside the camera. Bevy reads a range as
/// `distance >= start && distance < end`, so those are not loud failures at the seam: they are a
/// level that never draws, or two that draw at once, discovered by eye. The certificate is required
/// data (ADR-0011), so a violated invariant here is a panic in every build.
fn validate(certificate: &Certificate, path: &Path) {
    let refuse = |what: String| -> ! {
        panic!(
            "geometry_lod: the certificate {} is malformed — {what}. Rebuild the trio rather than \
             editing it",
            path.display(),
        )
    };
    for (key, chain) in &certificate.chains {
        if !chain.radius_m.is_finite() || chain.radius_m <= 0.0 {
            refuse(format!(
                "chain `{key}` carries radius_m {} and a bounding radius is a positive length",
                chain.radius_m
            ));
        }
        if chain.rungs.is_empty() {
            refuse(format!(
                "chain `{key}` carries no rung; a chain with nothing under it is an ABSENT chain, \
                 which the certificate records by not naming the primitive at all"
            ));
        }
        let mut previous = 0.0_f32;
        for rung in &chain.rungs {
            if rung.mesh.is_empty() {
                refuse(format!("chain `{key}` carries a rung with no mesh name"));
            }
            if !rung.deviation_mm.is_finite() || rung.deviation_mm <= previous {
                refuse(format!(
                    "chain `{key}` rung `{}` carries deviation_mm {} against the {previous} before \
                     it; deviations are finite, positive and strictly ascending, or the bands they \
                     derive gap and overlap",
                    rung.mesh, rung.deviation_mm,
                ));
            }
            previous = rung.deviation_mm;
        }
    }
}

/// Fingerprint one trio member against the certificate, or abort naming both.
///
/// The whole file is hashed: the recorded sha is over the artifact's bytes as published, and a
/// partial read certifies a prefix.
pub(crate) fn verify_member(certificate: &Certificate, root: &Path, id: &str, member: TrioMember) {
    let path = member_path(root, id, member);
    let measured = sha256_file(&path).unwrap_or_else(|err| {
        panic!(
            "geometry_lod: {} could not be read ({err}), and the certificate {} names its \
             sha256 — the trio is incomplete",
            path.display(),
            certificate_path(root, id).display(),
        )
    });
    let recorded = member.recorded_sha(certificate);
    assert!(
        measured == recorded,
        "geometry_lod: {} hashes to {measured} and the certificate {} records {recorded}. The \
         certificate's measurements belong to other bytes than the ones on disk — rebuild the \
         trio with `python3 scripts/tank/build.py build assets/{id}/{id}.blend`",
        path.display(),
        certificate_path(root, id).display(),
    );
}

/// sha256 of a file, streamed — the artifacts run to tens of megabytes and never need to be
/// resident to be fingerprinted.
fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        }))
}

// ---------------------------------------------------------------------------------------------
// The projection
// ---------------------------------------------------------------------------------------------

/// The view a chain's switch distances are currently derived for: one perspective field, the pixel
/// height the main pass actually renders at, and the screen-space error budget.
///
/// ONE PROFILE, the most demanding active view (ADR-0035's YAGNI ledger). The game has a single
/// 3-D camera and the optic swaps its projection in place, so the live pair IS the demanding view.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewProfile {
    /// Vertical field of view, radians.
    pub(crate) vfov_rad: f32,
    /// Rendered height of the main pass, pixels (window physical height × render scale).
    pub(crate) height_px: f32,
    /// Screen-space error budget, pixels (`settings::LodPixelBudget`).
    pub(crate) budget_px: f32,
}

impl Default for ViewProfile {
    /// The pre-window guess: the narrowest authored field, a modest height, the budget's own
    /// default. Every term errs toward switching LATER than the live profile will.
    fn default() -> Self {
        Self {
            vfov_rad: crate::camera::GUNNER_FOV_FALLBACK,
            height_px: 1080.0,
            budget_px: 1.0,
        }
    }
}

impl ViewProfile {
    /// A profile from live inputs, all three treated as UNTRUSTED.
    ///
    /// `vfov_rad` is a DIVISOR: `NaN` produces `NaN` boundaries, which compare false against every
    /// distance and stop the geometry being drawn at all; a value outside `(0, π)` has no
    /// perspective half-angle. A non-positive height is an absent window rather than a small one,
    /// and a non-positive budget collapses every distance onto the bounding radius. Each falls
    /// back to the default profile's value, and the next frame with sane inputs corrects it.
    pub(crate) fn new(vfov_rad: f32, height_px: f32, budget_px: f32) -> Self {
        let default = Self::default();
        Self {
            vfov_rad: if vfov_rad > 0.0 && vfov_rad < core::f32::consts::PI {
                vfov_rad
            } else {
                default.vfov_rad
            },
            height_px: if height_px.is_finite() && height_px > 0.0 {
                height_px
            } else {
                default.height_px
            },
            budget_px: if budget_px.is_finite() && budget_px > 0.0 {
                budget_px
            } else {
                default.budget_px
            },
        }
    }

    /// The distance beyond which `deviation_mm` fits inside this profile's pixel budget, plus the
    /// chain's bounding radius as slack.
    ///
    /// ```text
    /// D = dev_m · height_px / (2 · tan(vfov / 2) · budget_px) + radius_m
    /// ```
    ///
    /// EXACT, not small-angle — the same expression as `scripts/lod/config.py::switch_distance_m`
    /// (ADR-0033 §9). The radius term is there because [`VisibilityRange`] measures camera-to-entity
    /// ORIGIN while the certified deviation lives on the SURFACE, which can be one bounding radius
    /// nearer the camera than the point the runtime tested.
    pub(crate) fn switch_distance_m(self, deviation_mm: f32, radius_m: f32) -> f32 {
        let budget_rad = 2.0 * (self.vfov_rad / 2.0).tan() * self.budget_px;
        (deviation_mm / 1000.0) * self.height_px / budget_rad + radius_m
    }
}

impl Chain {
    /// The complementary ranges this chain's levels own: `[0, s₁) [s₁, s₂) … [sₙ, ∞)`, one entry
    /// per level with rung 0 (the source primitive) first.
    ///
    /// Each range ends exactly where the next begins, and `VisibilityRange::is_visible_at_all` is
    /// half-open `[start, end)` (bevy_camera 0.19 `visibility/range.rs`), so every distance in
    /// `[0, ∞)` is owned by exactly one level — no gap, no double-draw.
    ///
    /// `abrupt`, so `use_aabb` stays false and the distance is measured to the entity ORIGIN, which
    /// is what [`ViewProfile::switch_distance_m`]'s radius slack is priced against. Abrupt rather
    /// than crossfaded: a dithered range compiles a second permutation of every pipeline it touches
    /// for a sub-budget transition.
    pub(crate) fn bands(&self, view: ViewProfile) -> Vec<VisibilityRange> {
        let starts: Vec<f32> = self
            .rungs
            .iter()
            .map(|rung| view.switch_distance_m(rung.deviation_mm, self.radius_m))
            .collect();
        (0..=starts.len())
            .map(|level| {
                let start = if level == 0 { 0.0 } else { starts[level - 1] };
                let end = starts.get(level).copied().unwrap_or(f32::INFINITY);
                VisibilityRange::abrupt(start, end)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped tank's id, and the only trio in the tree today.
    const TIGER: &str = "tiger_1";

    /// A synthetic trio on disk: two artifacts of arbitrary bytes and a certificate that names
    /// their real hashes. Enough to exercise the whole failure law without copying 68 MB.
    struct Trio {
        root: PathBuf,
    }

    impl Trio {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "overmatch-trio-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(root.join("t")).unwrap();
            let trio = Self { root };
            trio.write(TrioMember::View, b"view bytes");
            trio.write(TrioMember::Sim, b"sim bytes");
            trio.write_certificate(&trio.certificate_text());
            trio
        }

        fn write(&self, member: TrioMember, bytes: &[u8]) {
            std::fs::write(member_path(&self.root, "t", member), bytes).unwrap();
        }

        fn write_certificate(&self, text: &str) {
            std::fs::write(certificate_path(&self.root, "t"), text).unwrap();
        }

        fn certificate_text(&self) -> String {
            self.certificate_with(
                r#"{"radius_m": 0.5, "rungs": [{"mesh": "M#0_LOD1", "deviation_mm": 4.0}]}"#,
            )
        }

        /// The same coherent document around a chosen `M#0` chain body — the hashes stay right, so
        /// only the semantics are under test.
        fn certificate_with(&self, chain: &str) -> String {
            format!(
                r#"{{"blend_digest": "b", "view_glb_sha": "{}", "sim_glb_sha": "{}",
                    "mesh_count": 1, "chains": {{"M#0": {chain}}}}}"#,
                sha256_file(&member_path(&self.root, "t", TrioMember::View)).unwrap(),
                sha256_file(&member_path(&self.root, "t", TrioMember::Sim)).unwrap(),
            )
        }

        /// Load and verify both members, exactly as a client composition does.
        fn read(&self) -> Certificate {
            let certificate = load(&self.root, "t");
            verify_member(&certificate, &self.root, "t", TrioMember::View);
            verify_member(&certificate, &self.root, "t", TrioMember::Sim);
            certificate
        }
    }

    impl Drop for Trio {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    /// The baseline the mutants are measured against: a coherent trio loads, and the chain it
    /// names is the one it read.
    #[test]
    fn a_coherent_trio_loads() {
        let trio = Trio::new("coherent");
        let certificate = trio.read();
        assert_eq!(certificate.mesh_count, 1);
        assert_eq!(certificate.chains["M#0"].rungs[0].mesh, "M#0_LOD1");
    }

    /// A FLIPPED BYTE in the view artifact. The certificate's deviations were measured on other
    /// bytes than the ones that would render.
    #[test]
    #[should_panic(expected = "hashes to")]
    fn a_flipped_byte_in_the_view_artifact_aborts() {
        let trio = Trio::new("view-flip");
        trio.write(TrioMember::View, b"view byteS");
        trio.read();
    }

    /// A FLIPPED BYTE in the sim artifact — the file both sides walk.
    #[test]
    #[should_panic(expected = "hashes to")]
    fn a_flipped_byte_in_the_sim_artifact_aborts() {
        let trio = Trio::new("sim-flip");
        trio.write(TrioMember::Sim, b"sim byteS");
        trio.read();
    }

    /// A DELETED certificate. There is no code default for a chain set.
    #[test]
    #[should_panic(expected = "could not be read")]
    fn a_deleted_certificate_aborts() {
        let trio = Trio::new("no-cert");
        std::fs::remove_file(certificate_path(&trio.root, "t")).unwrap();
        trio.read();
    }

    /// A MALFORMED certificate — here, one field short of the five.
    #[test]
    #[should_panic(expected = "is malformed")]
    fn a_malformed_certificate_aborts() {
        let trio = Trio::new("malformed");
        trio.write_certificate(r#"{"blend_digest": "b", "mesh_count": 1}"#);
        trio.read();
    }

    /// A member the certificate names and the tree does not hold.
    #[test]
    #[should_panic(expected = "the trio is incomplete")]
    fn a_missing_artifact_aborts() {
        let trio = Trio::new("no-sim");
        std::fs::remove_file(member_path(&trio.root, "t", TrioMember::Sim)).unwrap();
        trio.read();
    }

    /// A CHAIN THAT PARSES AND MEANS NOTHING. Every one of these reproduces both recorded hashes
    /// and deserializes cleanly; what they violate is the shape [`Chain::bands`] is defined over.
    /// Descending or repeated deviations put a band's end before (or on) its start, which bevy
    /// reads as never-visible; a non-positive radius pulls a switch inside the camera. None of that
    /// is loud at the seam — it is a level that stops drawing, found by eye — so the loader is where
    /// it has to refuse, in every build.
    #[test]
    fn a_chain_whose_numbers_are_nonsense_aborts() {
        for (label, chain, expected) in [
            (
                "descending",
                r#"{"radius_m": 0.5, "rungs": [{"mesh": "a", "deviation_mm": 9.0},
                                               {"mesh": "b", "deviation_mm": 4.0}]}"#,
                "strictly ascending",
            ),
            (
                "repeated",
                r#"{"radius_m": 0.5, "rungs": [{"mesh": "a", "deviation_mm": 4.0},
                                               {"mesh": "b", "deviation_mm": 4.0}]}"#,
                "strictly ascending",
            ),
            (
                "negative deviation",
                r#"{"radius_m": 0.5, "rungs": [{"mesh": "a", "deviation_mm": -4.0}]}"#,
                "strictly ascending",
            ),
            (
                "negative radius",
                r#"{"radius_m": -0.5, "rungs": [{"mesh": "a", "deviation_mm": 4.0}]}"#,
                "positive length",
            ),
            (
                "zero radius",
                r#"{"radius_m": 0.0, "rungs": [{"mesh": "a", "deviation_mm": 4.0}]}"#,
                "positive length",
            ),
            (
                "no rung",
                r#"{"radius_m": 0.5, "rungs": []}"#,
                "carries no rung",
            ),
            (
                "unnamed rung",
                r#"{"radius_m": 0.5, "rungs": [{"mesh": "", "deviation_mm": 4.0}]}"#,
                "no mesh name",
            ),
        ] {
            let trio = Trio::new("nonsense");
            trio.write_certificate(&trio.certificate_with(chain));
            let refusal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| trio.read()))
                .expect_err(&format!("a {label} chain must be refused, not accepted"));
            let text = refusal
                .downcast_ref::<String>()
                .map_or_else(String::new, Clone::clone);
            assert!(
                text.contains("is malformed") && text.contains(expected),
                "a {label} chain must be refused NAMING what is wrong with it, got: {text}",
            );
        }
    }

    /// A CHAIN ABSENT FROM THE CERTIFICATE is silence, not a refusal — that primitive renders at
    /// source detail, and the build's own `mesh_count` owns coverage.
    #[test]
    fn a_primitive_with_no_chain_is_not_an_error() {
        let trio = Trio::new("absent-chain");
        let certificate = trio.read();
        assert!(!certificate.chains.contains_key("Unnamed#0"));
    }

    /// THE SHIPPED TRIO. The one that matters: the certificate in the tree describes the two
    /// artifacts in the tree, byte for byte, and parses into the shape the runtime reads.
    #[test]
    fn the_shipped_trio_is_coherent() {
        let root = crate::assets::asset_root();
        let certificate = load(&root, TIGER);
        verify_member(&certificate, &root, TIGER, TrioMember::View);
        verify_member(&certificate, &root, TIGER, TrioMember::Sim);
        assert!(
            !certificate.chains.is_empty(),
            "the shipped certificate names no chain at all",
        );
        for (key, chain) in &certificate.chains {
            assert!(
                chain.radius_m > 0.0,
                "{key} carries a non-positive bounding radius",
            );
            assert!(!chain.rungs.is_empty(), "{key} carries no rung");
            for pair in chain.rungs.windows(2) {
                assert!(
                    pair[1].deviation_mm > pair[0].deviation_mm,
                    "{key}: deviations must ascend strictly",
                );
            }
        }
    }

    /// THE PROJECTION, worked by hand against the certificate's own reference view.
    ///
    /// `0.027841808 m × 2160 / (2·tan(0.06)) + 0.380365 = 500.9314 m` — the same arithmetic
    /// `scripts/lod/config.py::switch_distance_m` applies, and the same probe the deleted
    /// hand-transcribed chain table the track view used to carry.
    #[test]
    fn the_switch_distance_is_the_exact_projection() {
        let view = ViewProfile::new(0.12, 2160.0, 1.0);
        let derived = view.switch_distance_m(27.841808, 0.380365);
        assert!(
            (derived - 500.9314).abs() < 0.01,
            "D = dev_m × height_px / (2·tan(vfov/2)·budget_px) + radius, got {derived}",
        );
        // ...and it is EXACT, not small-angle: the shortcut reads 501.4 m here, 0.06 % at the optic
        // and 5.5 % at the commander field (ADR-0033 §9).
        let small_angle = (27.841808 / 1000.0) / (0.12 / 2160.0) + 0.380365;
        assert!(
            (derived - small_angle).abs() > 0.1,
            "the small-angle shortcut must not be what is wired",
        );
    }

    /// A HALVED BUDGET doubles every distance; a HALVED HEIGHT halves the deviation term. Both
    /// terms are linear, and the radius slack rides outside them.
    #[test]
    fn the_budget_and_the_height_scale_the_deviation_term() {
        let base = ViewProfile::new(0.12, 2160.0, 1.0);
        let radius = 0.5;
        let term = base.switch_distance_m(10.0, radius) - radius;
        let halved_budget = ViewProfile::new(0.12, 2160.0, 0.5);
        assert!((halved_budget.switch_distance_m(10.0, radius) - radius - 2.0 * term).abs() < 1e-2);
        let halved_height = ViewProfile::new(0.12, 1080.0, 1.0);
        assert!((halved_height.switch_distance_m(10.0, radius) - radius - 0.5 * term).abs() < 1e-2);
    }

    /// UNTRUSTED INPUTS fall back rather than poisoning a divisor.
    #[test]
    fn a_hostile_view_falls_back_to_the_conservative_profile() {
        let default = ViewProfile::default();
        for (fov, height, budget) in [
            (f32::NAN, 1440.0, 1.0),
            (0.0, 1440.0, 1.0),
            (core::f32::consts::PI, 1440.0, 1.0),
            (0.12, 0.0, 1.0),
            (0.12, f32::INFINITY, 1.0),
            (0.12, 1440.0, -1.0),
        ] {
            let view = ViewProfile::new(fov, height, budget);
            assert!(view.vfov_rad > 0.0 && view.vfov_rad < core::f32::consts::PI);
            assert!(view.height_px.is_finite() && view.height_px > 0.0);
            assert!(view.budget_px.is_finite() && view.budget_px > 0.0);
            assert!(view.switch_distance_m(10.0, 0.5).is_finite());
        }
        assert_eq!(ViewProfile::new(f32::NAN, -1.0, f32::NAN), default);
    }

    /// THE BANDS TILE `[0, ∞)`: one level per distance, no gap and no double-draw, with the
    /// half-open `[start, end)` boundary belonging to the level below it.
    #[test]
    fn every_distance_is_owned_by_exactly_one_level() {
        let root = crate::assets::asset_root();
        let certificate = load(&root, TIGER);
        let view = ViewProfile::new(0.12, 2160.0, 1.0);
        for (key, chain) in &certificate.chains {
            let bands = chain.bands(view);
            assert_eq!(
                bands.len(),
                chain.rungs.len() + 1,
                "{key}: one band per level, rung 0 included",
            );
            let mut probes: Vec<f32> = vec![0.0, 1e-3];
            for band in &bands {
                let start = band.start_margin.start;
                if start.is_finite() {
                    probes.extend([start - 1e-2, start, start + 1e-2]);
                }
            }
            probes.push(1e6);
            for probe in probes.into_iter().filter(|d| *d >= 0.0) {
                let owners = bands
                    .iter()
                    .filter(|band| probe >= band.start_margin.start && probe < band.end_margin.end)
                    .count();
                assert_eq!(owners, 1, "{key}: {probe} m is owned by {owners} levels");
            }
        }
    }
}
