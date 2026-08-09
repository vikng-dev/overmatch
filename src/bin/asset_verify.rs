//! `asset_verify <id>.glb` — the consumer contract at the command line.
//!
//! A thin adapter over `overmatch::verify_asset`: the same implementation the runtime bake runs at
//! startup and CI runs over every committed pair. The spec sheet is the model's sibling
//! `<id>.tank.ron`, derived mechanically, so the model names the whole pair.
//!
//! Exit 1 means the report contains an error. The report goes to stdout either way.

use std::path::PathBuf;

fn main() {
    let models: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if models.is_empty() {
        eprintln!("usage: asset_verify <assets/<id>/<id>.glb> [more models…]");
        std::process::exit(2);
    }
    let mut refused = false;
    for model in models {
        let findings = overmatch::verify_asset(&model);
        refused |= overmatch::has_error(&findings);
        print!("{}", overmatch::render(&findings));
        println!("{}: {} finding(s)", model.display(), findings.len());
    }
    if refused {
        std::process::exit(1);
    }
}
