//! Guard: the screen state never reaches the fixed clock.
//!
//! `flow::Flow` is view state. A net client on a menu screen must keep ticking and keep sending its
//! command, zeroed by the overlay authority rather than stopped, because native inputs hold their
//! last value when the stream stops. So no registration on a fixed-clock schedule may name `Flow`:
//! not as a run condition, not as a set's condition, not as a system. The input license reaches the
//! fixed-clock command bridge through `overlay::Overlays`, which `Flow` is folded into on `Update`.
//!
//! Only code is scanned (comments stripped). Every `add_systems(` / `configure_sets(` call under
//! `src/` whose schedule argument names a fixed-clock schedule is checked whole, for `Flow` and for
//! every identifier that reaches it ([`tainted`]): a system or run condition whose signature names
//! `Flow` (`Res<State<Flow>>`, a `SystemParam` that holds one), a type whose definition names it,
//! and a set some registration gates on it.

use std::collections::BTreeSet;
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

/// Whether `text` names `ident` as a whole identifier that is not a method call (`.ident`).
fn names(text: &str, ident: &str) -> bool {
    text.match_indices(ident).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + ident.len()..].chars().next();
        let ident_char = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !ident_char(before) && before != Some('.') && !ident_char(after)
    })
}

fn names_any(text: &str, idents: &BTreeSet<String>) -> bool {
    idents.iter().any(|ident| names(text, ident))
}

/// The identifier starting at byte `at`.
fn ident_at(code: &str, at: usize) -> &str {
    let len = code[at..]
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(code.len() - at);
    &code[at..at + len]
}

/// The text from `at` to the end of a `fn`'s signature: the first `{` or `;` outside brackets.
fn signature(code: &str, at: usize) -> &str {
    let mut depth = 0i32;
    for (offset, c) in code[at..].char_indices() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth -= 1,
            '{' | ';' if depth <= 0 => return &code[at..at + offset],
            _ => {}
        }
    }
    &code[at..]
}

/// The text from `at` to the end of a type's definition: the `}` matching its first `{`, or the
/// first `;` outside brackets.
fn definition(code: &str, at: usize) -> &str {
    let mut depth = 0i32;
    for (offset, c) in code[at..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => depth -= 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &code[at..=at + offset];
                }
            }
            ';' if depth == 0 => return &code[at..at + offset],
            _ => {}
        }
    }
    &code[at..]
}

/// The byte after every `keyword` that begins a token.
fn after_keyword<'a>(code: &'a str, keyword: &'a str) -> impl Iterator<Item = usize> + 'a {
    code.match_indices(keyword).filter_map(move |(at, _)| {
        let before = code[..at].chars().next_back();
        (!before.is_some_and(|c| c.is_alphanumeric() || c == '_')).then_some(at + keyword.len())
    })
}

/// `Flow` and every identifier whose code reaches it, over all sources: each `struct`/`enum`/`type`
/// whose definition names `Flow` or another such type (a `SystemParam` holding `Res<State<Flow>>`),
/// to a fixed point; then each `fn` whose signature names one of those types (a system's or a run
/// condition's parameters); then each set configured `.run_if(..)` on any of them. A `fn` taints
/// nothing further: only its own registration can carry `Flow` onto a schedule.
fn tainted(sources: &[String]) -> BTreeSet<String> {
    let mut types = BTreeSet::from(["Flow".to_owned()]);
    loop {
        let before = types.len();
        for code in sources {
            for keyword in ["struct ", "enum ", "type "] {
                for at in after_keyword(code, keyword) {
                    let name = ident_at(code, at);
                    if !name.is_empty() && names_any(definition(code, at + name.len()), &types) {
                        types.insert(name.to_owned());
                    }
                }
            }
        }
        if types.len() == before {
            break;
        }
    }
    let mut tainted = types.clone();
    for code in sources {
        for at in after_keyword(code, "fn ") {
            let name = ident_at(code, at);
            if !name.is_empty() && names_any(signature(code, at + name.len()), &types) {
                tainted.insert(name.to_owned());
            }
        }
    }
    for code in sources {
        for (at, _) in code.match_indices("configure_sets(") {
            let args = balanced(code, at + "configure_sets(".len() - 1);
            for (run_if, _) in args.match_indices(".run_if(") {
                let condition = balanced(args, run_if + ".run_if(".len() - 1);
                if !names_any(condition, &tainted) {
                    continue;
                }
                let set_end = args[..run_if].trim_end();
                let set_start = set_end
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |i| i + 1);
                if set_start < set_end.len() {
                    tainted.insert(set_end[set_start..].to_owned());
                }
            }
        }
    }
    tainted
}

/// Every fixed-clock registration in `code` (comments stripped) that names a tainted identifier,
/// as `line: registration`.
fn violations(code: &str, tainted: &BTreeSet<String>) -> Vec<String> {
    let mut found = Vec::new();
    for registration in REGISTRATIONS {
        for (at, _) in code.match_indices(registration) {
            let open = at + registration.len() - 1;
            let args = balanced(code, open);
            let schedule = first_argument(args);
            let fixed = FIXED_SCHEDULES
                .iter()
                .any(|s| schedule == *s || schedule.ends_with(&format!("::{s}")));
            if fixed && names_any(args, tainted) {
                let line = code[..at].matches('\n').count() + 1;
                let compact: String = args.split_whitespace().collect::<Vec<_>>().join(" ");
                found.push(format!("{line}: {registration}{compact})"));
            }
        }
    }
    found
}

/// [`violations`] of one source, tainting from that source alone.
fn violations_in(src: &str) -> Vec<String> {
    let code = strip_comments(src);
    let tainted = tainted(std::slice::from_ref(&code));
    violations(&code, &tainted)
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
    let sources: Vec<(PathBuf, String)> = files
        .into_iter()
        .map(|path| {
            let src = std::fs::read_to_string(&path).expect("readable source");
            (path, strip_comments(&src))
        })
        .collect();
    let codes: Vec<String> = sources.iter().map(|(_, code)| code.clone()).collect();
    let tainted = tainted(&codes);
    assert!(
        tainted.contains("InMatchSystems"),
        "the taint must reach the match-only set gated on Flow: {tainted:?}"
    );
    let mut all = Vec::new();
    for (path, code) in &sources {
        for violation in violations(code, &tainted) {
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
    assert_eq!(violations_in(gated).len(), 1, "{gated}");
    let set = "app.configure_sets(FixedUpdate, Sim.run_if(in_state(crate::flow::Flow::Lobby)));";
    assert_eq!(violations_in(set).len(), 1, "{set}");
    let view = "app.add_systems(Update, hud.run_if(in_state(Flow::InMatch)));";
    assert!(violations_in(view).is_empty(), "Update may gate on Flow");
    let commented = "app.add_systems(FixedUpdate, tick); // not Flow-gated";
    assert!(violations_in(commented).is_empty(), "comments are prose");
    let other = "app.add_systems(FixedUpdate, flow_rate.run_if(in_state(Flowing::On)));";
    assert!(violations_in(other).is_empty(), "only the identifier Flow");
    let param = "fn tick(flow: Res<State<Flow>>) {}\n\
                 fn p(app: &mut App) { app.add_systems(FixedUpdate, tick); }";
    assert_eq!(violations_in(param).len(), 1, "a system parameter: {param}");
    let condition = "fn in_lobby(flow: Res<State<crate::flow::Flow>>) -> bool { true }\n\
                     fn p(app: &mut App) { app.add_systems(FixedUpdate, tick.run_if(in_lobby)); }";
    assert_eq!(
        violations_in(condition).len(),
        1,
        "a run condition: {condition}"
    );
    let param_struct = "#[derive(SystemParam)]\nstruct Screen<'w> { flow: Res<'w, State<Flow>> }\n\
                        fn tick(screen: Screen) {}\n\
                        fn p(app: &mut App) { app.add_systems(FixedPostUpdate, tick); }";
    assert_eq!(
        violations_in(param_struct).len(),
        1,
        "a SystemParam: {param_struct}"
    );
    let gated_set = "fn p(app: &mut App) {\n\
                     app.configure_sets(Update, Hud.run_if(in_state(Flow::InMatch)));\n\
                     app.add_systems(FixedUpdate, tick.in_set(Hud));\n}";
    assert_eq!(
        violations_in(gated_set).len(),
        1,
        "a set gated on Flow: {gated_set}"
    );
    let method = "fn write(next: &mut NextState<Flow>) {}\n\
                  fn p(app: &mut App) { app.add_systems(FixedUpdate, tick.run_if(|w: Res<W>| w.write())); }";
    assert!(
        violations_in(method).is_empty(),
        "a method call is not the tainted fn: {method}"
    );
}
