//! Dependency-graph determinism tripwires.
//!
//! THE MECHANISM THIS GUARDS: the deterministic belt sim runs the same f32 arithmetic on every
//! peer, and that only holds while every math crate in the sim's float path compiles to SCALAR
//! IEEE operations. glam's SIMD backends re-associate dot products (4-lane horizontal adds), and
//! parry's raycasts ride on glam through its math shim — parry ≤ 0.29's raycasts were measurably
//! not cross-platform reproducible for exactly this reason (SIMD dot-product non-associativity;
//! parry 0.29 changelog). Our fix is the `scalar-math` feature pinned on glam in Cargo.toml —
//! but cargo features only UNIFY within one package instance: if a dependency bump ever splits
//! glam into two versions, the instance parry/bevy compile against silently stops receiving our
//! scalar pin, and the sim desyncs across platforms with no compile error at all. These tests
//! make that split loud.

use std::path::Path;

/// One `[[package]]` block from Cargo.lock.
struct Package {
    name: String,
    version: String,
    dependencies: Vec<String>,
}

/// Parse Cargo.lock (lock format 4): `[[package]]` blocks with `name`, `version`, and an
/// optional `dependencies = [ ... ]` list whose entries are `"name"` or `"name version"` (the
/// qualified form appears exactly when multiple versions of `name` exist in the graph).
fn parse_lock(lock: &str) -> Vec<Package> {
    let mut packages = Vec::new();
    let mut current: Option<Package> = None;
    let mut in_deps = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            if let Some(package) = current.take() {
                packages.push(package);
            }
            current = Some(Package {
                name: String::new(),
                version: String::new(),
                dependencies: Vec::new(),
            });
            in_deps = false;
            continue;
        }
        let Some(package) = current.as_mut() else {
            continue;
        };
        if in_deps {
            if line.starts_with(']') {
                in_deps = false;
            } else if let Some(entry) = line.split('"').nth(1) {
                package.dependencies.push(entry.to_string());
            }
            continue;
        }
        if let Some(name) = line.strip_prefix("name = \"") {
            package.name = name.trim_end_matches('"').to_string();
        } else if let Some(version) = line.strip_prefix("version = \"") {
            package.version = version.trim_end_matches('"').to_string();
        } else if line == "dependencies = [" {
            in_deps = true;
        }
    }
    if let Some(package) = current.take() {
        packages.push(package);
    }
    packages
}

/// Resolve a dependency entry on `dep_name` to a concrete version: `"glam 0.30.10"` is explicit;
/// bare `"glam"` is only unambiguous when a single version exists.
fn resolve_dep<'a>(entry: &'a str, versions: &'a [String]) -> Option<&'a str> {
    let mut parts = entry.split(' ');
    let name = parts.next()?;
    if name != "glam" {
        return None;
    }
    match parts.next() {
        Some(version) => Some(version),
        None => {
            assert_eq!(
                versions.len(),
                1,
                "Cargo.lock has a bare `glam` dependency entry but multiple glam versions — \
                 the lock format should have qualified it; parser out of date?"
            );
            Some(versions[0].as_str())
        }
    }
}

/// FAILS LOUDLY when a dependency bump splits glam — the parry-SIMD desync mechanism described in
/// the module doc. Every glam user in the graph must sit on the one glam Cargo.toml pins
/// `scalar-math` onto; `deny.toml`'s `[bans] deny` holds CI's graph to the same rule. On the next
/// Avian/bevy bump, if parry's math shim (`glamx`), bevy_math or Avian's BVH (`obvhs`) lands on
/// another glam, this is the test that catches it before a cross-platform desync does.
#[test]
fn glam_is_one_feature_unified_instance_for_the_sim_float_path() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock = std::fs::read_to_string(manifest_dir.join("Cargo.lock")).expect("Cargo.lock");
    let packages = parse_lock(&lock);
    assert!(packages.len() > 100, "Cargo.lock parse looks broken");

    let glam_versions: Vec<String> = packages
        .iter()
        .filter(|package| package.name == "glam")
        .map(|package| package.version.clone())
        .collect();
    assert!(!glam_versions.is_empty(), "no glam in Cargo.lock?");

    // The version OUR crate pins (the one `scalar-math` lands on).
    let ours = packages
        .iter()
        .find(|package| package.name == "overmatch")
        .and_then(|package| {
            package
                .dependencies
                .iter()
                .find_map(|entry| resolve_dep(entry, &glam_versions))
        })
        .expect("overmatch must depend on glam directly (the scalar-math pin)");

    // Every package that depends on glam depends on ours.
    let mut sim_glam_users = 0usize;
    for package in &packages {
        for entry in &package.dependencies {
            let Some(version) = resolve_dep(entry, &glam_versions) else {
                continue;
            };
            assert_eq!(
                version,
                ours,
                "GLAM VERSION SPLIT: `{name} {pkg_version}` depends on glam {version}, but the \
                 sim's float path is pinned to glam {ours} (Cargo.toml's `scalar-math` feature). \
                 Cargo features unify only WITHIN one package instance, so this split means \
                 `{name}`'s glam does NOT receive the scalar-math pin: glam's SIMD backends \
                 re-associate dot products, parry's raycasts ride on glam (parry ≤ 0.29 was \
                 measurably non-reproducible across platforms — parry 0.29 changelog), and the \
                 deterministic sim desyncs with no compile error. Fix: bump/pin our glam so the \
                 whole graph (parry's `glamx` shim, `bevy_math`, `obvhs`, us) shares ONE glam \
                 version; where `{name}`'s range admits ours, `cargo update -p \
                 glam@{version} --precise {ours}` does it.",
                name = package.name,
                pkg_version = package.version,
            );
            sim_glam_users += 1;
        }
    }
    assert!(
        sim_glam_users >= 3,
        "expected at least glamx (parry), bevy_math, and overmatch on the pinned glam; \
         found {sim_glam_users} — parser or graph shape changed"
    );

    // Parry's math shim MUST sit on our instance — this is the pin that makes armor raycasts
    // scalar. Named explicitly so an Avian bump that reroutes parry's math fails with a direct
    // message even if the loop above ever loosens.
    for anchor in ["glamx", "bevy_math"] {
        let package = packages
            .iter()
            .find(|package| package.name == anchor)
            .unwrap_or_else(|| {
                panic!("`{anchor}` missing from Cargo.lock — graph reshaped, re-audit the glam pin")
            });
        let version = package
            .dependencies
            .iter()
            .find_map(|entry| resolve_dep(entry, &glam_versions))
            .unwrap_or_else(|| panic!("`{anchor}` no longer depends on glam — re-audit the pin"));
        assert_eq!(
            version, ours,
            "`{anchor}` compiles against glam {version}, our scalar-math pin is on glam {ours}: \
             the sim float path just lost its scalar guarantee (see module doc)"
        );
    }
}

/// Cargo.toml must keep the `scalar-math` feature on its glam dependency — the other half of the
/// invariant the version-split test guards.
#[test]
fn cargo_toml_still_pins_glam_scalar_math() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("Cargo.toml");
    let glam_line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("glam"))
        .expect("Cargo.toml must declare the glam dependency (the scalar-math pin)");
    assert!(
        glam_line.contains("scalar-math"),
        "glam's `scalar-math` feature is gone from Cargo.toml — glam's SIMD backends \
         re-associate dot products and the deterministic sim (and parry's raycasts, which \
         feature-unify into this same glam instance) desync across platforms: {glam_line}"
    );
}
