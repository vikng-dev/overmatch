# Overlays declare presence; their consequences derive from one authority

The net client's four full-screen overlays — connect status, death, the Esc menu, view-death — are a **deep module** (`overlay.rs`): behind a two-part interface — *declare presence*, then read *pure functions of the active set* — sits everything about how overlays stack, who owns input and the cursor, who draws the one scrim, and which prompt shows. Before it, that behaviour was scattered: each overlay spawned and visibility-swapped on its own with no shared stacking (the view-death opaque black could silently occlude "YOU DIED"), and the input/cursor license was inferred independently in four hand-synced places. The decision is to give overlay presence one home and **derive** every consequence from it.

## The interface: declare, then reconcile

An owner keeps its own domain logic — the connect state machine, the death→respawn machine, the dead-crewman condition — and each frame calls `Overlays::declare(overlay, present)` **idempotently**: an absolute assertion of desired presence, not a toggle. There is no imperative push/pop stack — declare-then-reconcile, the same idiom `death_screen` already used internally, and the shape Bevy's own removal of stack states pushes toward. Its property is self-healing: a system that misses a frame's declaration drops out of the set and re-enters next frame, with nothing to keep in sync and no leaked stack entry to unwind. The one genuine latch is the Esc menu — edge-driven, with nothing in the world to re-derive it from — so its presence is toggled directly onto the set, and menu-openness has exactly one home, `Overlays.contains(Menu)`.

Everything else is a **pure function of the set**, unit-testable with no app or world:

- `input_blocked` — THE single input/cursor license. It replaced four independent inferences (the `cursor_locked` gate, `feed_action_state`'s command-zeroing, the menu open/close cursor calls, and the focus watcher). One derivation: blocked iff the window is unfocused or a latched overlay captures input.
- `top_dismissable` — Esc routing: the top overlay iff Esc may dismiss it (only ever the menu).
- `draws_scrim` — the one-scrim-total rule: only the top overlay draws its backdrop; every lower one suppresses (visibility-swap, not despawn, so it snaps back the instant the layer above closes).
- `death_status_line` — the single exemption: while the menu is drawn over a latched death, the full death screen hides and a thin status line shows through instead.

The depth is real by the deletion test: delete the module and the stacking logic, the license, the cursor rule, and the scrim arbitration reappear across all four owners — which is exactly the pre-module state the redesign found. The interface a caller learns (`declare` + four pure fns) is far smaller than the behaviour behind it (the whole stack/license/cursor machine), and the tests cross the *same* seam the owners do — the pure functions take an `Overlays` fixture and no world.

## Priority is not draw order — deliberately

Interactive **priority** (the `Ord` on `Overlay`) is distinct from **draw order** (`GlobalZIndex`). `Menu` outranks `Death` in priority — an open menu is the top interactive layer, so it owns the scrim and Esc closes it — yet `Death` draws *above* `Menu` in z, precisely so the death **status line** (exempt from the one-scrim rule, riding Death's higher z) stays legible *through* the menu backdrop. Collapse the two orders into one and you must choose between "the menu owns the scrim" and "the status line shows through"; the design wants both, so the two orders are separate facts, pinned by separate tests.

The status line exists because we never show "press R" while R cannot work: with the menu up the cursor is released and the respawn key is gated, so the full "press R" screen would be a lie — the thin "DEAD — respawn on menu close" line stands in for it until the menu closes and the full screen snaps back.

## The cursor moves only on the transition edge

`cursor_owner` is the one system that moves the cursor: `input_blocked` → release, unblocked → grab. It acts only on the grab↔release **edge** — re-issuing a grab every unblocked frame warps the cursor to centre each frame and fights the OS's own warp/motion-delta handling (the mouse-look stutter). The one wrinkle it hides is a winit quirk: a grab issued the same frame focus returns is silently dropped (bevy #16237/#16238), so a focus-regain arms a short deferral (`REFOCUS_GRAB_FRAMES`) and the grab waits a couple of frames. That deferral is an *implementation detail behind the interface* — an owner declaring presence never sees it, which is the depth paying off: one hard thing, hidden once.

## Scope: view and input-routing only

The module is net-client-only (mounted by `NetClientPlugin`, never single-player) and runs in `Update`, outside `GameplaySet` and outside the fixed/rollback schedule. **Nothing here is rollback-tracked**: overlays are presentation, and the sim keeps ticking under every one of them — there is no online pause, because a frozen predicting client desyncs from a server that keeps ticking ([[0014-sim-view-split]]: overlays live strictly on the view plane). Single-player's real pause (`state::client_plugin`) is a separate mechanism this decision does not touch.

## The normative rule

**A new full-screen overlay declares into `Overlays` and derives its visuals and input from the pure functions — it never infers the license locally.** Add the variant at its priority rank, give it a `zindex`, say whether it `blocks_input` and is `dismissable`, and have its owner `declare` presence each frame; the scrim, the cursor, Esc routing, and the input license then follow for free. Re-introducing a local `cursor.grab_mode = None` or a private "is a menu open" bool is the regression this module exists to prevent — the license has one derivation, and a fifth copy is how the four-way desync came back.

## What this ADR does not say

It is not a general UI-stack or windowing system — it governs exactly the client's full-screen, mutually-arbitrating overlays, four today. HUD elements, the crew bar, the toast, the reticles are not overlays: they do not capture input, own the scrim, or stack, so they stay their owners' business.

Nor does it move the overlays' *domain logic* into the module. The connect retry machine, the respawn round-trip, the dead-crewman test each stay with their owner; the module owns only presence and its consequences. Declaring is cheap precisely because the hard part — deciding *whether* you should be present — stays where the knowledge already is.

## Related

- [[0014-sim-view-split]] — overlays are view-plane only, never rollback-tracked; the sim ticks under them exactly as sim truth stays separate from presentation there.
- [[0016-replicate-causes-derive-consequences]] — the same shape one domain over: declare (or replicate) the one fact, then derive every consequence rather than hand-syncing copies. The death overlay's own presence derives from the `TankKnockedOut` that 0016 derives from `NetCrew`.
