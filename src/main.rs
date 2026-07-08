//! `overmatch` — the PVP client, and the product. A thin runtime shell: it hands off to
//! `net::client::run()`, which builds the Bevy `App` (windowed presentation + device gather),
//! resolves the server address (`OVERMATCH_SERVER` / the baked `OVERMATCH_DEFAULT_SERVER` / loopback),
//! points `AssetPlugin` at the exe-relative asset root, and connects. Requires the `net` feature
//! (default). The retired single-player entry lived here; the sim now only runs under the client
//! (predicted replica) or the dedicated `overmatch-server`.

// Same rationale as lib.rs's crate-level allow (bins don't inherit it).
#![allow(clippy::type_complexity)]

fn main() {
    overmatch::net::client::run();
}
