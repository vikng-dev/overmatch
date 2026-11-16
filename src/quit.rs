//! Retire rendering before window teardown, and route macOS Cmd+Q through `AppExit`.
//!
//! The final render update must finish while the main world still owns its windows. During
//! `World::clear_all`, a render-thread `RawHandleWrapper` removal can otherwise drop the last
//! native window owner. On macOS, winit 0.30.13's `Window::drop` synchronously dispatches its close
//! onto AppKit's main queue (`platform_impl/macos/window.rs`); Bevy 0.20.1's
//! `RenderAppChannels::drop` pumps the ECS executor, not that queue. [`retire_renderer_on_exit`]
//! receives and drops the render app before winit reaches `clear_all`, preserving those owners.
//!
//! winit installs the standard macOS app menu, whose Quit item sends `-[NSApplication terminate:]`.
//! winit does NOT implement `applicationShouldTerminate:` (there are zero hits for it in the
//! crate); it only observes `NSApplicationWillTerminateNotification`, which fires when termination
//! is already irrevocable. From there it closes the windows and emits `LoopExiting` straight into
//! `clear_all()` — no `AppExit` and no `Last`-schedule shutdown work. Upstream: bevy#9499, bevy#20316 (both open).
//!
//! [`terminate`] closes that by adding `applicationShouldTerminate:` to winit's delegate class at
//! runtime and answering `NSTerminateCancel`, recording the request instead. A system then writes
//! [`AppExit::Success`], so Cmd+Q takes EXACTLY the path the window's close button takes — which is
//! the ordinary shutdown path.
//!
//! Answering `NSTerminateCancel` also means a logout/restart initiated while the game is running is
//! cancelled rather than honoured; the app quits by itself a frame later, so the retry succeeds.
//! The alternative (`NSTerminateLater` + `replyToApplicationShouldTerminate:`) needs AppKit to pump
//! a run loop bevy does not own, which is a worse trade than one re-asked logout.

use bevy::ecs::schedule::MainThreadExecutor;
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::pipelined_rendering::{
    PipelinedRenderingPlugin, RenderAppChannels, RenderExtractApp,
};
use bevy::tasks::ComputeTaskPool;

/// Mounted once per WINDOWED root, and every windowed root is one of these four:
/// [`ClientPlugin`](crate::ClientPlugin) (single-player and the offline feel test),
/// [`NetClientPlugin`](crate::NetClientPlugin), and the two dev sandboxes
/// ([`sandbox::plugin`](crate::sandbox::plugin), [`track_sandbox::plugin`](crate::track_sandbox)) —
/// which mount `DefaultPlugins` themselves and share the same menu route. The
/// sandboxes reach this through their own crate-internal plugin, so nothing here needs to be `pub`
/// for the `bin/` shells that run them.
///
/// The headless server has no render app and no application menu, so there is nothing here for it
/// to do; it is the one root that does not mount this.
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(RetireRenderer);
    #[cfg(target_os = "macos")]
    app.add_plugins(terminate::TerminateRoute);
}

/// Retires the last render frame while the main world's window owners still live. Winit clears
/// them in `exiting`; waiting in RenderAppChannels::drop is later than this boundary. Decided in
/// `finish`, so the answer does not depend on whether this root added `DefaultPlugins` before or
/// after the quit plugin.
struct RetireRenderer;

impl Plugin for RetireRenderer {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        // No render thread to retire: a windowless root mounts no pipelined rendering, and a
        // GPU-less one (`backends: None`) mounts it with no `RenderApp` to pipeline. `RenderApp`
        // is still a sub-app here; `PipelinedRenderingPlugin::cleanup` moves it to its thread.
        if !app.is_plugin_added::<PipelinedRenderingPlugin>()
            || app.get_sub_app(RenderApp).is_none()
        {
            return;
        }
        let extract = app.get_sub_app_mut(RenderExtractApp).expect(
            "PipelinedRenderingPlugin, added after RenderPlugin, builds RenderExtractApp beside \
             the RenderApp it pipelines",
        );
        retire_renderer_on_exit(extract);
    }
}

fn retire_renderer_on_exit(extract: &mut SubApp) {
    let mut inner = extract
        .take_extract()
        .expect("PipelinedRenderingPlugin installs its renderer extract with RenderExtractApp");
    let mut retired = false;
    extract.set_extract(move |main, render| {
        if retired {
            return;
        }
        inner(main, render);
        if main.resource::<Messages<AppExit>>().is_empty() {
            return;
        }
        retired = true;
        main.resource_scope(|main, executor: Mut<MainThreadExecutor>| {
            main.resource_scope(|_, mut channels: Mut<RenderAppChannels>| {
                drop(ComputeTaskPool::get().scope_with_executor(
                    true,
                    Some(&executor.0),
                    |scope| {
                        scope.spawn(async { channels.recv().await });
                    },
                ));
            });
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
    /// this). A winit bump that renames it breaks the startup contract and must refuse startup
    /// rather than restore the broken quit path.
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

    /// Mounts the Cmd+Q route on a root that carries `WinitPlugin`, decided in `finish` so the
    /// answer does not depend on whether this root added `DefaultPlugins` before or after the quit
    /// plugin. A windowless composition has no winit runner, delegate or application menu.
    pub(super) struct TerminateRoute;

    impl Plugin for TerminateRoute {
        fn build(&self, _app: &mut App) {}

        fn finish(&self, app: &mut App) {
            if app.is_plugin_added::<bevy::winit::WinitPlugin>() {
                app.add_systems(Startup, (install_terminate_handler, check_quit_latency))
                    .add_systems(First, forward_quit_request);
            }
        }
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
    /// and `settings::observe_window_placement` use, for the same reason: this is AppKit state.
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
            assert!(
                !class.is_null(),
                "quit: winit must register WinitApplicationDelegate before Startup; update the \
                 terminate handler for this winit version before starting a windowed root"
            );
            let selector: *const objc_selector =
                sel_registerName(c"applicationShouldTerminate:".as_ptr());
            assert!(
                class_getInstanceMethod(class, selector).is_null(),
                "quit: winit now implements applicationShouldTerminate:; verify its AppExit \
                 routing and remove the custom terminate handler before starting a windowed root"
            );
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
        #[allow(
            clippy::bool_comparison,
            reason = "`BOOL` is `bool` on aarch64-apple only; this comparison is the one shape correct on both"
        )]
        let failed = added == NO;
        assert!(
            !failed,
            "quit: the registered winit delegate must accept applicationShouldTerminate:; \
             refusing startup without the AppExit quit route"
        );
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
    /// nobody reads. What strands a quit is a mode that can wait a long time — or, at
    /// `Duration::MAX`, forever — for an event that cancelling a terminate never produces.
    fn stalls_quit(mode: UpdateMode) -> bool {
        match mode {
            UpdateMode::Continuous => false,
            UpdateMode::Reactive { wait, .. } => wait > QUIT_LATENCY_BUDGET,
        }
    }

    /// Refuse a windowed root whose update cadence can strand a cancelled Cmd+Q. This is an
    /// application-composition invariant: cancelling termination produces no event to wake winit.
    /// Mounted only beside `WinitPlugin`, which inserts the settings this reads.
    fn check_quit_latency(settings: Res<WinitSettings>) {
        for (which, mode) in [
            ("focused", settings.focused_mode),
            ("unfocused", settings.unfocused_mode),
        ] {
            assert!(
                !stalls_quit(mode),
                "quit: the {which} update mode {mode:?} can strand a cancelled Cmd+Q beyond \
                 {QUIT_LATENCY_BUDGET:?}; use continuous or briefly reactive updates, or wake \
                 the event loop from the terminate callback"
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A root with a stranded quit must fail before play. Merely logging the bad cadence
        /// leaves the player unable to quit, even though the mode classifier still passes its test.
        #[test]
        #[should_panic(expected = "can strand a cancelled Cmd+Q")]
        fn a_root_that_can_strand_quit_is_refused() {
            let mut app = App::new();
            app.insert_resource(WinitSettings::desktop_app())
                .add_systems(Startup, check_quit_latency);
            app.update();
        }

        /// The guard has to be silent on every update mode this repo actually ships and loud on the
        /// ones that would strand a quit. The first two rows are the reason it is not a
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
    use super::*;

    /// Window teardown may invalidate renderer inputs. Every AppExit writer, including capture
    /// completion and Cmd+Q, needs the pipelined render app dropped before winit clears the world,
    /// whichever order the root mounts the renderer and this plugin in: here this plugin comes
    /// first, before `RenderExtractApp` exists. Fails if the retire is decided at build.
    #[test]
    fn an_exit_frame_retires_the_render_app_before_window_teardown() {
        use bevy::app::AppExit;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        #[derive(Resource)]
        struct Retired(Arc<AtomicBool>);
        impl Drop for Retired {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }

        let retired = Arc::new(AtomicBool::new(false));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let mut render = SubApp::new();
        render.insert_resource(Retired(Arc::clone(&retired)));
        app.insert_sub_app(RenderApp, render);
        app.add_plugins((plugin, PipelinedRenderingPlugin));
        app.finish();
        app.cleanup();
        app.update();
        assert!(
            !retired.load(Ordering::Relaxed),
            "ordinary frames keep rendering alive"
        );
        app.add_systems(Update, |mut exit: MessageWriter<AppExit>| {
            exit.write(AppExit::Success);
        });
        app.update();
        assert_eq!(app.should_exit(), Some(AppExit::Success));
        assert!(
            retired.load(Ordering::Relaxed),
            "rendering must retire before winit's World::clear_all"
        );
        // An extra update must not wait for a renderer that has already retired.
        app.update();
    }

    /// A root without a window or a renderer needs neither exit route: no application delegate,
    /// and no render thread, whether it mounts no pipelined rendering (windowless) or mounts it
    /// with no `RenderApp` to pipeline (GPU-less, the headless fixtures' shape). Fails if the
    /// retire is keyed on the pipelined plugin alone.
    #[test]
    fn a_root_without_a_window_or_renderer_needs_no_exit_route() {
        for pipelined in [false, true] {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins);
            if pipelined {
                app.add_plugins(PipelinedRenderingPlugin);
            }
            app.add_plugins(plugin);
            app.finish();
            app.cleanup();
            app.update();
        }
    }

    /// Pipelined rendering mounted before its `RenderApp` existed builds nothing (Bevy reads
    /// `PipelinedRenderingPlugin` added before `RenderPlugin` as "do nothing"); the root is
    /// refused at finish, naming the order, rather than left without its retire.
    #[test]
    #[should_panic(expected = "PipelinedRenderingPlugin, added after RenderPlugin")]
    fn a_renderer_without_its_pipelined_extract_is_refused() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PipelinedRenderingPlugin));
        app.insert_sub_app(RenderApp, SubApp::new());
        app.add_plugins(plugin);
        app.finish();
    }
}
