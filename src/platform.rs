//! OS-level process and window helpers that belong to no game layer.

/// Renounce macOS app activation for a hidden capture client — registered wherever a window is
/// forced hidden: the networked client's hidden-capture path, `run_offline`'s hidden-capture mode,
/// and the dev frame capture.
///
/// winit's `didFinishLaunching` forces `ActivationPolicy::Regular` on an unbundled binary and
/// activates it unconditionally, and bevy 0.19 exposes no macOS `EventLoopBuilder` hook to
/// pre-empt that, so the revocation must run post-launch, in `Startup`.
///
/// `Prohibited` means the process can never own the menu bar or become the active application
/// again — exactly right for a headless-in-spirit capture — and the `deactivate` yields the focus
/// winit already stole back to whatever the user had. The `NonSendMarker` pins the system to the
/// main thread (the repo's `settings::probe` idiom), which is what makes the CHECKED
/// `MainThreadMarker` constructor infallible here.
#[cfg(target_os = "macos")]
pub(crate) fn revoke_macos_activation(_non_send_marker: bevy::ecs::system::NonSendMarker) {
    use bevy::log::warn;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = objc2_foundation::MainThreadMarker::new()
        .expect("NonSendMarker pinned this system to the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // The returned bool is "did the policy switch take"; a capture run has no UI to degrade, so
    // log-and-continue is the right failure mode.
    if !app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited) {
        warn!("capture client: macOS refused ActivationPolicy::Prohibited — focus may be stolen");
    }
    // SAFETY: a plain AppKit call with no invariants beyond main-thread, which `mtm` proves;
    // `unsafe` only because the objc2 0.2 generation did not audit it as safe.
    unsafe { app.deactivate() };
}
