//! `asset_verify <id>.glb` — the consumer contract at the command line.
//!
//! A thin adapter over `overmatch::verify_asset`: the same implementation the runtime bake runs at
//! startup and CI runs over every committed pair. The spec sheet is the model's sibling
//! `<id>.tank.ron`, derived mechanically, so the model names the whole pair.
//!
//! `--canon <id>.tank.ron` is the other direction: it writes `overmatch::canon_lists`' one JSON
//! document to stdout, which is how the Blender source pass reads canonical Rust lists it may not
//! keep a second copy of. Nothing Blender-shaped exists here — the door's wrapper generates the
//! file and hands the path to `export_tank.py --canon`.
//!
//! `--registry <materials.ron>` names the substance data both directions are read against. The door
//! passes the registry beside the model — which, in the lanes that verify a REVISION rather than the
//! work tree, is that revision's file. Omitted, the registry compiled into this binary is used: the
//! game ships the two together, so for the game they cannot disagree.
//!
//! Exit 1 means the report contains an error. The report goes to stdout either way.

use std::path::{Path, PathBuf};

const USAGE: &str = "usage: asset_verify [--registry <materials.ron>] <assets/<id>/<id>.glb> \
                     [more models…]\n       \
                     asset_verify [--registry <materials.ron>] --canon \
                     <assets/<id>/<id>.tank.ron>";

fn main() {
    let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    // `--registry` names the substance registry these models are read against. The door passes the
    // one beside the model — which, in the lanes that verify a REVISION, is that revision's file
    // rather than the work tree's. Omitted, the registry compiled into this binary is used, which
    // is what the game itself wants: it ships the two together.
    let (registry, rest) = match arguments.split_first() {
        Some((first, rest)) if first == "--registry" => match rest.split_first() {
            Some((path, rest)) => (Some(path.clone()), rest.to_vec()),
            None => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        },
        _ => (None, arguments.clone()),
    };
    let registry = registry.as_deref();
    match rest.split_first() {
        Some((first, models)) if first == "--canon" => canon(models, registry),
        Some(_) => verify(&rest, registry),
        None => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Write the canon file's one JSON document to stdout. A sheet that does not parse is a report on
/// stderr and exit 1, so a wrapper redirecting stdout into a file never captures half a document.
fn canon(arguments: &[PathBuf], registry: Option<&Path>) {
    let [spec] = arguments else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    match overmatch::canon_lists(spec, registry) {
        Ok(json) => println!("{json}"),
        Err(findings) => {
            eprint!("{}", overmatch::render(&findings));
            std::process::exit(1);
        }
    }
}

fn verify(models: &[PathBuf], registry: Option<&Path>) {
    let mut refused = false;
    for model in models {
        let findings = overmatch::verify_asset(Path::new(model), registry);
        refused |= overmatch::has_error(&findings);
        print!("{}", overmatch::render(&findings));
        println!("{}: {} finding(s)", model.display(), findings.len());
    }
    if refused {
        std::process::exit(1);
    }
}
