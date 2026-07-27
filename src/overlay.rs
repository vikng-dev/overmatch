//! Net-client overlay state and derived input/cursor policy.
//!
//! Invariant: [`Overlays`] is the single source of truth for overlay precedence, input blocking, and
//! scrim ownership. This module is view/input routing only; it never pauses rollback simulation.

use std::collections::BTreeSet;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};

/// The client's full-screen overlays, ordered by PRIORITY via the derived `Ord`. The variants are
/// declared LOW→HIGH, so the greatest is the top layer: `ConnectStatus` is the maximum (a connect /
/// reconnect takes over everything), then `Menu` (an open menu is the top INTERACTIVE layer — Esc
/// closes it, R can't respawn under it), then `SpawnMap`, then `Death`, then `ViewDead` at the
/// bottom.
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
    /// The `M` spawn map: a top view of the terrain whose click picks where the NEXT respawn lands.
    /// Above `Death` because it is opened FROM the death screen (pick where to come back, then press
    /// R), below `Menu` so Esc still reaches the menu over it.
    SpawnMap,
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
            Overlay::SpawnMap => 150,
            Overlay::Menu => 100,
            Overlay::ViewDead => 50,
        }
    }

    /// The z-layer for CONTENT that rides the menu's scrim without being a scrim: today, exactly the
    /// settings page (`settings::ui`). It is a rung of THIS ladder rather than a literal in that
    /// module, so the one-scrim precedence stays readable in one place — above [`Overlay::Menu`]'s
    /// backdrop (100) so it draws ON it, and below [`Overlay::SpawnMap`] (150) and [`Overlay::Death`]
    /// (200) so a map or a death screen still covers it exactly as they cover the menu.
    ///
    /// It carries no `OverlayNode`: it is not part of the active set, declares no presence, and owns
    /// no backdrop. The menu owns all of that; this is only where the menu's content draws.
    pub(crate) const MENU_CONTENT_Z: i32 = Overlay::Menu.zindex() + 10;

    /// Whether this overlay CAPTURES input while active — play stops and the cursor frees. The menu is
    /// the obvious one; connect status too (there is no tank to drive until the link is up, and a
    /// visible cursor over "CONNECTING…" is the honest state). Death and view-death do NOT block: the
    /// respawn key and the Lshift view switch both ride `PlayerInputSet`, which needs the cursor
    /// captured — the player is still "in" the tank, just with a dead station.
    /// The spawn map blocks too, and must: picking a point needs a visible, free cursor, which is
    /// exactly what a blocking overlay buys (the cursor owner releases, `feed_action_state` zeroes
    /// the wire command, so the tank coasts to a stop instead of driving blind under the map).
    const fn blocks_input(self) -> bool {
        matches!(
            self,
            Overlay::Menu | Overlay::ConnectStatus | Overlay::SpawnMap
        )
    }

    /// Whether Esc may dismiss this overlay. Only the menu: Death and ConnectStatus are NEVER
    /// Esc-dismissed (you don't Esc away being dead or disconnected), ViewDead clears on a crew
    /// switch or a respawn, never on Esc, and the SpawnMap is closed by its own `M` toggle (so Esc
    /// over the map opens the menu, exactly as it does over the death screen).
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

/// Whether `overlay` may be OPENED right now: only when nothing latched OUTRANKS it. CLOSING is
/// never gated — an owner may always withdraw its own declaration, so a key that toggles an overlay
/// stays able to dismiss what it opened.
///
/// This is the open-side generalization of [`esc_menu_target`]'s "never latch beneath an overlay that
/// outranks you" rule. Without it, a toggle key pressed under a higher layer latches an overlay the
/// one-scrim rule immediately hides — invisible, yet counted by [`input_blocked`] and ready to seize
/// the cursor the instant the layer above closes. `<=` rather than `<`: an overlay that is already the
/// top layer trivially may re-open, and re-opening is what a self-healing declaration does.
///
/// The gate is only as good as the SET GENERATION it reads, so every caller must run in
/// [`OverlaySet::Toggle`] — after the state-driven declarers have all written this frame. Read from
/// inside [`OverlaySet::Declare`] it can miss a higher overlay declared later in the same frame and
/// wave through exactly the invisible latch it exists to prevent.
pub(crate) fn may_open(overlays: &Overlays, overlay: Overlay) -> bool {
    overlays.top().is_none_or(|top| top <= overlay)
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

/// Marks a full-screen overlay backdrop node as `overlay`, so ONE generic reconciler
/// ([`apply_overlay_visibility`]) drives its scrim visibility from the shared [`draws_scrim`] rule and
/// its [`Overlay::zindex`] is stamped exactly once (the `on_add` hook below). This retires the three
/// hand-copied `set_if_neq(draws_scrim …)` blocks (menu / death backdrop / connect) and the four
/// scattered `GlobalZIndex(Overlay::X.zindex())` stamps into this single marker. Every marked node is
/// persistent (spawned once, visibility-swapped, never despawned for the swap) so it snaps back the
/// instant the layer above closes. The death STATUS LINE is the sole exempt sibling — it is NOT an
/// [`OverlayNode`] and keeps its own tiny reconciler in `net::death_screen`.
#[derive(Component, Clone, Copy)]
#[component(on_add = stamp_overlay_zindex)]
pub(crate) struct OverlayNode(pub(crate) Overlay);

/// Stamp the overlay's one-scrim `GlobalZIndex` the instant its [`OverlayNode`] marker lands, so no
/// spawn site has to remember the z — the draw order is a pure function of which overlay it is.
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

/// The pinned intra-frame order for the overlay authority: state-driven owners DECLARE presence, THEN
/// the priority-gated key TOGGLES read that settled set, THEN the cursor owner derives the license and
/// moves the cursor. Chained so each stage — and, transitively, the `PlayerInputSet` gate the cursor
/// drives — reads a fully-reconciled set rather than a half-written one.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OverlaySet {
    /// State-driven owners declare desired overlay presence into [`Overlays`]: the connect screen from
    /// the link state, the death screen from the crew, view-death from the active station, the menu
    /// from a focus loss. None of them READS the set, so their order among themselves is free. Runs
    /// first.
    Declare,
    /// The key toggles whose OPEN side is gated on priority: Esc's menu ([`esc_menu_target`]) and the
    /// spawn map's `M` ([`may_open`]). Separated from [`OverlaySet::Declare`] — and ordered after it —
    /// precisely BECAUSE their gate reads the set. Sharing the declare stage, a toggle could read a
    /// generation in which a higher overlay declared later that same frame was still missing, pass the
    /// gate, and latch an overlay the one-scrim rule then hides: invisible, yet counted by
    /// [`input_blocked`], and seizing the cursor the instant the layer above closes. Toggles among
    /// themselves are unordered — Esc and `M` in one frame land on a legitimate state either way (a
    /// map under a menu the player opened is reachable again on menu close).
    Toggle,
    /// The single cursor owner: release when [`input_blocked`], (deferred) grab when not.
    Cursor,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Overlays>()
        .init_resource::<RefocusGrab>()
        // Declare presence → gated toggles → cursor owner. The wire-command zeroing
        // (`feed_action_state`) is the fourth conceptual link but lives in `FixedPreUpdate`; it reads
        // the same [`input_blocked`] authority off the set reconciled by the previous frame's
        // declarations, exactly as the old zeroing read last frame's `menu.open`.
        .configure_sets(
            Update,
            (OverlaySet::Declare, OverlaySet::Toggle, OverlaySet::Cursor).chain(),
        )
        .add_systems(Startup, spawn_menu_overlay)
        .add_systems(
            Update,
            (
                // Focus loss is state, not a gated toggle: it declares the menu present unconditionally
                // and reads nothing, so it belongs with the other owners.
                focus_declare.in_set(OverlaySet::Declare),
                // Esc's routing READS the set, so it runs in the toggle stage.
                esc_toggle.in_set(OverlaySet::Toggle),
                cursor_owner.in_set(OverlaySet::Cursor),
                // The ONE scrim reconciler for every marked overlay node. Ordered after every writer —
                // the connect / death / view-death owners in `Declare` AND the Esc / `M` toggles in
                // `Toggle` — so it reads one fully-reconciled generation of the set, the fix for the
                // cross-overlay one-frame skew. The death status line is the lone exemption and runs in
                // `net::death_screen`, also after `Toggle`.
                apply_overlay_visibility.after(OverlaySet::Toggle),
            ),
        );
}

/// Spawn the Esc menu backdrop once, hidden. [`apply_overlay_visibility`] reveals it whenever the
/// menu is the scrim owner, and the `OverlayNode` hook stamps its one-scrim `GlobalZIndex`.
///
/// The menu is a BACKDROP ONLY — it carries no banner of its own. Its CONTENT is `settings::ui`'s
/// page, which draws on its own rung of this module's z ladder ([`Overlay::MENU_CONTENT_Z`]) and is
/// visible exactly while this overlay owns the scrim. That is why this no longer goes through
/// `ui_font::spawn_overlay` (which always spawns a centred text child) as the connect / death /
/// pause overlays still do: those three ARE a line of text, and this one is a surface behind one.
/// The placeholder "MENU / Esc to close" banner it used to render is what the settings page
/// replaced.
fn spawn_menu_overlay(mut commands: Commands) {
    commands.spawn((
        OverlayNode(Overlay::Menu),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        Visibility::Hidden,
    ));
}

/// Esc toggles the menu presence directly on [`Overlays`] — the menu's one home, retiring the old
/// `MenuOverlay{open}`. The routing is the pure [`esc_menu_target`]; the cursor follows from
/// [`input_blocked`] via the cursor owner — this system moves no cursor itself.
///
/// Runs in [`OverlaySet::Toggle`], after every state-driven declarer: the routing reads the set, so it
/// needs this frame's connect / death declarations already in it.
fn esc_toggle(keys: Res<ButtonInput<KeyCode>>, mut overlays: ResMut<Overlays>) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let present = esc_menu_target(&overlays);
    overlays.declare(Overlay::Menu, present);
}

/// The menu presence an Esc press should declare, given the current set — pure so the routing is
/// unit-testable without an app. Three cases:
///   - the menu is the top DISMISSABLE layer → Esc CLOSES it (`false`);
///   - an UNDISMISSABLE overlay outranks the menu (only a connect screen can) → Esc never OPENS a menu
///     that would latch invisibly beneath it, and UNLATCHES one already buried there (dismiss intent),
///     so both cases resolve to `false` — Esc under a connect screen with no menu is a no-op;
///   - otherwise nothing latched outranks the menu → Esc OPENS it (`true`), even over the death screen.
///
/// This fixes the one-way latch: the old `top_dismissable(..) != Some(Menu)` opened the menu on EVERY
/// Esc while a connect screen was up (undismissable, always top), so the player reconnected into a
/// surprise input-blocking menu that no Esc could then remove.
fn esc_menu_target(overlays: &Overlays) -> bool {
    if top_dismissable(overlays) == Some(Overlay::Menu) {
        return false; // the menu is on top and dismissable → close it
    }
    // Menu absent, or buried under an undismissable overlay that outranks it. If something outranks the
    // menu (a connect screen), Esc must not open one and unlatches any buried; otherwise open.
    let outranked = overlays
        .top()
        .is_some_and(|t| !t.dismissable() && t > Overlay::Menu);
    !outranked
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
        // Act only on the grab→release EDGE: `grab_mode` is our requested state (the OS-truth
        // divergence on focus loss is repaired by writing it here), and re-writing it every blocked
        // frame would mark the cursor options changed each frame for no transition.
        if cursor.grab_mode != CursorGrabMode::None {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
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
        // Grab only on the release→grab EDGE. `grab_now` warps the cursor to centre; issuing that
        // warp every unblocked frame (instead of on the transition) marks the window changed each
        // frame and fights the OS's warp/motion-delta handling — per-frame re-centering is exactly
        // the mouse-look stutter to avoid. The release paths above (and the SP/focus ones) all write
        // `grab_mode = None` explicitly, so "!= Locked" IS the transition edge.
        None if cursor.grab_mode != CursorGrabMode::Locked => {
            crate::state::grab_now(&mut window, &mut cursor);
        }
        None => {}
    }
}

/// The ONE reconciler of the one-scrim rule for every [`OverlayNode`]: each marked backdrop is
/// `Visible` only while its overlay owns the scrim ([`draws_scrim`] — the top of the reconciled set)
/// and `Hidden` otherwise (visibility-swap, never despawn, so it snaps back the instant the layer above
/// closes). Replaces the three hand-copied per-overlay visibility systems (menu / death backdrop /
/// connect) with one; the death status line is the lone exemption (`net::death_screen`). Runs after
/// [`OverlaySet::Toggle`] — hence after [`OverlaySet::Declare`] too — so every owner, the connect /
/// death / view-death declarers and the Esc / `M` toggles alike, has written its presence into the
/// SAME set generation this reads.
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

    /// Build an `Overlays` with exactly the given overlays latched — the fixture for the pure-function
    /// tests (no app, no world).
    fn overlays(active: &[Overlay]) -> Overlays {
        Overlays {
            active: active.iter().copied().collect(),
        }
    }

    /// `top` picks the right scrim owner for every combination — ConnectStatus > Menu > Death >
    /// ViewDead — through the public overlay set, not by restating `Ord` literals.
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
        assert_eq!(Overlay::ConnectStatus.zindex(), 300);
        assert_eq!(Overlay::Death.zindex(), 200);
        assert_eq!(Overlay::Menu.zindex(), 100);
        assert_eq!(Overlay::ViewDead.zindex(), 50);
        assert!(
            Overlay::Death.zindex() > Overlay::Menu.zindex(),
            "Death draws over Menu though Menu outranks it — for the status line",
        );
        // The menu's CONTENT rung (the settings page) is bracketed by the ladder, not floating
        // beside it: on the menu's backdrop, under everything that outranks the menu in draw order.
        assert!(
            Overlay::MENU_CONTENT_Z > Overlay::Menu.zindex(),
            "the menu's content must draw ON the menu's scrim",
        );
        assert!(
            Overlay::MENU_CONTENT_Z < Overlay::SpawnMap.zindex()
                && Overlay::MENU_CONTENT_Z < Overlay::Death.zindex(),
            "the map and the death screen must still cover the menu's content",
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

    /// Esc under a connect screen is a NO-OP: it must not latch an invisible, input-blocking menu that
    /// the player then reconnects into (the one-way-latch bug). With no menu present and the connect
    /// screen on top, the routing keeps the menu absent.
    #[test]
    fn esc_under_connect_is_a_no_op() {
        assert!(
            !esc_menu_target(&overlays(&[Overlay::ConnectStatus])),
            "Esc under a connect screen latches nothing",
        );
    }

    /// A menu latched before a reconnect took over is UNLATCHED by Esc (dismiss intent), so it isn't
    /// waiting to block input the instant the link returns.
    #[test]
    fn esc_unlatches_a_menu_buried_under_connect() {
        assert!(
            !esc_menu_target(&overlays(&[Overlay::ConnectStatus, Overlay::Menu])),
            "Esc dismisses a menu buried beneath the connect screen",
        );
    }

    /// The ordinary toggle is unchanged: Esc opens the menu when nothing latched outranks it (including
    /// over the death screen) and closes it when it is on top.
    #[test]
    fn esc_toggles_the_menu_normally() {
        assert!(
            esc_menu_target(&overlays(&[])),
            "nothing up → Esc opens the menu",
        );
        assert!(
            !esc_menu_target(&overlays(&[Overlay::Menu])),
            "menu on top → Esc closes it",
        );
        assert!(
            esc_menu_target(&overlays(&[Overlay::Death])),
            "Death doesn't outrank the menu — Esc opens the menu while dead",
        );
        assert!(
            !esc_menu_target(&overlays(&[Overlay::Menu, Overlay::Death])),
            "menu over death → Esc closes it",
        );
    }

    /// Opening is gated on priority, closing never is. The spawn map is the live case: `M` under the
    /// menu or a connect screen must latch NOTHING (an invisible input-blocking map that grabs the
    /// cursor when the layer above closes), while `M` over the death screen — which the map outranks,
    /// and is the whole point of the feature — still opens.
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

    /// The SCHEDULED half of the gate — the pure `esc_menu_target` tests above prove the routing, this
    /// proves the toggle READS a settled set. A declarer that latches the connect screen is registered
    /// in [`OverlaySet::Declare`] and, deliberately, LAST — so absent the `Declare → Toggle` chain the
    /// schedule would run `esc_toggle` first, against a set that does not have the connect screen in it
    /// yet, and Esc would latch an invisible input-blocking menu under it (the same-frame declare race).
    /// The executor is pinned single-threaded so registration order is the only thing the chain has to
    /// beat.
    #[test]
    fn esc_cannot_latch_a_menu_under_an_overlay_declared_the_same_frame() {
        let mut app = App::new();
        app.init_resource::<Overlays>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.configure_sets(
            Update,
            (OverlaySet::Declare, OverlaySet::Toggle, OverlaySet::Cursor).chain(),
        );
        app.add_systems(
            Update,
            (
                esc_toggle.in_set(OverlaySet::Toggle),
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

    /// The declarer half of the race fixture: an ordinary state-driven owner latching the highest
    /// overlay, exactly like `net::client::update_connect_status` does from the live link state.
    fn declare_connect_status(mut overlays: ResMut<Overlays>) {
        overlays.declare(Overlay::ConnectStatus, true);
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
