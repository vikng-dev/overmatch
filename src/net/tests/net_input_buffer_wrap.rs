//! UPSTREAM TRIPWIRE for the input-message range wrap that `src/net/client.rs`'s
//! `drop_stranded_input_buffer` guard compensates for (the 2026-07-11 §7 connect-hang fix; full
//! decode in `.agents/docs/design/sim-divergence-and-determinism.md` §7).
//!
//! The mechanism, as lightyear 0.28 had it (`lightyear_inputs_native` input_message.rs):
//! `build_from_input_buffer` computed `buffer_end = (end_tick + 1 - buffer_start_tick) as usize`
//! and looped `buffer_start..buffer_end`. `Tick - Tick` is a signed `i32`, so whenever the buffer's
//! `start_tick` led `end_tick` by ≥ 2 the difference went negative, the `as usize` sign-extended to
//! ~2^64, and the loop became an unbounded allocating spin — the load-gated connect hang (silent
//! wedge, RSS balloon, eventual SIGKILL). The strand itself is persistent because
//! `InputBuffer::set_raw` refuses writes below `start_tick`, so a backward connect-window resync
//! leaves the buffer ahead of the timeline forever.
//!
//! Lightyear 0.30 BOUNDS the encoder (`(end - start).saturating_add(1).max(0)` pre-sizing and a
//! `tick <= end_tick` loop), so the call is now safe to make here — and the bounded reproduction
//! below shows it is still WRONG: an inverted range encodes one state, the value stored at the
//! future `start_tick`, labelled as `end_tick`. The guard stays until that stops being true. The
//! two enablers are pinned beside it: a still-negative `Tick` difference, and `set_raw`'s refusal
//! to re-anchor below `start_tick`. If the reproduction's pin fails after an upgrade, re-read the
//! encoder: the guard in `src/net/client.rs` may be retirable.

use bevy::prelude::Reflect;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::InputBuffer;
use lightyear_inputs::input_message::ActionStateSequence;
use lightyear_inputs_native::prelude::{ActionState, NativeStateSequence};
use serde::{Deserialize, Serialize};

/// Minimal action for this input buffer — the shape of our `TankCommand` without dragging the
/// game's input type into the pin.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default, Reflect)]
struct TestAction(u8);

type TestBuffer = InputBuffer<ActionState<TestAction>, TestAction>;

/// Enabler #1: `Tick` subtraction is a signed `i32` difference (0.30 clamps it to the `i32` range,
/// which changes nothing at these magnitudes). The wedge needs the negative value; if upstream
/// saturates at zero the inverted range cannot arise.
#[test]
fn tick_subtraction_still_goes_negative() {
    let d: i32 = Tick(20) - Tick(313);
    assert_eq!(
        d, -293,
        "lightyear changed Tick - Tick semantics (expected plain i32 difference -293, got {d}) — \
         the §7 wrap may be closed upstream; re-verify with a loaded connect batch and retire \
         drop_stranded_input_buffer in src/net/client.rs (see module doc)"
    );
}

/// Enabler #2: `set_raw` refuses writes below `start_tick`, which is what makes a stranded
/// buffer PERSISTENT after a backward resync (the timeline drops, the buffer can't follow).
#[test]
fn set_raw_still_refuses_lower_ticks() {
    let mut buffer = TestBuffer::default();
    buffer.set_raw(Tick(313), Some(ActionState(TestAction(1))));
    buffer.set_raw(Tick(20), Some(ActionState(TestAction(2))));
    assert_eq!(
        buffer.start_tick,
        Some(Tick(313)),
        "lightyear's InputBuffer::set_raw now accepts/re-anchors below start_tick — re-evaluate \
         the production scheduling with a bounded reproduction before retiring the §7 guard in \
         src/net/client.rs (see module doc)"
    );
}

/// The bounded reproduction: a buffer stranded at tick 313 asked to encode up to tick 20 — the
/// inverted range the guard clears. 0.30 returns promptly with ONE state, the value authored for
/// tick 313, which the receiver will file under tick 20. Fires when upstream changes the answer
/// (refuses the range, or encodes nothing) — the signal that the guard may be retirable.
#[test]
fn an_inverted_range_encodes_one_mislabeled_state() {
    let mut buffer = TestBuffer::default();
    buffer.set(Tick(313), ActionState(TestAction(7)));
    let sequence = NativeStateSequence::<TestAction>::build_from_input_buffer(&buffer, 5, Tick(20));
    let len = sequence.as_ref().map(ActionStateSequence::len);
    assert_eq!(
        len,
        Some(1),
        "lightyear's encoder changed its answer to an inverted range (expected one mislabeled \
         state, got {len:?}) — re-read `build_from_input_buffer` and consider retiring \
         drop_stranded_input_buffer in src/net/client.rs (see module doc)"
    );
}
