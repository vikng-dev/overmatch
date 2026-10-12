//! Guard: the screen state never reaches the fixed clock.
//!
//! `flow::Flow` is view state. A net client on a menu screen must keep ticking and keep sending its
//! command, zeroed by the overlay authority rather than stopped, because native inputs hold their
//! last value when the stream stops. So no registration on a fixed-clock schedule may name `Flow`:
//! not as a run condition, not as a set's condition, not as a system. The input license reaches the
//! fixed-clock command bridge through `overlay::Overlays`, which `Flow` is folded into on `Update`.
//!
//! Only code is scanned (comments stripped). Every `add_systems(` / `configure_sets(` call under
//! `src/` whose schedule argument names a fixed-clock schedule is checked whole.

use std::path::{Path, PathBuf};

/// The schedules that run on the fixed clock, plus the loop that drives them.
const FIXED_SCHEDULES: &[&str] = &[
    "FixedFirst",
    "FixedPreUpdate",
    "FixedUpdate",
    "FixedPostUpdate",
    "FixedLast",
    "FixedMain",
    "RunFixedMainLoop",
];

const REGISTRATIONS: &[&str] = &["add_systems(", "configure_sets("];

/// Blank `//` and nested `/* */` comments, keeping newlines.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut depth = 0usize;
    while i < src.len() {
        let rest = &src[i..];
        if depth > 0 {
            if rest.starts_with("/*") {
                depth += 1;
                out.push_str("  ");
                i += 2;
            } else if rest.starts_with("*/") {
                depth -= 1;
                out.push_str("  ");
                i += 2;
            } else {
                let c = rest.chars().next().expect("char boundary");
                out.push(if c == '\n' { '\n' } else { ' ' });
                i += c.len_utf8();
            }
        } else if rest.starts_with("/*") {
            depth = 1;
            out.push_str("  ");
            i += 2;
        } else if rest.starts_with("//") {
            let end = rest.find('\n').unwrap_or(rest.len());
            out.push_str(&" ".repeat(end));
            i += end;
        } else {
            let c = rest.chars().next().expect("char boundary");
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// The text between the `(` at `open` and its matching `)`.
fn balanced(src: &str, open: usize) -> &str {
    let mut depth = 0i32;
    for (offset, c) in src[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    &src[open + 1..]
}

/// The first top-level argument of an argument list.
fn first_argument(args: &str) -> &str {
    let mut depth = 0i32;
    for (offset, c) in args.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => return args[..offset].trim(),
            _ => {}
        }
    }
    args.trim()
}

/// Whether `text` names `Flow` as a whole identifier.
fn names_flow(text: &str) -> bool {
    text.match_indices("Flow").any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + 4..].chars().next();
        let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !ident(before) && !ident(after)
    })
}

/// Every fixed-clock registration in `src` that names `Flow`, as `line: registration`.
fn violations(src: &str) -> Vec<String> {
    let code = strip_comments(src);
    let mut found = Vec::new();
    for registration in REGISTRATIONS {
        for (at, _) in code.match_indices(registration) {
            let open = at + registration.len() - 1;
            let args = balanced(&code, open);
            let schedule = first_argument(args);
            let fixed = FIXED_SCHEDULES
                .iter()
                .any(|s| schedule == *s || schedule.ends_with(&format!("::{s}")));
            if fixed && names_flow(args) {
                let line = code[..at].matches('\n').count() + 1;
                let compact: String = args.split_whitespace().collect::<Vec<_>>().join(" ");
                found.push(format!("{line}: {registration}{compact})"));
            }
        }
    }
    found
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("readable entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_fixed_clock_registration_names_flow() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rs_files(&root.join("src"), &mut files);
    files.sort();
    assert!(
        files.iter().any(|f| f.ends_with("flow.rs")),
        "the scan must see the module that defines Flow"
    );
    let mut all = Vec::new();
    for path in files {
        let src = std::fs::read_to_string(&path).expect("readable source");
        for violation in violations(&src) {
            all.push(format!("{}:{violation}", path.display()));
        }
    }
    assert!(
        all.is_empty(),
        "the screen state reaches the fixed clock — a client on a menu screen would stop ticking or \
         stop sending. Gate the view, not the sim:\n{}",
        all.join("\n")
    );
}

/// The detector sees what it guards against, and passes what it must not flag.
#[test]
fn the_detector_flags_a_fixed_registration_naming_flow() {
    let gated = "fn p(app: &mut App) {\n    app.add_systems(\n        FixedPreUpdate,\n        \
                 feed.run_if(in_state(Flow::InMatch)),\n    );\n}";
    assert_eq!(violations(gated).len(), 1, "{gated}");
    let set = "app.configure_sets(FixedUpdate, Sim.run_if(in_state(crate::flow::Flow::Lobby)));";
    assert_eq!(violations(set).len(), 1, "{set}");
    let view = "app.add_systems(Update, hud.run_if(in_state(Flow::InMatch)));";
    assert!(violations(view).is_empty(), "Update may gate on Flow");
    let commented = "app.add_systems(FixedUpdate, tick); // not Flow-gated";
    assert!(violations(commented).is_empty(), "comments are prose");
    let other = "app.add_systems(FixedUpdate, flow_rate.run_if(in_state(Flowing::On)));";
    assert!(violations(other).is_empty(), "only the identifier Flow");
}
