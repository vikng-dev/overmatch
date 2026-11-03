//! Net-client overlay state and derived input/cursor policy.
//!
//! Invariant: [`Overlays`] is the single source of truth for overlay precedence, input blocking, and
//! scrim ownership. This module is view/input routing only; it never pauses rollback simulation.

use std::collections::BTreeSet;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::query::QuerySingleError;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};

use crate::flow::{Flow, InMatchSystems};
use crate::state::{EscapeAction, EscapeRouted};

/// Match-only overlays, declared low→high priority (`Ord`): the greatest is top. `ConnectStatus`
/// takes over on link loss; `Menu` is the top interactive layer (Esc closes it, R cannot respawn
/// beneath it). Lobby and initial connect are screens (`flow::Flow`), not overlays.
///
/// Priority differs from [`Overlay::zindex`]: `Menu` owns the scrim over `Death` but draws below it,
/// letting the exempt [`death_status_line`] show through the menu backdrop.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Overlay {
    /// The game's damage model: the active station is out of action while the tank remains in play.
    /// Black optic/commander view with a "switch view" prompt; suppressed entirely under `Death`.
    ViewDead,
    /// The player's own tank is knocked out: "YOU DIED / press R".
    Death,
    /// `M` opens a terrain top view; clicks choose the next respawn point. Above `Death` so players
    /// can pick before pressing R, below `Menu` so Esc can open the menu over it.
    SpawnMap,
    /// The Esc cursor-release menu (the net stand-in for SP pause; the sim never stops). Esc is the
    /// only thing that opens it — losing focus releases the cursor without declaring an overlay.
    Menu,
    /// The match's link dropped: "RECONNECTING…". Declared only in a match, while its link is down.
    ConnectStatus,
}

impl Overlay {
    /// `GlobalZIndex` draw order; wide gaps accommodate exempt siblings. See [`Overlay`] for why
    /// `Death` draws over `Menu` despite its lower priority.
    pub(crate) const fn zindex(self) -> i32 {
        match self {
            Overlay::ConnectStatus => 300,
            Overlay::Death => 200,
            Overlay::SpawnMap => 150,
            Overlay::Menu => 100,
            Overlay::ViewDead => 50,
        }
    }

    /// Settings content (`settings::ui`): above `Menu`'s backdrop, below `SpawnMap` and `Death`,
    /// keeping precedence in this ladder rather than a consumer's literal. No `OverlayNode`, active
    /// presence or backdrop of its own; the menu owns those.
    pub(crate) const MENU_CONTENT_Z: i32 = Overlay::Menu.zindex() + 10;

    /// Matchmaking's LEAVE MATCH button: above the full-screen settings wrapper to receive clicks,
    /// below layers covering the menu. Like [`Self::MENU_CONTENT_Z`], it has no `OverlayNode` and
    /// shows only while the menu owns the scrim.
    pub(crate) const MENU_ACTIONS_Z: i32 = Overlay::MENU_CONTENT_Z + 1;

    /// Optic surround with glass cut out (`sight::reticle`): below world labels, fixed HUD, graticule,
    /// intent cursor and every overlay, so overlays cover it whole. Keep its rung here, like
    /// [`Self::MENU_CONTENT_Z`], rather than in the consumer. Part of the view, not an overlay:
    /// no [`OverlayNode`], presence, input capture or scrim; the one-scrim rule never suppresses it.
    pub(crate) const SIGHT_FURNITURE_Z: i32 = -10;

    /// World-anchored labels sit above optic furniture and below every fixed HUD panel.
    pub(crate) const WORLD_LABEL_Z: i32 = Self::SIGHT_FURNITURE_Z + 1;

    /// Fixed HUD panels share the ordinary UI rung; their labels cover world-anchored labels.
    pub(crate) const FIXED_HUD_Z: i32 = 0;

    /// World-only tint ([`death_tint`]) for `net::death_screen` and the kill-cam handoff
    /// (`damage_cam::pen_cam::killcam`): below [`Self::SIGHT_FURNITURE_Z`] and all UI, which must
    /// remain untinted. No [`OverlayNode`], presence, input capture or scrim; its owner sets visibility.
    pub(crate) const DEATH_TINT_Z: i32 = Overlay::SIGHT_FURNITURE_Z - 10;

    /// Pen-cam inset (`damage_cam::pen_cam`): an exclusive fixed slot above unindexed HUD (0),
    /// below every overlay so each covers it whole. No [`OverlayNode`], presence, input capture or scrim.
    pub(crate) const PEN_CAM_CORNER_Z: i32 = 20;

    /// Battle scoreboard (`net::battle_hud`): Tab reads over `Death`'s banner, below `ConnectStatus`.
    /// No [`OverlayNode`], presence, input capture or scrim. It hides while [`input_blocked`],
    /// keeping it below menus, spawn maps and reconnect screens regardless of z.
    pub(crate) const SCOREBOARD_Z: i32 = Overlay::Death.zindex() + 10;

    /// Blocks player input and frees the cursor for menus, reconnecting (no driving until the link
    /// returns) and map clicks. `feed_action_state` zeroes the wire command so the tank coasts rather
    /// than driving blind under the map. `Death`/`ViewDead` leave it captured: R respawn and Lshift
    /// view-switch run in `PlayerInputSet`, which requires capture. Simulation continues.
    const fn blocks_input(self) -> bool {
        matches!(
            self,
            Overlay::Menu | Overlay::ConnectStatus | Overlay::SpawnMap
        )
    }

    /// Only menus are Esc-dismissable. `Death`/`ConnectStatus` must persist; `ViewDead` clears on
    /// view-switch or respawn, and `SpawnMap` on `M`. Esc opens the menu over map or death screens.
    const fn dismissable(self) -> bool {
        matches!(self, Overlay::Menu)
    }
}

/// Absolute tint spans its parent without affecting layout; `GlobalZIndex` escapes the parent's
/// stacking context to [`Overlay::DEATH_TINT_Z`].
pub(crate) fn death_tint() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(crate::ui_font::DEATH_TINT),
        GlobalZIndex(Overlay::DEATH_TINT_Z),
    )
}

/// Overlay authority: owners declare absolute presence ([`Self::declare`]); the menu latches
/// between Esc presses. Consumers derive policy; priority `Ord` makes [`Self::top`] greatest.
#[derive(Resource, Default)]
pub(crate) struct Overlays {
    active: BTreeSet<Overlay>,
    /// [`declare_screen`]'s per-frame `Flow` projection, false by default (input blocked).
    /// A flag keeps the fixed-clock `feed_action_state` reader independent of `Flow`.
    in_match: bool,
}

impl Overlays {
    /// Absolute, idempotent presence rather than a toggle: after a skipped system, the next
    /// declaration self-heals without another state to synchronize.
    pub(crate) fn declare(&mut self, overlay: Overlay, present: bool) {
        if present {
            self.active.insert(overlay);
        } else {
            self.active.remove(&overlay);
        }
    }

    /// Declare whether the client is on the match screen this frame; absolute, like [`Self::declare`].
    pub(crate) fn declare_in_match(&mut self, in_match: bool) {
        self.in_match = in_match;
    }

    pub(crate) fn contains(&self, overlay: Overlay) -> bool {
        self.active.contains(&overlay)
    }

    /// Scrim owner and top interactive layer; `None` in normal play with nothing latched.
    fn top(&self) -> Option<Overlay> {
        self.active.iter().copied().next_back()
    }
}

/// Shared input license for cursor ownership, `state::cursor_locked`/`PlayerInputSet` and
/// `feed_action_state`: unfocused, off-match (lobby/connect), or [`Overlay::blocks_input`] means
/// release the cursor and send default commands, coasting instead of holding the last input.
/// Pass live focus separately so the fixed-schedule consumer need not use a stale set value.
pub(crate) fn input_blocked(overlays: &Overlays, window_focused: bool) -> bool {
    !window_focused || !overlays.in_match || overlays.active.iter().any(|o| o.blocks_input())
}

/// Esc's dismissable top layer (only `Menu`), never `Death`/`ConnectStatus`. Return the overlay,
/// not a bool, so callers can assert which layer they dismiss.
pub(crate) fn top_dismissable(overlays: &Overlays) -> Option<Overlay> {
    overlays.top().filter(|o| o.dismissable())
}

/// Open only if no active overlay outranks it; never gate closing, so toggles can dismiss what they
/// opened. Like [`esc_menu_target`], this prevents invisible lower layers from blocking input and
/// seizing the cursor when a higher layer closes. `<=` permits self-healing re-declaration at top.
///
/// Call only in [`OverlaySet::Toggle`], after all state-driven declarations: a caller in
/// [`OverlaySet::Declare`] could miss a higher layer declared later that frame and allow that latch.
pub(crate) fn may_open(overlays: &Overlays, overlay: Overlay) -> bool {
    overlays.top().is_none_or(|top| top <= overlay)
}

/// Only the top active overlay draws backdrop and centered content. Lower layers visibility-swap,
/// never despawn, to return immediately when uncovered. `Death` outranks `ViewDead`, keeping its
/// opaque `ViewDead` black from covering "YOU DIED".
pub(crate) fn draws_scrim(overlays: &Overlays, overlay: Overlay) -> bool {
    overlays.top() == Some(overlay)
}

/// [`draws_scrim`]'s exemption: menu-over-death hides the red backdrop + "YOU DIED / press R" and
/// shows a thin, non-interactive "DEAD — respawn on menu close" line without a backdrop, at `Death`'s
/// higher z. A released cursor gates R, so "press R" would mislead; closing the menu restores the
/// full screen with `Death` as scrim owner.
pub(crate) fn death_status_line(overlays: &Overlays) -> bool {
    overlays.contains(Overlay::Death) && overlays.top() == Some(Overlay::Menu)
}

/// Resolution: defer refocus grabs until winit settles; same-frame grabs are silently dropped
/// (bevy #16237/#16238).
const REFOCUS_GRAB_FRAMES: u8 = 2;

/// [`cursor_owner`]'s focus-regain countdown while input is unblocked; `None` is idle.
#[derive(Resource, Default)]
struct RefocusGrab(Option<u8>);

/// Persistent backdrop marker: [`apply_overlay_visibility`] applies [`draws_scrim`]; `on_add`
/// stamps [`Overlay::zindex`] once. Visibility swaps never despawn nodes, so uncovered layers return
/// immediately. The exempt death status line has no marker and reconciles in `net::death_screen`.
#[derive(Component, Clone, Copy)]
#[component(on_add = stamp_overlay_zindex)]
pub(crate) struct OverlayNode(pub(crate) Overlay);

/// Queue `GlobalZIndex` from the added [`OverlayNode`]; spawn sites need not repeat the draw order.
fn stamp_overlay_zindex(mut world: DeferredWorld, HookContext { entity, .. }: HookContext) {
    let overlay = world
        .entity(entity)
        .get::<OverlayNode>()
        .expect("OverlayNode present in its own on_add hook")
        .0;
    world
        .commands()
        .entity(entity)
        .insert(GlobalZIndex(overlay.zindex()));
}

/// Declare → Toggle → Cursor, chained so toggles and cursor-derived `PlayerInputSet` gating read
/// settled declarations, never a half-written set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OverlaySet {
    /// State-driven declarations (link, tank and active-station state) run first, without gating on
    /// other overlay declarations, so their mutual order is free. Focus loss declares no menu.
    Declare,
    /// Priority-gated Esc ([`esc_menu_target`]) and `M` ([`may_open`]); see [`may_open`] for the
    /// settled-generation requirement. Mutually unordered: simultaneous Esc/`M` is legitimate either
    /// way, since a map under an explicitly opened menu remains reachable when the menu closes.
    Toggle,
    /// The single cursor owner: release when [`input_blocked`], (deferred) grab when not.
    Cursor,
}

pub(crate) fn plugin(app: &mut App) {
    crate::flow::require(app, "overlay");
    // Esc is routed once per press (`state::EscapeAction`); the menu's toggle reads the decision.
    crate::state::escape_router(app);
    app.init_resource::<Overlays>()
        .init_resource::<RefocusGrab>()
        // Match overlays cannot survive into the next screen.
        .add_systems(OnExit(Flow::InMatch), clear_overlays)
        // `feed_action_state` is the fourth link, in `FixedPreUpdate`: it reads [`input_blocked`]
        // from the previous frame's reconciled declarations.
        .configure_sets(
            Update,
            (OverlaySet::Declare, OverlaySet::Toggle, OverlaySet::Cursor).chain(),
        )
        .add_systems(Startup, spawn_menu_overlay)
        .add_systems(
            Update,
            (
                declare_screen.in_set(OverlaySet::Declare),
                // Read the settled set after Esc routing; menus exist only in matches.
                esc_toggle
                    .in_set(OverlaySet::Toggle)
                    .in_set(InMatchSystems)
                    .after(EscapeRouted),
                cursor_owner.in_set(OverlaySet::Cursor),
                // Read one settled generation, like the exempt `net::death_screen` status line,
                // which also runs after `Toggle`.
                apply_overlay_visibility.after(OverlaySet::Toggle),
            ),
        );
}

/// Bare Esc toggle for composed tests; excludes match flow, cursor ownership and Esc routing.
/// The caller supplies the router.
#[cfg(test)]
pub(crate) fn mount_bare(app: &mut App) {
    app.init_resource::<Overlays>().add_systems(
        Update,
        esc_toggle.in_set(OverlaySet::Toggle).after(EscapeRouted),
    );
}

fn clear_overlays(mut overlays: ResMut<Overlays>) {
    *overlays = Overlays::default();
}

/// Fold the screen into the set: input is licensed only on the match screen.
fn declare_screen(flow: Res<State<Flow>>, mut overlays: ResMut<Overlays>) {
    let in_match = *flow.get() == Flow::InMatch;
    if overlays.in_match != in_match {
        overlays.declare_in_match(in_match);
    }
}

/// Backdrop only: `settings::ui` supplies content at [`Overlay::MENU_CONTENT_Z`], visible while
/// the menu owns the scrim. Unlike connect/death/pause banners, it needs no centered text child
/// from `ui_font::spawn_overlay`; the settings page occupies that space. [`OverlayNode`] supplies
/// visibility reconciliation and z stamping.
fn spawn_menu_overlay(mut commands: Commands) {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Look {
        menu_scrim_srgba: crate::provenance::Sourced<(u8, u8, u8, u8)>,
    }
    let look: Look = crate::provenance::from_shipped_str(
        "assets/ui/overlay.ron",
        include_str!("../assets/ui/overlay.ron"),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let (r, g, b, a) = look.menu_scrim_srgba.0;
    commands.spawn((
        OverlayNode(Overlay::Menu),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba_u8(r, g, b, a)),
        Visibility::Hidden,
    ));
}

/// Consume [`EscapeAction::Menu`] (kill-cam runs route Esc to skip instead), declaring the menu
/// presence selected by [`esc_menu_target`]. [`input_blocked`]/cursor ownership handle the cursor.
/// Runs in [`OverlaySet::Toggle`] to read this frame's settled connect/death declarations.
fn esc_toggle(action: Res<EscapeAction>, mut overlays: ResMut<Overlays>) {
    if *action != EscapeAction::Menu {
        return;
    }
    let present = esc_menu_target(&overlays);
    overlays.declare(Overlay::Menu, present);
}

/// Pure Esc routing, testable without an app: close a top menu; under an undismissable higher
/// layer (only `ConnectStatus`), keep absent or unlatch a buried menu; otherwise open, even over
/// `Death`. Merely checking `top_dismissable(..) != Some(Menu)` would open an invisible menu under
/// reconnect and leave input unexpectedly blocked when the link returns.
fn esc_menu_target(overlays: &Overlays) -> bool {
    if top_dismissable(overlays) == Some(Overlay::Menu) {
        return false; // the menu is on top and dismissable → close it
    }
    let outranked = overlays
        .top()
        .is_some_and(|t| !t.dismissable() && t > Overlay::Menu);
    !outranked
}

// Alt-tab releases input through `input_blocked`'s live-focus term; `RefocusGrab` handles regain.
// No automatic menu: online simulation continues, and a menu left latched after focus returns
// would falsely suggest a pause. This also permits testing two clients on one machine.

/// Single cursor owner under [`input_blocked`]: release while blocked, grab otherwise. Focus-regain
/// arms [`RefocusGrab`] for [`REFOCUS_GRAB_FRAMES`] (see its winit constraint); blocking cancels
/// the countdown and keeps the cursor released, so deferred grabs fire only into unblocked states.
fn cursor_owner(
    overlays: Res<Overlays>,
    mut focus: MessageReader<WindowFocused>,
    mut refocus: ResMut<RefocusGrab>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
) {
    // Arm the deferred re-grab on a focus-REGAIN edge; a loss cancels any pending one.
    if let Some(focused) = crate::state::collapse_focus(&mut focus) {
        refocus.0 = focused.then_some(REFOCUS_GRAB_FRAMES);
    }
    let (mut window, mut cursor) = match windows.single_mut() {
        Ok(window) => window,
        Err(QuerySingleError::NoEntities(_)) => return, // Window teardown has no cursor to reconcile.
        Err(QuerySingleError::MultipleEntities(_)) => {
            panic!("overlay cursor ownership requires exactly one primary window")
        }
    };
    if input_blocked(&overlays, window.focused) {
        // Write the release edge to reconcile requested grab with OS focus loss; repeating it
        // every blocked frame would mark cursor options changed without a transition.
        if cursor.grab_mode != CursorGrabMode::None {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
        refocus.0 = None; // nothing to (re)grab while blocked
        return;
    }
    match refocus.0 {
        Some(n) if n > 1 => refocus.0 = Some(n - 1),
        Some(_) => {
            refocus.0 = None;
            crate::state::grab_now(&mut window, &mut cursor);
        }
        // `grab_now` recenters: per-frame warps would mark `Window` changed, fight OS motion-delta
        // handling and stutter mouse-look. Release paths here and in SP/focus explicitly set None,
        // so "!= Locked" identifies the release→grab edge.
        None if cursor.grab_mode != CursorGrabMode::Locked => {
            crate::state::grab_now(&mut window, &mut cursor);
        }
        None => {}
    }
}

/// Apply [`draws_scrim`] by visibility swap to every [`OverlayNode`]. Runs after
/// [`OverlaySet::Toggle`] (and therefore Declare), reading all declarers and toggles' settled set.
/// The exempt status line reconciles separately in `net::death_screen`.
fn apply_overlay_visibility(
    overlays: Res<Overlays>,
    mut nodes: Query<(&OverlayNode, &mut Visibility)>,
) {
    for (node, mut vis) in &mut nodes {
        vis.set_if_neq(if draws_scrim(&overlays, node.0) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlays(active: &[Overlay]) -> Overlays {
        Overlays {
            active: active.iter().copied().collect(),
            in_match: true,
        }
    }

    /// `top` must pick the scrim owner across sampled priority combinations, including `SpawnMap`,
    /// through the overlay set rather than numeric `Ord` literals.
    #[test]
    fn priority_orders_connect_over_menu_over_death_over_viewdead() {
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
        assert_eq!(
            overlays(&[Overlay::SpawnMap, Overlay::Death]).top(),
            Some(Overlay::SpawnMap),
            "the spawn map is opened from the death screen and draws over it",
        );
        assert_eq!(
            overlays(&[Overlay::Menu, Overlay::SpawnMap]).top(),
            Some(Overlay::Menu),
            "Esc's menu still comes out on top of the map",
        );
    }

    /// The z-order contract is fixed AND deliberately not the priority order: Death draws above Menu so
    /// the death status line (which rides Death's z) shows through the menu.
    #[test]
    fn zindex_contract_is_pinned() {
        assert_eq!(Overlay::WORLD_LABEL_Z, -9);
        assert_eq!(Overlay::FIXED_HUD_Z, 0);
        const {
            assert!(Overlay::SIGHT_FURNITURE_Z < Overlay::WORLD_LABEL_Z);
            assert!(Overlay::WORLD_LABEL_Z < Overlay::FIXED_HUD_Z);
            assert!(Overlay::FIXED_HUD_Z < Overlay::PEN_CAM_CORNER_Z);
            assert!(Overlay::PEN_CAM_CORNER_Z < Overlay::SCOREBOARD_Z);
        }
        assert_eq!(Overlay::ConnectStatus.zindex(), 300);
        assert_eq!(Overlay::Death.zindex(), 200);
        assert_eq!(Overlay::Menu.zindex(), 100);
        assert_eq!(Overlay::ViewDead.zindex(), 50);
        assert!(
            Overlay::Death.zindex() > Overlay::Menu.zindex(),
            "Death draws over Menu though Menu outranks it — for the status line",
        );
        // Settings content stays on the menu backdrop and under higher draw layers.
        assert!(
            Overlay::MENU_CONTENT_Z > Overlay::Menu.zindex(),
            "the menu's content must draw ON the menu's scrim",
        );
        assert!(
            Overlay::MENU_CONTENT_Z < Overlay::SpawnMap.zindex()
                && Overlay::MENU_CONTENT_Z < Overlay::Death.zindex(),
            "the map and the death screen must still cover the menu's content",
        );
        assert!(
            Overlay::MENU_ACTIONS_Z > Overlay::MENU_CONTENT_Z
                && Overlay::MENU_ACTIONS_Z < Overlay::SpawnMap.zindex(),
            "the menu's actions draw over its page and under whatever covers the menu",
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
        assert!(
            input_blocked(&overlays(&[Overlay::SpawnMap]), true),
            "the spawn map must block — clicking it needs the cursor released",
        );
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

    /// Menu-over-death must draw one backdrop plus status line, with R blocked; `Death` must
    /// suppress `ViewDead` entirely.
    #[test]
    fn menu_over_death_yields_exactly_one_backdrop() {
        let set = overlays(&[Overlay::Menu, Overlay::Death]);
        let backdrops = [Overlay::Menu, Overlay::Death]
            .into_iter()
            .filter(|&o| draws_scrim(&set, o))
            .count();
        assert_eq!(backdrops, 1, "one scrim total");
        assert!(draws_scrim(&set, Overlay::Menu));
        assert!(!draws_scrim(&set, Overlay::Death), "death backdrop hidden");
        assert!(death_status_line(&set), "status line shown instead");
        assert!(input_blocked(&set, true), "R gated — the menu blocks input");

        let dead = overlays(&[Overlay::Death, Overlay::ViewDead]);
        assert!(draws_scrim(&dead, Overlay::Death));
        assert!(
            !draws_scrim(&dead, Overlay::ViewDead),
            "ViewDead suppressed entirely under Death — no opaque black over YOU DIED",
        );
    }

    /// Esc must toggle a top menu, open over `Death`, and unlatch or keep absent any menu buried
    /// under `ConnectStatus`, avoiding an invisible input block after reconnect.
    #[test]
    fn esc_toggles_the_menu_normally() {
        for (latched, opens, why) in [
            (&[][..], true, "nothing up → Esc opens the menu"),
            (&[Overlay::Menu][..], false, "menu on top → Esc closes it"),
            (
                &[Overlay::Death][..],
                true,
                "Death doesn't outrank the menu — Esc opens the menu while dead",
            ),
            (
                &[Overlay::Menu, Overlay::Death][..],
                false,
                "menu over death → Esc closes it",
            ),
            (
                &[Overlay::ConnectStatus][..],
                false,
                "Esc under a connect screen latches nothing",
            ),
            (
                &[Overlay::ConnectStatus, Overlay::Menu][..],
                false,
                "Esc dismisses a menu buried beneath the connect screen",
            ),
        ] {
            assert_eq!(esc_menu_target(&overlays(latched)), opens, "{why}");
        }
    }

    /// `M` must not latch an invisible input-blocking map under menus/reconnect, yet must open
    /// over `Death` to let players pick a respawn point. Opening is gated; closing never is.
    #[test]
    fn may_open_refuses_to_latch_beneath_a_higher_overlay() {
        assert!(
            may_open(&overlays(&[]), Overlay::SpawnMap),
            "nothing latched → the map opens",
        );
        assert!(
            may_open(&overlays(&[Overlay::Death]), Overlay::SpawnMap),
            "the map outranks Death — picking a spawn from the death screen is the feature",
        );
        assert!(
            !may_open(&overlays(&[Overlay::Menu]), Overlay::SpawnMap),
            "M under the menu must not latch an invisible map",
        );
        assert!(
            !may_open(&overlays(&[Overlay::ConnectStatus]), Overlay::SpawnMap),
            "M under the connect screen must not latch an invisible map",
        );
        assert!(
            may_open(&overlays(&[Overlay::SpawnMap]), Overlay::SpawnMap),
            "already the top layer → re-declaring is always allowed",
        );
    }

    /// The composed schedule must let Esc read settled declarations. Register connect last in
    /// [`OverlaySet::Declare`]: without Declare → Toggle, `esc_toggle` could run first and latch an
    /// invisible menu. Single-threaded execution isolates registration order; presses arrive through
    /// the Esc router, as in play. Pure routing is covered above.
    #[test]
    fn esc_cannot_latch_a_menu_under_an_overlay_declared_the_same_frame() {
        let mut app = App::new();
        app.init_resource::<Overlays>();
        app.init_resource::<ButtonInput<KeyCode>>();
        crate::state::escape_router(&mut app);
        app.configure_sets(
            Update,
            (OverlaySet::Declare, OverlaySet::Toggle, OverlaySet::Cursor).chain(),
        );
        app.add_systems(
            Update,
            (
                esc_toggle.in_set(OverlaySet::Toggle).after(EscapeRouted),
                declare_connect_status.in_set(OverlaySet::Declare),
            ),
        );
        app.edit_schedule(Update, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();

        let overlays = app.world().resource::<Overlays>();
        assert!(
            overlays.contains(Overlay::ConnectStatus),
            "fixture: the connect screen must have been declared this frame",
        );
        assert!(
            !overlays.contains(Overlay::Menu),
            "Esc pressed the frame a connect screen appears must latch NO menu — the gate has to \
             read the settled declarations, not the half-written ones",
        );
    }

    /// Race fixture: declare the highest overlay like `net::client::update_connect_status` on link loss.
    fn declare_connect_status(mut overlays: ResMut<Overlays>) {
        overlays.declare(Overlay::ConnectStatus, true);
    }

    /// Cursor policy tolerates window teardown and refuses contradictory primary windows;
    /// neither case may silently skip the ownership system through a `Single` parameter.
    #[test]
    fn cursor_ownership_distinguishes_teardown_from_multiple_windows() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<Overlays>();
        world.init_resource::<RefocusGrab>();
        world.init_resource::<Messages<WindowFocused>>();
        world.run_system_once(cursor_owner).unwrap();
        for _ in 0..2 {
            world.spawn((Window::default(), PrimaryWindow));
        }
        let refusal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world.run_system_once(cursor_owner).unwrap();
        }));
        assert!(
            refusal.is_err(),
            "two primary windows must refuse cursor ownership"
        );
    }

    /// Off the match screen input is blocked whatever is latched, and the default set — the state
    /// before any screen is declared — is off it.
    #[test]
    fn off_the_match_screen_input_is_blocked() {
        assert!(input_blocked(&Overlays::default(), true));
        let off_screen = Overlays {
            in_match: false,
            ..overlays(&[Overlay::Death])
        };
        assert!(input_blocked(&off_screen, true));
        assert!(!input_blocked(&overlays(&[Overlay::Death]), true));
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
