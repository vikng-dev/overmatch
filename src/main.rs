//! `overmatch` — the PVP client, and the product. A thin executable shell: it hands off to the
//! library's client runtime, which builds the Bevy `App` (windowed presentation + device gather),
//! resolves the server address (`OVERMATCH_SERVER` / the baked `OVERMATCH_DEFAULT_SERVER` / loopback),
//! points `AssetPlugin` at the exe-relative asset root, and connects.
//!
//! `--offline` (or `OVERMATCH_OFFLINE=1`) instead runs the netcode-free single-player
//! composition — the element-grip feel-test route (`overmatch::run_offline`).

// Same rationale as lib.rs's crate-level allow (bins don't inherit it).
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
