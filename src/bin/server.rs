// Same rationale as lib.rs's crate-level allow (bins don't inherit it).
#![allow(clippy::type_complexity)]

fn main() {
    overmatch::net::server::run();
}
