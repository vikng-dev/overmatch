# Upstream: macOS `set_fullscreen` silently ignores a monitor change on an already-borderless window

Status: **DRAFT, not filed.** Found: 2026-08-17, hunting a settings-page defect ("changing the target
display while in fullscreen does nothing"). Root-caused from winit source, not inferred.

Target: **winit 0.30.13** (crates.io;
`~/.cargo/registry/src/index.crates.io-*/winit-0.30.13/src/platform_impl/macos/window_delegate.rs`).
Severity (us): **HIGH** — the display row is inert in the mode the game is normally played in, and
the window is left on a monitor winit reports it is not on.

## Symptom

`Window::set_fullscreen(Some(Fullscreen::Borderless(Some(other_monitor))))` on a window that is
already in borderless fullscreen returns without doing anything observable: no `toggleFullScreen:`
is sent, the window stays in the Space it was in, on the screen it was on. The call is not rejected
and nothing is logged. Afterwards `Window::fullscreen()` reports the monitor that was *requested*,
so every subsequent equality check — including the caller's own — agrees the move happened.

## Mechanism (verified against winit-0.30.13 source)

`WindowDelegate::set_fullscreen` (`window_delegate.rs:1300-1489`) ends in

```rust
match (old_fullscreen, fullscreen) {          // :1421
    (None, Some(fullscreen))                                        => …  // :1422 toggle_fullscreen
    (Some(Fullscreen::Borderless(_)), None)                         => …  // :1445
    (Some(Fullscreen::Exclusive(ref video_mode)), None)             => …  // :1449
    (Some(Fullscreen::Borderless(_)), Some(Fullscreen::Exclusive(_)))  => …  // :1453
    (Some(Fullscreen::Exclusive(ref video_mode)), Some(Fullscreen::Borderless(_))) => …  // :1473
    _ => {},                                                              // :1487
};
```

There is no `(Some(Borderless), Some(Borderless))` arm, so a monitor change on an already-borderless
window falls to the catch-all and no fullscreen transition is ever issued.

Two things have already happened by then, and both outlive the no-op:

- **:1411** `self.ivars().fullscreen.replace(fullscreen.clone())` — the new value is committed
  *before* the match, so `fullscreen()` afterwards describes a window that never moved. State and
  reality disagree permanently, with no path back: a caller that re-issues the same request hits the
  `fullscreen == old_fullscreen` early return at **:1315**.
- **:1319-1341** when the target screen differs from `self.window().screen()`, `setFrameOrigin` is
  called on a window that is sitting in a macOS fullscreen Space (**:1339**). In the arms that do
  work this is the pre-positioning for the toggle that follows; here nothing follows it.

The deferral path lands in the same hole. A call arriving while `in_fullscreen_transition` is set is
parked in `target_fullscreen` (**:1308-1312**) and replayed from `windowDidEnterFullScreen`
(**:291-299**) — as `Borderless -> Borderless`, into the missing arm again.

`(Some(Exclusive), Some(Exclusive))` — an exclusive video-mode change — falls into the same
catch-all. Not exercised here; likely the same defect.

### Why a caller cannot work around it by re-writing the mode

The committed-ivar half is what makes this unreachable from above rather than merely awkward.
`bevy_winit` forwards a window-mode change only where `winit_window.fullscreen() != new_mode`
(`bevy_winit-0.19.0/src/system.rs:361-364`), and after the no-op that comparison is already equal —
so the second request is not even sent. Any caller with the same (reasonable) guard is in the same
position.

## Suggested upstream fix

Add the missing arm, sequenced through the machinery already there rather than as a second
`toggleFullScreen:`:

```rust
(Some(Fullscreen::Borderless(_)), Some(Fullscreen::Borderless(_))) => {
    // exit, and let `windowDidExitFullScreen` replay the entry on the new screen
    self.ivars().target_fullscreen.replace(Some(fullscreen));
    self.ivars().fullscreen.replace(old_fullscreen);
    toggle_fullscreen(self);
}
```

`target_fullscreen` + the `windowDidExitFullScreen` replay (**:303-311**) already express exactly
this: leave, then apply the parked request as the `None -> Some` arm that works. The one thing the
arm must also do is *not* commit the new value to `ivars().fullscreen` up front, which argues for
moving the **:1411** commit into the arms that actually perform a transition — a fix worth making on
its own, since it is what makes the failure silent and unrecoverable rather than merely a no-op.

Note for whoever files it: the same-screen case must stay a no-op, and `Borderless(None)` has to be
resolved through `current_monitor_inner()` before comparing, exactly as **:1323-1335** already does.

## Our workaround

`observe_window_placement` + `RoundTrip` in `src/settings.rs`. The app observes the window's REAL
monitor (`winit_window.current_monitor()`) beside its real fullscreen state, compares it against the
display row resolved through `bevy_winit`'s own `select_monitor`, and on a disagreement drives the
round trip winit does implement: write `WindowMode::Windowed`, wait for the OBSERVED fullscreen edge
to go false, then write `BorderlessFullscreen(target)` — a `None -> Some`.

It is armed by the DISAGREEMENT rather than by the player touching the row, because the same
swallowed call reaches us three other ways: an indexed rung cannot be named at window creation, so a
fullscreen boot is born on the fallback monitor and its own `Startup` correction is a
`Borderless -> Borderless` no-op; a monitor hot-plug changes what a stored rung resolves to on a
window that is already fullscreen; and once winit's belief has split from the real window, every
later recompute — including the `setFrameOrigin` above — is working from the wrong screen, so
repairing the divergence as soon as it is observable is what stops it being something to recompute
from.

The cost is not just the code. It is ~1-2 s of two visible macOS Space animations to change display,
one attempt per target (a trip that does not land must not be retried, or the broader trigger flaps
the window between Spaces for the rest of the session — it settles and logs once instead), and a
transitional windowed state that every other window rule in that module has to be taught to ignore.

Observing the real monitor is the load-bearing half and would remain worth keeping: nothing else can
notice this defect, because the component, bevy's window cache and winit's own state all read as
correct while the window sits on the other panel.

## Removal condition

A winit release whose macOS `set_fullscreen` carries a `(Some(Borderless), Some(Borderless))` arm.
At that point the round trip (`RoundTrip`, `PlacementStep`, their two tests and the mode yield in
`apply_settings`) is deleted and the display row goes back to the plain level-triggered
`Window::mode` write.

No automatic tripwire: the observable is an OS window moving between physical panels, which no test
in this tree can see. Re-check the arm list by hand on a winit bump.

## What fixing this unlocks for us

Deleting `RoundTrip`, `PlacementStep`, `WindowPlacement::round_trip`, the mode yield in
`apply_settings` and two tests — and, more than the lines, the removal of a transitional window state
that every future window rule would otherwise have to account for. The player-facing win is real
though small: changing display while fullscreen becomes instant instead of two Space animations.

`ObservedPlacement` and the real-monitor observation stay regardless — that is our instrument, not
the workaround.
