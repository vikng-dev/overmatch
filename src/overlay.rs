//! The net client's single overlay authority. Four full-screen overlays used to be ad-hoc — connect
//! status (`net::client`), the death screen (`net::death_screen`), the Esc menu (`net::client`), and
//! the view-death black (`sight`) — each spawning/visibility-swapping on its own with NO shared
//! stacking, and the input/cursor license inferred independently in ~four places (the `cursor_locked`
//! run condition, `feed_action_state`'s zeroing, the `open_menu`/`close_menu` cursor calls, and the
//! `focus_menu`/`tick_refocus_grab` pair). Stacking was spawn-order luck (the view-death opaque black
//! could silently occlude "YOU DIED"), and the license was four copies that had to be kept in sync by
//! hand.
//!
//! This module makes overlay presence and its consequences DERIVED from one place:
//!
//! - [`Overlays`] holds the active set. Owners keep their domain logic (the connect state machine, the
//!   death→respawn state machine, the dead-crewman condition) and only DECLARE their overlay's desired
//!   presence idempotently every frame — the same declare-then-reconcile idiom `death_screen` already
//!   uses internally. There is no imperative push/pop, so a dropped declaration self-heals next frame.
//!   The Esc menu is the one genuine LATCH (edge-driven, nothing in the world to re-derive it from), so
//!   its presence is toggled directly on [`Overlays`] — which is exactly the "retire `MenuOverlay`" the
//!   redesign asks for: menu-openness has one home, `Overlays.contains(Menu)`.
//! - The consequences are PURE functions of that set, so they are unit-testable with no app: whether
//!   input is blocked ([`input_blocked`] — THE single license, replacing all four old inferences),
//!   what Esc dismisses ([`top_dismissable`]), which overlay owns the one scrim ([`draws_scrim`]), and
//!   whether the death status line takes over ([`death_status_line`]).
//! - [`cursor_owner`] is the ONE system that moves the cursor: blocked → release, unblocked → grab. The
//!   winit refocus-grab deferral lives here as its implementation detail.
//!
//! Everything here is view / input-routing only: it runs in `Update` outside `GameplaySet` and outside
//! the fixed/rollback schedule, so the sim keeps ticking under every overlay (there is no online pause —
//! a frozen predicting client desyncs from a server that keeps ticking). Net-client only: mounted by
//! [`crate::NetClientPlugin`], never by single-player (which has `state::client_plugin`'s real pause).

use std::collections::BTreeSet;

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};

use crate::ui_font::UiFonts;

/// The client's full-screen overlays, ordered by PRIORITY via the derived `Ord`. The variants are
/// declared LOW→HIGH, so the greatest is the top layer: `ConnectStatus` is the maximum (a connect /
/// reconnect takes over everything), then `Menu` (an open menu is the top INTERACTIVE layer — Esc
/// closes it, R can't respawn under it), then `Death`, then `ViewDead` at the bottom.
///
/// Priority is deliberately DISTINCT from [`Overlay::zindex`] (the physical draw order): `Menu`
/// outranks `Death` in priority (so the menu, not the death screen, owns the scrim while both are
/// latched) yet draws BELOW it in z, so the death status line — which is part of the death overlay and
/// exempt from the one-scrim rule — renders on top of the menu backdrop (see [`death_status_line`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Overlay {
    /// The active view's crewman is dead (partial crew loss, tank still alive): the optic/commander
    /// goes solid black with a "switch view" prompt. Lowest priority — suppressed entirely under Death.
    ViewDead,
    /// The player's own tank is knocked out: "YOU DIED / press R".
    Death,
    /// The Esc / alt-tab cursor-release menu (the net stand-in for SP pause; the sim never stops).
    Menu,
    /// Not connected yet, or the in-game link dropped: "CONNECTING…" / "RECONNECTING…". Highest.
    ConnectStatus,
}

impl Overlay {
    /// The overlay's explicit `GlobalZIndex` — the one-scrim contract's draw order. Wide gaps leave
    /// room for exempt siblings (the death status line rides Death's 200, above the menu's 100). NOTE
    /// this is NOT the priority order: `Death` (200) draws ABOVE `Menu` (100) even though `Menu`
    /// outranks it in priority, precisely so the death status line shows THROUGH the menu.
    pub(crate) const fn zindex(self) -> i32 {
        match self {
            Overlay::ConnectStatus => 300,
            Overlay::Death => 200,
            Overlay::Menu => 100,
            Overlay::ViewDead => 50,
        }
    }

    /// Whether this overlay CAPTURES input while active — play stops and the cursor frees. The menu is
    /// the obvious one; connect status too (there is no tank to drive until the link is up, and a
    /// visible cursor over "CONNECTING…" is the honest state). Death and view-death do NOT block: the
    /// respawn key and the Lshift view switch both ride `PlayerInputSet`, which needs the cursor
    /// captured — the player is still "in" the tank, just with a dead station.
    const fn blocks_input(self) -> bool {
        matches!(self, Overlay::Menu | Overlay::ConnectStatus)
    }

    /// Whether Esc may dismiss this overlay. Only the menu: Death and ConnectStatus are NEVER
    /// Esc-dismissed (you don't Esc away being dead or disconnected), and ViewDead clears on a crew
    /// switch or a respawn, never on Esc.
    const fn dismissable(self) -> bool {
        matches!(self, Overlay::Menu)
    }
}

/// The active overlay set — the single source of truth for which overlays are latched. Owners declare
/// presence into it every frame ([`Overlays::declare`]); the derived functions below read it. A
/// `BTreeSet` keyed on the priority `Ord`, so [`Overlays::top`] is just the greatest element.
#[derive(Resource, Default)]
pub(crate) struct Overlays {
    active: BTreeSet<Overlay>,
}

impl Overlays {
    /// Idempotently declare whether `overlay` is present this frame. Absolute, not a toggle — re-running
    /// it with the same `present` is a no-op — so a dropped declaration (a system that didn't run one
    /// frame) self-heals the next frame with nothing to keep in sync.
    pub(crate) fn declare(&mut self, overlay: Overlay, present: bool) {
        if present {
            self.active.insert(overlay);
        } else {
            self.active.remove(&overlay);
        }
    }

    /// Whether `overlay` is currently latched.
    fn contains(&self, overlay: Overlay) -> bool {
        self.active.contains(&overlay)
    }

    /// The highest-priority active overlay — the scrim owner AND the top interactive layer. `None` when
    /// nothing is latched (normal play).
    fn top(&self) -> Option<Overlay> {
        self.active.iter().copied().next_back()
    }
}

/// THE single input-license authority — the one derivation that replaces the four scattered inferences
/// (the `cursor_locked` gate, `feed_action_state`'s zeroing, the menu open/close cursor calls, and the
/// focus watcher). Input is blocked when the window is unfocused (the OS took the cursor) OR any latched
/// overlay captures input ([`Overlay::blocks_input`] — the menu or the connect screen). When this is
/// true the cursor owner releases the cursor (which idles `PlayerInputSet` via `state::cursor_locked`)
/// and `feed_action_state` sends a default command, so the tank coasts to a stop instead of holding the
/// last input. `window_focused` is passed in (not read from the set) so it can reflect the LIVE window
/// even in the fixed-schedule consumer, exactly as the old zeroing read `window.focused` directly.
pub(crate) fn input_blocked(overlays: &Overlays, window_focused: bool) -> bool {
    !window_focused || overlays.active.iter().any(|o| o.blocks_input())
}

/// Esc routing: the top overlay IF Esc may dismiss it (only ever the menu). Esc toggles the menu — it
/// opens the menu, or closes it when the menu is the top dismissable layer — and NEVER touches Death or
/// ConnectStatus. Returned as the concrete overlay (not a bare bool) so the caller can assert exactly
/// which layer it is dismissing.
pub(crate) fn top_dismissable(overlays: &Overlays) -> Option<Overlay> {
    overlays.top().filter(|o| o.dismissable())
}

/// The one-scrim-total rule: only the TOP active overlay draws its backdrop + centered content; every
/// lower latched overlay suppresses both (visibility-swap, not despawn — so it snaps back the instant
/// the layer above closes). `draws_scrim(o)` is true only for the scrim owner. This also fixes the
/// view-death occlusion bug for free: with Death latched, Death outranks ViewDead, so ViewDead is never
/// the scrim owner and its opaque black can no longer cover "YOU DIED".
pub(crate) fn draws_scrim(overlays: &Overlays, overlay: Overlay) -> bool {
    overlays.top() == Some(overlay)
}

/// The one exemption from [`draws_scrim`]: while Death is latched but the MENU is drawn on top of it,
/// the full death screen (red backdrop + "YOU DIED / press R") hides and a thin, non-interactive status
/// line — "DEAD — respawn on menu close" — takes over, drawing no backdrop and staying legible THROUGH
/// the menu (it rides Death's higher z). This is why we never show "press R" while it can't work: with
/// the menu up the cursor is released and the respawn key is gated, so the prompt would be a lie. Menu
/// closes → Death becomes the scrim owner again → the full screen returns.
pub(crate) fn death_status_line(overlays: &Overlays) -> bool {
    overlays.contains(Overlay::Death) && overlays.top() == Some(Overlay::Menu)
}

/// Frames to wait after focus returns before the cursor owner auto-grabs. A grab issued the SAME frame
/// focus returns is silently dropped by winit (bevy #16237/#16238), so the owner defers this many
/// frames to let the focus event settle so the grab actually takes.
const REFOCUS_GRAB_FRAMES: u8 = 2;

/// Countdown to a deferred cursor re-grab after the window regains focus while input is unblocked.
/// `None` = idle. Armed by [`cursor_owner`] on a focus-regain event and counted down there (the
/// `focus_menu`/`tick_refocus_grab` deferral, now folded into the single cursor owner).
#[derive(Resource, Default)]
struct RefocusGrab(Option<u8>);

/// The Esc menu overlay's backdrop node. Persistent (spawned once, visibility-swapped) so it can
/// coexist in state with a latched death screen and snap back instantly, per the one-scrim rule.
#[derive(Component)]
struct MenuNode;

/// The pinned intra-frame order for the overlay authority: owners DECLARE presence, THEN the cursor
/// owner derives the license and moves the cursor. Chained so the cursor (and, transitively, the
/// `PlayerInputSet` gate it drives) reads a fully-reconciled set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OverlaySet {
    /// Owners declare desired overlay presence into [`Overlays`]. Runs first.
    Declare,
    /// The single cursor owner: release when [`input_blocked`], (deferred) grab when not.
    Cursor,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Overlays>()
        .init_resource::<RefocusGrab>()
        // Declare presence → cursor owner. The wire-command zeroing (`feed_action_state`) is the fourth
        // conceptual link but lives in `FixedPreUpdate`; it reads the same [`input_blocked`] authority
        // off the set reconciled by the previous frame's declarations, exactly as the old zeroing read
        // last frame's `menu.open`.
        .configure_sets(Update, (OverlaySet::Declare, OverlaySet::Cursor).chain())
        .add_systems(Startup, spawn_menu_overlay)
        .add_systems(
            Update,
            (
                (esc_toggle, focus_declare).in_set(OverlaySet::Declare),
                cursor_owner.in_set(OverlaySet::Cursor),
                // Reads the reconciled set; ordered after the declarations so a menu opened this frame
                // shows this frame.
                apply_menu_visibility.after(OverlaySet::Declare),
            ),
        );
}

/// Spawn the Esc menu backdrop once, hidden. `apply_menu_visibility` reveals it whenever the menu is
/// the scrim owner. Shares `ui_font::spawn_overlay` with the connect / death / pause overlays so the
/// family reads as one, then stamps the one-scrim `GlobalZIndex` on the returned node.
fn spawn_menu_overlay(mut commands: Commands, fonts: Res<UiFonts>) {
    let node = crate::ui_font::spawn_overlay(
        &mut commands,
        &fonts.hud,
        MenuNode,
        "MENU\nEsc to close",
        (),
        Some(Color::srgba(0.0, 0.0, 0.0, 0.6)),
    );
    commands
        .entity(node)
        .insert((GlobalZIndex(Overlay::Menu.zindex()), Visibility::Hidden));
}

/// Esc toggles the menu presence directly on [`Overlays`] — the menu's one home, retiring the old
/// `MenuOverlay{open}`. Close only when the menu is the top DISMISSABLE layer (so Esc under a connect
/// screen can't half-close a menu buried beneath it); otherwise open. The cursor follows from
/// [`input_blocked`] via the cursor owner — this system moves no cursor itself.
fn esc_toggle(keys: Res<ButtonInput<KeyCode>>, mut overlays: ResMut<Overlays>) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let open = top_dismissable(&overlays) != Some(Overlay::Menu);
    overlays.declare(Overlay::Menu, open);
}

/// Alt-tab out declares the menu present (there is no online pause — the game keeps running behind the
/// translucent overlay), preserving today's focus-loss behavior. Regaining focus does NOT auto-close
/// the menu (matching the old `focus_menu`); the player closes it with Esc, and the cursor owner's
/// deferred re-grab then takes over. Only the loss edge matters here; the cursor is the cursor owner's
/// job.
fn focus_declare(mut focus: MessageReader<WindowFocused>, mut overlays: ResMut<Overlays>) {
    if let Some(false) = crate::state::collapse_focus(&mut focus) {
        overlays.declare(Overlay::Menu, true);
    }
}

/// The ONE cursor owner: blocked → release the cursor, unblocked → grab it, with the winit refocus-grab
/// deferral as an implementation detail. Folds together the four old cursor sites (the menu open/close
/// grabs and the `focus_menu`/`tick_refocus_grab` pair) behind the single [`input_blocked`] authority.
///
/// The deferral: a grab issued the same frame focus returns is dropped by winit, so a focus-REGAIN
/// event arms [`RefocusGrab`] and the grab waits [`REFOCUS_GRAB_FRAMES`] frames. While blocked (menu
/// open, or still unfocused) the countdown is cancelled and the cursor stays released — so the deferral
/// only ever fires on a regain into an unblocked state (menu closed), exactly as before.
fn cursor_owner(
    overlays: Res<Overlays>,
    mut focus: MessageReader<WindowFocused>,
    mut refocus: ResMut<RefocusGrab>,
    window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
) {
    let (mut window, mut cursor) = window.into_inner();
    // Arm the deferred re-grab on a focus-REGAIN edge; a loss cancels any pending one.
    if let Some(focused) = crate::state::collapse_focus(&mut focus) {
        refocus.0 = focused.then_some(REFOCUS_GRAB_FRAMES);
    }
    if input_blocked(&overlays, window.focused) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        refocus.0 = None; // nothing to (re)grab while blocked
        return;
    }
    // Unblocked → grab, honoring any refocus deferral (stay released until it elapses).
    match refocus.0 {
        Some(n) if n > 1 => refocus.0 = Some(n - 1),
        Some(_) => {
            refocus.0 = None;
            crate::state::grab_now(&mut window, &mut cursor);
        }
        None => crate::state::grab_now(&mut window, &mut cursor),
    }
}

/// Visibility-swap the menu backdrop: shown only while the menu is the scrim owner (open and nothing
/// higher-priority — a connect screen — on top of it). Kept latched in `Overlays` regardless, so it
/// snaps back the instant a layer above closes.
fn apply_menu_visibility(
    overlays: Res<Overlays>,
    mut menu: Query<&mut Visibility, With<MenuNode>>,
) {
    if let Ok(mut vis) = menu.single_mut() {
        vis.set_if_neq(if draws_scrim(&overlays, Overlay::Menu) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `Overlays` with exactly the given overlays latched — the fixture for the pure-function
    /// tests (no app, no world).
    fn overlays(active: &[Overlay]) -> Overlays {
        Overlays {
            active: active.iter().copied().collect(),
        }
    }

    /// Priority `Ord` is ConnectStatus > Menu > Death > ViewDead, so `top` picks the right scrim owner
    /// for every combination. This is the ordering the whole authority hangs on.
    #[test]
    fn priority_orders_connect_over_menu_over_death_over_viewdead() {
        assert!(Overlay::ConnectStatus > Overlay::Menu);
        assert!(Overlay::Menu > Overlay::Death);
        assert!(Overlay::Death > Overlay::ViewDead);
        assert_eq!(overlays(&[]).top(), None);
        assert_eq!(
            overlays(&[Overlay::Death, Overlay::ViewDead]).top(),
            Some(Overlay::Death),
            "Death outranks ViewDead — the occlusion bug can't recur",
        );
        assert_eq!(
            overlays(&[Overlay::Menu, Overlay::Death]).top(),
            Some(Overlay::Menu),
            "an open menu is the top interactive layer over the death screen",
        );
        assert_eq!(
            overlays(&[Overlay::ConnectStatus, Overlay::Menu, Overlay::Death]).top(),
            Some(Overlay::ConnectStatus),
        );
    }

    /// The z-order contract is fixed AND deliberately not the priority order: Death draws above Menu so
    /// the death status line (which rides Death's z) shows through the menu.
    #[test]
    fn zindex_contract_is_pinned() {
        assert_eq!(Overlay::ConnectStatus.zindex(), 300);
        assert_eq!(Overlay::Death.zindex(), 200);
        assert_eq!(Overlay::Menu.zindex(), 100);
        assert_eq!(Overlay::ViewDead.zindex(), 50);
        assert!(
            Overlay::Death.zindex() > Overlay::Menu.zindex(),
            "Death draws over Menu though Menu outranks it — for the status line",
        );
    }

    /// `input_blocked` is the single license: an unfocused window blocks regardless of overlays; the
    /// menu and connect screen block; Death and ViewDead do NOT (R and Lshift ride `PlayerInputSet`,
    /// which needs the cursor captured).
    #[test]
    fn input_blocked_matches_the_capturing_overlays() {
        assert!(
            !input_blocked(&overlays(&[]), true),
            "focused, nothing up → free"
        );
        assert!(
            input_blocked(&overlays(&[]), false),
            "unfocused always blocks"
        );
        assert!(input_blocked(&overlays(&[Overlay::Menu]), true));
        assert!(input_blocked(&overlays(&[Overlay::ConnectStatus]), true));
        assert!(
            !input_blocked(&overlays(&[Overlay::Death]), true),
            "the death screen must NOT block — the respawn key rides PlayerInputSet",
        );
        assert!(
            !input_blocked(&overlays(&[Overlay::ViewDead]), true),
            "view-death must NOT block — the Lshift view switch rides PlayerInputSet",
        );
    }

    /// Esc dismisses only the menu, and only when the menu is the top layer. Death / ConnectStatus /
    /// ViewDead are never dismissable, even when they are on top.
    #[test]
    fn top_dismissable_is_only_the_menu_on_top() {
        assert_eq!(top_dismissable(&overlays(&[])), None);
        assert_eq!(
            top_dismissable(&overlays(&[Overlay::Menu, Overlay::Death])),
            Some(Overlay::Menu),
        );
        assert_eq!(
            top_dismissable(&overlays(&[Overlay::Death])),
            None,
            "Esc never dismisses the death screen",
        );
        assert_eq!(
            top_dismissable(&overlays(&[Overlay::ConnectStatus, Overlay::Menu])),
            None,
            "connect status is on top and undismissable — Esc can't reach the menu beneath it",
        );
        assert_eq!(top_dismissable(&overlays(&[Overlay::ViewDead])), None);
    }

    /// The one-scrim rule: exactly the top overlay draws; ViewDead is suppressed under Death.
    #[test]
    fn only_the_top_overlay_draws_its_scrim() {
        let set = overlays(&[Overlay::Death, Overlay::ViewDead]);
        assert!(draws_scrim(&set, Overlay::Death));
        assert!(
            !draws_scrim(&set, Overlay::ViewDead),
            "ViewDead suppressed entirely under Death — no opaque black over YOU DIED",
        );
    }

    /// The original-bug regression, one scrim total: with BOTH Menu and Death latched, exactly one of
    /// them draws its backdrop (the menu), the death backdrop is suppressed, the death status line
    /// takes over, and R is gated (input blocked by the menu). This is the whole redesign in one case.
    #[test]
    fn menu_over_death_yields_exactly_one_backdrop() {
        let set = overlays(&[Overlay::Menu, Overlay::Death]);
        // Exactly one full backdrop across the two overlays: the menu's.
        let backdrops = [Overlay::Menu, Overlay::Death]
            .into_iter()
            .filter(|&o| draws_scrim(&set, o))
            .count();
        assert_eq!(backdrops, 1, "one scrim total");
        assert!(draws_scrim(&set, Overlay::Menu));
        assert!(!draws_scrim(&set, Overlay::Death), "death backdrop hidden");
        assert!(death_status_line(&set), "status line shown instead");
        assert!(input_blocked(&set, true), "R gated — the menu blocks input");
    }

    /// The status line is menu-over-death ONLY: it does not show when Death owns the scrim (full screen
    /// instead), nor when a connect screen is what's covering the death state.
    #[test]
    fn status_line_is_menu_over_death_only() {
        assert!(
            !death_status_line(&overlays(&[Overlay::Death])),
            "Death alone shows the full screen, not the status line",
        );
        assert!(!death_status_line(&overlays(&[Overlay::Menu])));
        assert!(
            !death_status_line(&overlays(&[Overlay::ConnectStatus, Overlay::Death])),
            "a connect screen over death takes over fully — no death status line under it",
        );
    }
}
