#![allow(clippy::type_complexity)]

fn main() {
    let offline = std::env::args().any(|a| a == "--offline")
        || std::env::var("OVERMATCH_OFFLINE").as_deref() == Ok("1");
    if offline {
        overmatch::run_offline();
    } else {
        overmatch::run_client();
    }
}
