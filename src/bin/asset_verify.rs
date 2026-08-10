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
//! Exit 1 means the report contains an error. The report goes to stdout either way.

use std::path::{Path, PathBuf};

const USAGE: &str = "usage: asset_verify <assets/<id>/<id>.glb> [more models…]\n       \
                     asset_verify --canon <assets/<id>/<id>.tank.ron>";

fn main() {
    let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    match arguments.split_first() {
        Some((first, rest)) if first == "--canon" => canon(rest),
        Some(_) => verify(&arguments),
        None => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Write the canon file's one JSON document to stdout. A sheet that does not parse is a report on
/// stderr and exit 1, so a wrapper redirecting stdout into a file never captures half a document.
fn canon(arguments: &[PathBuf]) {
    let [spec] = arguments else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    match overmatch::canon_lists(spec) {
        Ok(json) => println!("{json}"),
        Err(findings) => {
            eprint!("{}", overmatch::render(&findings));
            std::process::exit(1);
        }
    }
}

fn verify(models: &[PathBuf]) {
    let mut refused = false;
    for model in models {
        let findings = overmatch::verify_asset(Path::new(model));
        refused |= overmatch::has_error(&findings);
        print!("{}", overmatch::render(&findings));
        println!("{}: {} finding(s)", model.display(), findings.len());
    }
    if refused {
        std::process::exit(1);
    }
}
