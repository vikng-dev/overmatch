//! Quitting: the two holes between "bevy wants to exit" and "the process is gone".
//!
//! Both are macOS-shaped, both were found the same afternoon by sampling a wedged `overmatch`
//! (2026-07-27, M4 / macOS 26.5 / Metal), and they compound: the second one is what makes Cmd+Q
//! look like a crash, and the first one is why routing Cmd+Q anywhere else would not have helped.
//!
//! # Hole 1 — bevy's exit teardown DEADLOCKS against its own render thread
//!
//! MEASURED, from `sample(1)` on a hung debug build:
//!
//! ```text
//! main thread:   -[NSApplication terminate:] / winit internal_exit
//!                  -> Event::LoopExiting -> bevy_winit exiting() -> World::clear_all()
//!                  -> drop_in_place<RenderAppChannels> -> recv_blocking -> __psynch_cvwait
//! render thread: SubApp::update -> Render schedule -> MultiThreadedExecutor
//!                  -> blocked waiting for a NON-SEND system to be run on the MainThreadExecutor
//! ```
//!
//! `bevy_render::pipelined_rendering` hands the render `SubApp` to a render thread each frame and
//! takes it back inside `renderer_extract`, which waits with
//! `scope_with_executor(true, Some(&main_thread_executor), …)` — i.e. it TICKS the main-thread
//! executor while it waits, which is the only way the render schedule's main-thread-pinned systems
//! ever run. `RenderAppChannels`' `Drop` waits with a bare `recv_blocking()` instead, and
//! `bevy_winit`'s `exiting()` calls `World::clear_all()`, which runs that `Drop`. So if the render
//! thread still owes a non-send system when the app decides to exit, the main thread parks waiting
//! for the render thread and the render thread parks waiting for the main thread. Forever.
//!
//! The render schedule ALWAYS owes one on macOS: `bevy_render::view::window::create_surfaces` takes
//! a `NonSendMarker` under `#[cfg(any(target_os = "macos", target_os = "ios"))]`. Our own
//! [`settings::probe`](crate::settings) adds a second on every platform. Upstream: bevy#12912
//! (open since 2024), repro bevy#24035, fix bevy#24059 — open, milestone 0.20, NOT in 0.19.
//!
//! MEASURED here, driving the real `-[NSApplication terminate:]` from inside the process (the
//! literal Cmd+Q menu action, no debugger attached): **4/4 hung** without this module, **8/8 exited
//! 0** with it. The same hang reproduces through the window's close button, so it is the EXIT that
//! is broken, not Cmd+Q. It is a timing race, not a certainty — the same trials on a release build
//! exited cleanly 4/4 before the fix, and bevy#24059's author measures "20-40% of the time on Apple
//! Silicon" — which is exactly the shape of an intermittent "sometimes it quits, sometimes it
//! wedges" report.
//!
//! [`recall_render_app`] is that fix at OUR altitude, with no vendored crate: it wraps
//! `RenderExtractApp`'s extract function (public API — `take_extract`/`set_extract`), which is the
//! one hook that runs on the main thread AFTER the render app has been handed over and BEFORE the
//! winit runner notices the exit. When an [`AppExit`] is pending it pulls the render app back the
//! way `renderer_extract` does — ticking the main-thread executor — and drops it there. That clears
//! `RenderAppChannels`' "still in the render thread" flag, so the `Drop` inside `clear_all()`
//! becomes a no-op and cannot park. The render thread then sees its channel close and returns.
//!
//! # Hole 2 — Cmd+Q never becomes an `AppExit` at all
//!
//! winit installs the standard macOS app menu, whose Quit item sends `-[NSApplication terminate:]`.
//! winit does NOT implement `applicationShouldTerminate:` (there are zero hits for it in the
//! crate); it only observes `NSApplicationWillTerminateNotification`, which fires when termination
//! is already irrevocable. From there it closes the windows and emits `LoopExiting` straight into
//! `clear_all()` — no `AppExit`, no `Last`-schedule shutdown work, and no chance for hole 1's fix
//! to have run. Upstream: bevy#9499, bevy#20316 (both open).
//!
//! [`terminate`] closes that by adding `applicationShouldTerminate:` to winit's delegate class at
//! runtime and answering `NSTerminateCancel`, recording the request instead. A system then writes
//! [`AppExit::Success`], so Cmd+Q takes EXACTLY the path the window's close button takes — which is
//! the path hole 1's fix sits on.
//!
//! Answering `NSTerminateCancel` also means a logout/restart initiated while the game is running is
//! cancelled rather than honoured; the app quits by itself a frame later, so the retry succeeds.
//! The alternative (`NSTerminateLater` + `replyToApplicationShouldTerminate:`) needs AppKit to pump
//! a run loop bevy does not own, which is a worse trade than one re-asked logout.

use bevy::app::AppExit;
use bevy::ecs::message::Messages;
use bevy::ecs::schedule::MainThreadExecutor;
use bevy::prelude::*;
use bevy::render::pipelined_rendering::{RenderAppChannels, RenderExtractApp};
use bevy::tasks::ComputeTaskPool;

/// Mounted once per WINDOWED root, and every windowed root is one of these four:
/// [`ClientPlugin`](crate::ClientPlugin) (single-player and the offline feel test),
/// [`NetClientPlugin`](crate::NetClientPlugin), and the two dev sandboxes
/// ([`sandbox::plugin`](crate::sandbox::plugin), [`track_sandbox::plugin`](crate::track_sandbox)) —
/// which mount `DefaultPlugins` themselves and are therefore on the same deadlocking exit path. The
/// sandboxes reach this through their own crate-internal plugin, so nothing here needs to be `pub`
/// for the `bin/` shells that run them.
///
/// The headless server has no render app and no application menu, so there is nothing here for it
/// to do; it is the one root that does not mount this.
pub(crate) fn plugin(app: &mut App) {
    recall_render_app_on_exit(app);
    #[cfg(target_os = "macos")]
    app.add_plugins(terminate::plugin);
}

/// Wrap `RenderExtractApp`'s extract so that the frame which decides to exit also pulls the render
/// app back out of the render thread. See the module doc for why this is the hook.
///
/// Ordering is the whole point: `SubApps::update` runs the main schedule (where `AppExit` is
/// written), then `sub_app.extract(main_world)` — this function — and the winit runner only checks
/// for the exit after `App::update` returns. So this is the last main-thread code to run before
/// `bevy_winit`'s `exiting()` calls `World::clear_all()`.
fn recall_render_app_on_exit(app: &mut App) {
    // No render app (headless), or pipelined rendering disabled: nothing owes anything.
    let Some(extract_app) = app.get_sub_app_mut(RenderExtractApp) else {
        return;
    };
    let Some(mut inner) = extract_app.take_extract() else {
        return;
    };
    let mut recalled = false;
    extract_app.set_extract(move |main_world, extract_world| {
        // Past the recall the render app is gone — dropped, on this thread, on the exit frame. A
        // second recall, and bevy's own extract just as much, would wait on a render thread that
        // has nothing left to send: the exact hang this exists to prevent. There is also nothing
        // left to extract FOR, so the whole hand-off simply stops. (bevy stops updating after an
        // `AppExit` anyway; this is the belt to that braces.)
        if recalled {
            return;
        }
        inner(main_world, extract_world);
        if !exit_pending(main_world) {
            return;
        }
        recalled = true;
        recall_render_app(main_world);
    });
}

/// Is this the frame that ends the app? Read off the message buffer rather than a cursor because
/// there is no state to keep: any `AppExit` at all, from any writer, means `bevy_winit` is about to
/// stop the loop and tear the world down.
fn exit_pending(world: &World) -> bool {
    world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|exits| !exits.is_empty())
}

/// Pull the render `SubApp` back to the main thread and drop it there, ticking the main-thread
/// executor while waiting — the same handshake `bevy_render`'s `renderer_extract` performs, and the
/// one `RenderAppChannels`' `Drop` is missing.
///
/// Dropping it HERE rather than leaking it is deliberate: the render world's non-send data (the
/// wgpu surface, the Metal objects behind it) was created on the main thread and is documented by
/// bevy as needing to be dropped there. This is that drop, just early enough to still be possible.
fn recall_render_app(world: &mut World) {
    if !world.contains_resource::<RenderAppChannels>()
        || !world.contains_resource::<MainThreadExecutor>()
    {
        return;
    }
    world.resource_scope(|world, main_thread_executor: Mut<MainThreadExecutor>| {
        world.resource_scope(|_world, mut channels: Mut<RenderAppChannels>| {
            let render_app = ComputeTaskPool::get().scope_with_executor(
                true,
                Some(&*main_thread_executor.0),
                |scope| {
                    scope.spawn(async { channels.recv().await });
                },
            );
            // On the main thread, by construction — see the doc above.
            drop(render_app);
        });
    });
}

/// The macOS `Cmd+Q` half: teach winit's application delegate to ANSWER the terminate request, and
/// answer it by asking bevy to exit instead. See the module doc.
#[cfg(target_os = "macos")]
mod terminate {
    use core::ffi::CStr;
    use core::mem;
    use core::time::Duration;
    use std::sync::atomic::{AtomicBool, Ordering};

    use bevy::app::AppExit;
    use bevy::ecs::system::NonSendMarker;
    use bevy::prelude::*;
    use bevy::winit::{UpdateMode, WinitSettings};
    // The runtime's OWN declarations, not hand-written ones. Two of the four types in this module
    // are the ones that are easiest to get silently wrong, and a mismatched foreign declaration is
    // unsound whatever it happens to do today:
    //   * `BOOL` is C `_Bool` (Rust `bool`) on aarch64-apple and `signed char` (`i8`) on
    //     x86_64-apple — `objc-sys`' `types.rs` carries the whole cfg matrix. We ship macOS on
    //     arm64 only, but that must not be what makes this correct.
    //   * `IMP` is a NULLABLE FUNCTION pointer, `Option<unsafe extern "C" fn()>` — a niche-packed
    //     fn pointer, not a `*const c_void`.
    use objc_sys::{
        BOOL, IMP, NO, NSUInteger, class_addMethod, class_getInstanceMethod, objc_class,
        objc_getClass, objc_object, objc_selector, sel_registerName,
    };

    /// Set from AppKit's thread (which is the main thread — `applicationShouldTerminate:` is a
    /// main-thread delegate callback), read by [`forward_quit_request`] on the same thread. An
    /// atomic rather than a resource because the callback is a bare C function with no world.
    static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

    /// `NSTerminateCancel` — "do not terminate". We quit ourselves instead, one frame later.
    /// `NSApplicationTerminateReply` is an `NS_ENUM(NSUInteger, …)`, hence the type.
    const NS_TERMINATE_CANCEL: NSUInteger = 0;

    /// winit's `NSApplicationDelegate` subclass (winit 0.30's `declare_class!` names it exactly
    /// this). If a winit bump renames it, [`install_terminate_handler`] says so loudly rather than
    /// silently restoring the old behaviour.
    const WINIT_DELEGATE_CLASS: &CStr = c"WinitApplicationDelegate";

    /// The Objective-C type encoding for `- (NSUInteger)applicationShouldTerminate:(id)sender`:
    /// `NSUInteger` return, then the two implicit arguments (`self`, `_cmd`) and the sender.
    const SHOULD_TERMINATE_TYPES: &CStr = c"Q@:@";

    /// `Q` above is `unsigned long long`, i.e. `NSUInteger` under LP64 and nothing else. Every
    /// Apple platform this can build for is 64-bit, and this is where that stops being an
    /// assumption.
    const _: () = assert!(
        size_of::<NSUInteger>() == 8,
        "the applicationShouldTerminate: type encoding spells NSUInteger as 'Q' (LP64 only)",
    );

    /// The signature the runtime will call [`application_should_terminate`] with — spelled out so
    /// the transmute into the type-erased [`IMP`] is a named conversion rather than an inferred one.
    type ShouldTerminateImp = unsafe extern "C" fn(
        this: *mut objc_object,
        cmd: *const objc_selector,
        sender: *mut objc_object,
    ) -> NSUInteger;

    pub(super) fn plugin(app: &mut App) {
        app.add_systems(Startup, (install_terminate_handler, check_quit_latency))
            .add_systems(First, forward_quit_request);
    }

    /// AppKit's `- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender`.
    /// Runs on the main thread, inside `-[NSApplication terminate:]`, before ANY of the teardown
    /// this module exists to avoid.
    unsafe extern "C" fn application_should_terminate(
        _this: *mut objc_object,
        _cmd: *const objc_selector,
        _sender: *mut objc_object,
    ) -> NSUInteger {
        QUIT_REQUESTED.store(true, Ordering::Relaxed);
        NS_TERMINATE_CANCEL
    }

    /// Add the method to winit's delegate class. `Startup` is the earliest schedule that runs
    /// INSIDE the winit runner, i.e. the first point at which the class is guaranteed registered
    /// (winit builds the delegate in `EventLoop::new`, which `App::run` reaches before the first
    /// update).
    ///
    /// The `NonSendMarker` pins this to the main thread — the same pin `branding::set_window_icon`
    /// and `settings::observe_window_mode` use, for the same reason: this is AppKit state.
    fn install_terminate_handler(_non_send_marker: NonSendMarker) {
        // SAFETY: `objc_getClass` / `sel_registerName` / `class_getInstanceMethod` /
        // `class_addMethod` are called with the declarations from `objc-sys`, i.e. the runtime's
        // own — see the `use` above for the two that matter. The class pointer is checked for null
        // before it is used, and `class_addMethod` documents that adding a method to an existing
        // class (as opposed to adding an ivar) is legal at any time.
        //
        // The IMP: the runtime will call it as `SHOULD_TERMINATE_TYPES` describes — `(id, SEL, id)
        // -> NSUInteger` — which is exactly [`ShouldTerminateImp`], so the erasing transmute below
        // is the same one `objc2`'s own `ClassBuilder::add_method` performs. Transmuting straight
        // to `IMP` rather than to its inner fn type keeps the erased signature the runtime crate's
        // to define, and is sound for the reason `Option<extern fn>` is the canonical FFI nullable
        // callback: a function pointer and its null-niched `Option` share size and layout, and this
        // one is non-null, so it lands on `Some`.
        let added: BOOL = unsafe {
            let class: *const objc_class = objc_getClass(WINIT_DELEGATE_CLASS.as_ptr());
            if class.is_null() {
                error!(
                    "quit: winit's application delegate class is not registered under the name \
                     this build expects — Cmd+Q will tear the world down mid-frame instead of \
                     exiting through AppExit (see src/quit.rs)"
                );
                return;
            }
            let selector: *const objc_selector =
                sel_registerName(c"applicationShouldTerminate:".as_ptr());
            if !class_getInstanceMethod(class, selector).is_null() {
                warn!(
                    "quit: winit now answers applicationShouldTerminate: itself — leaving it \
                     alone, and this module's Cmd+Q half is dead code (see src/quit.rs)"
                );
                return;
            }
            let imp = mem::transmute::<ShouldTerminateImp, IMP>(application_should_terminate);
            class_addMethod(
                class.cast_mut(),
                selector,
                imp,
                SHOULD_TERMINATE_TYPES.as_ptr(),
            )
        };
        // `BOOL` is a plain `bool` on aarch64-apple and an `i8` on x86_64-apple, so this comparison
        // against the runtime's own `NO` is the one shape that reads correctly on both — and it is
        // a `bool` comparison on exactly one of them, which is what the allow is for.
        #[allow(clippy::bool_comparison)]
        let failed = added == NO;
        if failed {
            error!("quit: could not install the applicationShouldTerminate: handler");
            return;
        }
        debug!("quit: Cmd+Q now routes through AppExit");
    }

    /// Turn a recorded terminate request into the ordinary exit. `Local` rather than clearing the
    /// flag, so a second Cmd+Q during the (single-frame) shutdown cannot queue a second exit.
    ///
    /// Cancelling the terminate produces NO winit event, so this system is reached only by the
    /// app's ordinary update cadence — the invariant [`check_quit_latency`] guards.
    fn forward_quit_request(mut exit: MessageWriter<AppExit>, mut sent: Local<bool>) {
        if *sent || !QUIT_REQUESTED.load(Ordering::Relaxed) {
            return;
        }
        *sent = true;
        info!("quit: Cmd+Q requested - exiting");
        exit.write(AppExit::Success);
    }

    /// How long a cancelled terminate may sit before the app gets an update to write [`AppExit`]
    /// in. A frame at any sane rate is far inside this; the number exists to be a threshold, not a
    /// target, and 250 ms is the boundary between "the window vanished" and "did that do
    /// anything?".
    const QUIT_LATENCY_BUDGET: Duration = Duration::from_millis(250);

    /// Can a root in this update mode leave a cancelled Cmd+Q hanging?
    ///
    /// This — not "is it `Continuous`" — is the real invariant, and the distinction is load-bearing
    /// rather than pedantic: `WinitSettings::game()` (which is `default()`, and therefore what both
    /// dev sandboxes run) is Continuous while FOCUSED and reactive at 60 Hz while not, and a Cmd+Q
    /// from the Dock's menu arrives unfocused. Demanding `Continuous` in both slots would cry wolf
    /// on every sandbox boot while catching nothing, and a guard that is normally wrong is a guard
    /// nobody reads. What genuinely breaks Half B is a mode that can wait a long time — or, at
    /// `Duration::MAX`, forever — for an event that cancelling a terminate never produces.
    fn stalls_quit(mode: UpdateMode) -> bool {
        match mode {
            UpdateMode::Continuous => false,
            UpdateMode::Reactive { wait, .. } => wait > QUIT_LATENCY_BUDGET,
        }
    }

    /// The invariant guard for [`forward_quit_request`]: warn, loudly and by name, if this root's
    /// update cadence could swallow a Cmd+Q.
    ///
    /// A `warn!` rather than a panic because the failure it describes is a slow quit, not a broken
    /// game — and because the fix belongs to whoever changed the update mode, who needs to be told
    /// what they broke, not stopped from booting. `Option<Res<…>>` so a root without winit (the
    /// headless server never mounts this module, but tests and future roots might) simply says
    /// nothing.
    fn check_quit_latency(settings: Option<Res<WinitSettings>>) {
        let Some(settings) = settings else {
            return;
        };
        for (which, mode) in [
            ("focused", settings.focused_mode),
            ("unfocused", settings.unfocused_mode),
        ] {
            if stalls_quit(mode) {
                warn!(
                    "quit: this root's {which} update mode is {mode:?}, which can wait longer than \
                     {QUIT_LATENCY_BUDGET:?} for an event. Cancelling a terminate produces no \
                     event, so Cmd+Q will appear to do nothing until something else wakes the \
                     loop. Either keep the mode continuous (or briefly reactive), or make \
                     quit::terminate poke the event loop itself (see src/quit.rs)."
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The guard has to be silent on every update mode this repo actually ships and loud on the
        /// ones that would break Half B. The first two rows are the reason it is not a
        /// `== Continuous` check.
        #[test]
        fn the_latency_guard_fires_only_on_modes_that_can_swallow_a_quit() {
            let quiet = |settings: WinitSettings| {
                !stalls_quit(settings.focused_mode) && !stalls_quit(settings.unfocused_mode)
            };
            // What the game's windowed roots insert.
            assert!(quiet(WinitSettings::continuous()));
            // What the two dev sandboxes get by default — reactive while unfocused, and fine.
            assert!(quiet(WinitSettings::game()));
            assert!(quiet(WinitSettings::default()));

            // The shapes that would strand a cancelled terminate.
            assert!(stalls_quit(UpdateMode::reactive(Duration::MAX)));
            assert!(stalls_quit(WinitSettings::desktop_app().focused_mode));
            assert!(!quiet(WinitSettings::desktop_app()));

            // And the boundary itself, from both sides.
            assert!(!stalls_quit(UpdateMode::reactive(QUIT_LATENCY_BUDGET)));
            assert!(stalls_quit(UpdateMode::reactive(
                QUIT_LATENCY_BUDGET + Duration::from_millis(1)
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bevy::app::SubApp;

    use super::*;

    /// The exact version string cargo resolved for a crate, read out of the lockfile. Pulled in at
    /// COMPILE time, so the tripwire below is re-evaluated the moment `Cargo.lock` moves rather
    /// than whenever someone remembers to look.
    fn locked_version<'lock>(lock: &'lock str, crate_name: &str) -> Option<&'lock str> {
        let mut lines = lock.lines();
        while let Some(line) = lines.next() {
            if line.trim_end() != format!("name = \"{crate_name}\"") {
                continue;
            }
            // Cargo writes `version` on the line after `name`, but scan rather than index it: a
            // lockfile format change should make this return `None` (and fail loudly at the call
            // site) instead of quietly reading the wrong field.
            for next in lines.by_ref() {
                if let Some(rest) = next.trim().strip_prefix("version = \"") {
                    return rest.strip_suffix('"');
                }
                if next.trim().is_empty() {
                    break;
                }
            }
        }
        None
    }

    /// UPSTREAM TRIPWIRE, and this one is a DELETION notice rather than a defect pin.
    ///
    /// [`recall_render_app_on_exit`] exists for exactly one upstream defect — bevy#12912, whose fix
    /// (PR #24059, `RenderAppChannels::drop` learning to tick the `MainThreadExecutor`) is open
    /// against milestone **0.20**. On the bevy bump that lands it, Half A becomes redundant: not
    /// wrong, not harmful, just a wrapper doing by hand what the `Drop` now does for itself — and
    /// therefore a thing that will sit in this tree forever unless something says otherwise.
    ///
    /// **This test is that something.** When it fails, the removal checklist is:
    ///   1. confirm the bump actually carries #24059 (`RenderAppChannels::drop` in the new
    ///      `bevy_render` — if it still calls a bare `recv_blocking()`, the fix did NOT land and
    ///      Half A must stay; only widen the version check below);
    ///   2. delete [`recall_render_app_on_exit`], [`recall_render_app`], [`exit_pending`] and their
    ///      three tests, plus the `RenderExtractApp`/`RenderAppChannels`/`MainThreadExecutor`/
    ///      `ComputeTaskPool` imports and "Hole 1" from the module doc;
    ///   3. KEEP the whole `terminate` module — Half B answers a winit gap (no
    ///      `applicationShouldTerminate:` at all; bevy#9499, bevy#20316), which #24059 does not
    ///      touch and which nothing upstream has fixed;
    ///   4. delete this test and `locked_version`.
    #[test]
    fn bevy_render_is_still_the_0_19_that_needs_half_a() {
        let lock = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock"));
        let version = locked_version(lock, "bevy_render")
            .expect("bevy_render has a [[package]] entry in Cargo.lock with a version field");
        assert!(
            version.starts_with("0.19."),
            "bevy_render is now {version} — upstream PR #24059's RenderAppChannels fix likely \
             shipped, so quit.rs's Half A (the render-app recall wrapper) is probably redundant. \
             Verify, then delete Half A, KEEP Half B (winit still has no terminate hook), and \
             delete this test. Full checklist on this test's doc comment.",
        );
    }

    /// Build a main-world `App` carrying a `RenderExtractApp` whose extract counts its calls —
    /// the shape `PipelinedRenderingPlugin::build` leaves behind, minus a renderer.
    ///
    /// The counter is an `Arc`, deliberately, and NOT a `static`: `cargo test` runs these in
    /// parallel on one process, so a shared counter reset per call makes each test's assertions
    /// depend on the other's scheduling. (It did — the two callers below started colliding the
    /// moment a third test joined the module.)
    fn app_with_counting_extract() -> (App, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let mut app = App::new();
        app.add_message::<AppExit>();
        let mut extract_app = SubApp::new();
        extract_app.set_extract(move |_main, _sub| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        app.insert_sub_app(RenderExtractApp, extract_app);
        (app, calls)
    }

    /// The wrapper is a wrapper: bevy's own extract still runs, every frame, exit or no exit.
    /// Getting this wrong would stop the renderer rather than fix the shutdown.
    #[test]
    fn the_wrapper_still_runs_bevys_extract() {
        let (mut app, calls) = app_with_counting_extract();
        recall_render_app_on_exit(&mut app);
        app.update();
        app.update();
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        // …including on the frame that decides to exit.
        app.world_mut().write_message(AppExit::Success);
        app.update();
        assert_eq!(calls.load(Ordering::Relaxed), 3);

        // But NOT after it: the render app has been recalled and dropped, so bevy's extract would
        // be waiting on a render thread with nothing left to send.
        app.update();
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    /// Without the pipelined-rendering resources there is nothing to recall, and in particular
    /// nothing to BLOCK on: the recall must be a no-op rather than a `resource_scope` panic or a
    /// wait on a channel that will never deliver. This is the headless/`RenderApp`-less shape, and
    /// it is also the shape a second recall would see — which is why the wrapper only fires once.
    #[test]
    fn a_recall_without_the_channels_is_a_no_op() {
        let (mut app, _calls) = app_with_counting_extract();
        recall_render_app_on_exit(&mut app);
        app.world_mut().write_message(AppExit::Success);
        app.update();
        app.update();
        // Reached at all == neither panicked nor parked.
        assert!(!app.world().contains_resource::<RenderAppChannels>());
    }

    /// `exit_pending` reads the buffer, not a cursor: it must be true for the whole frame in which
    /// the exit was written, whoever wrote it and whenever they wrote it.
    #[test]
    fn exit_pending_sees_the_frames_exit() {
        let mut world = World::new();
        assert!(!exit_pending(&world), "no message resource at all");
        world.init_resource::<Messages<AppExit>>();
        assert!(!exit_pending(&world), "an empty buffer is not an exit");
        world.write_message(AppExit::Success);
        assert!(exit_pending(&world));
    }
}
