//! `overmatch` — the PVP client, and the product. A thin executable shell: it hands off to the
//! library's client runtime, which builds the Bevy `App` (windowed presentation + device gather),
//! resolves the server address (`OVERMATCH_SERVER` / the baked `OVERMATCH_DEFAULT_SERVER` / loopback),
//! points `AssetPlugin` at the exe-relative asset root, and connects.

// Same rationale as lib.rs's crate-level allow (bins don't inherit it).
#![allow(clippy::type_complexity)]

fn main() {
    overmatch::run_client();
}
