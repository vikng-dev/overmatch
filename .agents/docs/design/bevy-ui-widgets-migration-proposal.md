# Proposal: move settings controls onto Bevy's headless widgets

**Status:** proposal only. This is not an ADR and authorizes no implementation.

**Decision requested:** whether to replace the settings page's hand-written interaction semantics
with Bevy 0.19's headless `Button` and `Slider` widgets, while retaining Overmatch's layout,
styling, `Settings` source of truth, capability gates and save policy.

Adoption is not a refactor. Pointer timing, drag behavior, disabled behavior and focus are observable
player behavior, so the owner should choose the migration and judge it in play.

## Evidence and scope

MEASURED 2026-07-29 on `chore/simplify`: `src/` has zero references to
`bevy_ui_widgets`, `bevy_core_widgets`, a `Core*` widget or `Interaction`. The settings page already
uses Bevy picking for hit testing and bubbling, but implements the controls above that substrate
itself:

- `Row::ORDER` contains MEASURED 9 rows. DERIVED from `Row::kind`, 6 are steppers, represented by
  12 separately spawned `ArrowGlyph` click targets, and 3 are sliders.
- `page_pressed` interprets `Pointer<Press>` for both arrows and slider tracks.
- `SliderDrag`, `ActiveDrag`, `mouse_input`, `scrub` and `track_fraction` jointly own drag capture,
  raw mouse-button state, release-time persistence, grab-time geometry and physical-pixel mapping.
- `Selection`, `keyboard_input`, `Hovered` and `refresh_page` jointly own row navigation,
  hover-selection, selected styling and the disabled frame-cap row.
- The slider fill is the value visualization. There is no thumb.

This proposal is limited to `src/settings/ui.rs`. `net::spawn_map` also interprets a pointer over UI,
but it is a domain-specific map-to-world coordinate picker, not a button, slider or list. Moving it
onto a generic widget would not delete its UV and world-space policy, so it stays out.

## What Bevy 0.19 supplies

VERIFIED against the locked `bevy_ui_widgets 0.19.0` source:

- `Button` owns pressed state, cancel/drag-end cleanup, disabled gating, keyboard activation and an
  accessibility button role. It emits `Activate`. `ActivateOnPress` preserves the settings arrows'
  current pointer-down activation if that timing is retained.
- `Slider` owns pointer and keyboard input, drag state, range clamping, optional snapping/rounding,
  disabled gating and an accessibility slider role. Its supporting types are `SliderValue`,
  `SliderRange`, `SliderStep`, `SliderPrecision`, `SliderDragState`, optional `SliderThumb`, and
  `ValueChange<f32>`. `TrackClick::Snap` is the nearest upstream policy to the current
  click-to-set track.
- `ValueChange<f32>::is_final` distinguishes an in-progress drag from its final drag event. State
  remains externally owned unless the app deliberately installs `slider_self_update`, which fits
  the existing rule that `Settings` is truth and the page is a view/editor of it.
- `UiWidgetsPlugins` is already included by Bevy 0.19's `DefaultPlugins` when the default
  `bevy_ui_widgets` feature is enabled. This project's `bevy` dependency uses default features, so
  adoption adds no dependency and should not add `UiWidgetsPlugins` again.
- Bevy 0.19 also supplies `Checkbox`, `RadioButton`/`RadioGroup`, `ListBox`, `Menu`,
  `ScrollArea`/`Scrollbar`, and input handling for the new `EditableText`. None replaces an
  existing settings control honestly: the enum rows expose previous/next buttons rather than all
  choices at once, and the page's selected row is navigation state rather than a selected value.

The 0.18 `Core*` names are stale for this pin. In 0.19 the public widget names are unprefixed
(`Button`, `Slider`, `SliderDragState`, and so on).

`bevy_feathers` is explicitly excluded. Its own crate documentation says it targets editors and
inspectors and is deliberately not intended for game UI. The headless widgets have no inherent
style, so Overmatch can keep its shipped Barlow Condensed typography and visual language.

Versioned references:

- [`bevy_ui_widgets 0.19.0`](https://docs.rs/bevy_ui_widgets/0.19.0/bevy_ui_widgets/)
- [`Slider` 0.19.0](https://docs.rs/bevy_ui_widgets/0.19.0/bevy_ui_widgets/struct.Slider.html)
- [`Button` 0.19.0](https://docs.rs/bevy_ui_widgets/0.19.0/bevy_ui_widgets/struct.Button.html)
- [`bevy_feathers 0.19.0`](https://docs.rs/bevy_feathers/0.19.0/bevy_feathers/)

## Proposed ownership after migration

Keep the existing `Row` model and `Settings` as the deep interface. Bevy should own generic
interaction; Overmatch should continue to own game policy:

| Concern | Owner after migration |
|---|---|
| Pointer press, release, cancel and drag mechanics | Bevy `Button` / `Slider` |
| Pressed and disabled interaction state | Bevy `Pressed` / `InteractionDisabled` |
| Range, step and drag value event | Bevy `SliderRange` / `SliderStep` / `ValueChange<f32>` |
| Which settings rows exist and their order | `Row::ORDER` |
| Capability-gated values and frame-cap availability | `PresentCaps` / `Row::enabled` |
| Discrete ladder-to-setting conversion | `Row` and the setting newtypes |
| Rendered fill, colors, labels and Barlow Condensed fonts | `settings::ui` |
| Apply and persistence policy | `Settings`, `ApplySettings`, `SaveSettings` |
| Pause/menu visibility and overlay priority | existing state and overlay modules |

For the DERIVED 3 sliders, expose the discrete stop index as the widget value rather than exposing
the underlying setting's numeric representation. DERIVED from that stop-index model: use a
`SliderRange` from the first through last stop, `SliderStep(1.0)` and `SliderPrecision(0)`, then map
the emitted stop back through the existing setting newtypes. That preserves the invariant that mouse
and keyboard can reach exactly the same rungs, including render scale's authored ladder.

Do not add a `SliderThumb` in the first slice. It is optional upstream, and retaining the fill-only
visual avoids mixing an interaction migration with a new look or a changed effective track length.

Do not install `TabNavigationPlugin` or add `TabIndex` in the first slice. `DefaultPlugins` already
contains input focus and dispatch, but the official 0.19 widget example adds tab navigation
separately. Keyboard focus and Tab/Enter/Space behavior would be a useful accessibility improvement,
but it is another player-visible input design and should be judged separately from replacing pointer
mechanics.

## Player-visible differences to decide and measure

### Buttons

Today an arrow changes its value on `Pointer<Press>`. A default Bevy `Button` activates through
`Activate` after click completion and cancels when the gesture becomes a drag or is cancelled.
There are two honest choices:

1. Add `ActivateOnPress` and preserve the current down-edge timing.
2. Adopt release/click activation, gaining conventional cancel-by-drag behavior.

The first migration slice should preserve timing with `ActivateOnPress`; release activation can be
playtested later as an explicit behavior change.

### Sliders

The current slider:

- jumps to the pressed track position immediately;
- continues scrubbing outside the track while the primary button is held;
- snaps to discrete stops;
- freezes the track rectangle at grab time;
- saves once on release or when the page closes during a drag.

Bevy's closest policy is `TrackClick::Snap`, but its 0.19 implementation is not behavior-identical:

- track press emits a non-final `ValueChange`;
- a drag emits cumulative-distance changes and a final change at drag end;
- a press and release with no drag does not get its finality from `DragEnd`;
- drag conversion is owned by the widget and uses the live computed node, render-target scale,
  `UiScale` and transform rather than Overmatch's frozen physical-pixel rectangle.

The migration therefore still needs a small persistence adapter: mark the setting dirty on a
non-final value and emit one `SaveSettings` on the corresponding release, cancel or page close.
That adapter owns no coordinates or value math. Whether a cancelled gesture persists its last live
preview must be pinned before implementation.

The UI-scale slider is the high-risk case. Its own value changes the layout it is being dragged on.
The existing grab-time rectangle was introduced after MEASURED stationary-hand jitter/run-away.
Bevy's conversion may correctly cancel those scale factors, but source inspection is not proof of
feel. The migration is rejected unless the existing frozen-rect regression is replaced by an
equivalent widget-level test and a Retina playtest shows a stationary pointer cannot move the value.

### Disabled and selected rows

`InteractionDisabled` should become the interaction gate on the frame-cap widgets, but the existing
`Row::enabled` remains the single policy fact. Styling must continue to derive from that same fact.
The current Up/Down walk saturates and skips disabled rows; Bevy's `ListBox` navigation wraps, so
replacing `Selection` with `ListBox` in this migration would change behavior and is out of scope.

## Migration order

1. **Pin the current behavior.** Retain the existing pure stepping, slider round-trip, disabled-row,
   physical-pixel and picked-press tests. Add event-level pins for arrow activation timing, a
   click-without-drag save, drag-outside-track, cancel, and page-close-during-drag before deleting
   the mechanisms they exercise.
2. **Move only the arrow affordances to `Button`.** Add `Button`, `ActivateOnPress` and
   `InteractionDisabled`; translate `Activate` into the existing `change_row`. Keep `Selection`,
   hover styling and all slider code. This is the lowest-risk playtest slice.
3. **Move Render Scale to `Slider`.** It has no conditional enable gate and does not resize the UI.
   Keep the existing fill visual and external `Settings` ownership. Prove click, scrub, snapping and
   one-save-per-gesture behavior.
4. **Move Frame Cap to `Slider`.** Exercise `InteractionDisabled` against every effective-vsync
   state, including unavailable capability probes.
5. **Move UI Scale last.** Run the scale matrix below and delete `SliderDrag`, `ActiveDrag`,
   `scrub`, `track_fraction`, the raw `ButtonInput<MouseButton>` reader and direct
   `physical_cursor_position()` reads only after the widget owns the DERIVED 3 sliders.
6. **Decide focus separately.** If keyboard focus is wanted, add tab navigation and visible focus
   treatment in its own behavior change, with controller/keyboard playtesting.

Each step is independently revertible. Do not carry both old and new pointer mechanisms on the
same control: replace the old mechanism in the slice so duplicate activation is impossible.

## Acceptance matrix

DERIVED from the Retina target and the shipped ladder endpoints: the acceptance matrix should cover
window scale factors 1x and 2x and UI scale rungs 75%, 100% and 150%. For each slider:

- press at the left endpoint, midpoint and right endpoint;
- press and release without moving;
- drag across at least two stops;
- drag beyond both ends;
- hold the pointer stationary while the UI-scale value changes;
- close the page while held;
- lose pointer focus or cancel while held;
- verify exactly one settings-file save per completed gesture;
- verify Left/Right still reaches the same rungs and saturates at both ends.

For stepper buttons:

- verify one activation per primary press;
- verify a press that becomes a drag follows the chosen activation policy;
- verify the disabled frame-cap arrows neither activate nor light;
- verify hovering a child affordance still selects and describes its row.

The visual comparison should show unchanged card geometry, fonts, colors, value wells, slider fills
and hover/selected states. Any thumb, focus ring, transition or sound is a separate design choice.

## Risk

| Risk | Level | Containment |
|---|---|---|
| UI-scale drag feeds back through live layout | High | migrate last; scale matrix plus Retina playtest |
| Click-only `TrackClick::Snap` has no drag-final event | High | explicit dirty/release persistence adapter and event test |
| Duplicate activation while old and new observers overlap | High | replace per control; never layer mechanisms |
| Disabled policy and widget marker drift | Medium | one `Row::enabled` writer for `InteractionDisabled` and styling |
| Pointer-down versus release activation feels different | Medium | use `ActivateOnPress` first; change only by owner choice |
| Experimental widget API changes on the next Bevy bump | Medium | keep the adaptation local to `settings::ui`; trip through versioned tests |
| Focus changes steal or reinterpret existing keys | Medium | defer tab navigation and focus to a separate decision |
| Visual style drifts toward tooling UI | Low | use headless widgets only; exclude `bevy_feathers` |

## Recommendation

Approve a tracer-bullet migration of the DERIVED 12 arrow affordances, preserving pointer-down
timing with `ActivateOnPress`. If that feels unchanged and the diff deletes the custom press
interpretation, trial Render Scale as the first `Slider`. Do not approve the remaining sliders until
the click-without-drag persistence seam and the UI-scale feedback test are demonstrated.
