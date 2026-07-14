//! UPSTREAM TRIPWIRE for the input-message range wrap that `src/net/client.rs`'s
//! `drop_stranded_input_buffer` guard compensates for (the 2026-07-11 §7 connect-hang fix; full
//! decode in `.agents/docs/design/sim-divergence-and-determinism.md` §7).
//!
//! The mechanism being pinned, in lightyear 0.28 (`lightyear_inputs_native` input_message.rs):
//! `build_from_input_buffer` computes `buffer_end = (end_tick + 1 - buffer_start_tick) as usize`
//! and loops `buffer_start..buffer_end`, pushing one `Compressed` per iteration. `Tick - Tick`
//! returns a plain `i32`, so whenever the buffer's `start_tick` leads `end_tick` by ≥ 2 the
//! difference is negative, the `as usize` sign-extends to ~2^64, and the loop becomes an
//! unbounded allocating spin — the load-gated connect hang (silent wedge, RSS balloon, eventual
//! SIGKILL). The strand itself is persistent because `InputBuffer::set_raw` refuses writes below
//! `start_tick`, so a backward connect-window resync leaves the buffer ahead of the timeline
//! forever.
//!
//! These tests deliberately pin only the two safe semantic enablers: non-saturating `Tick`
//! subtraction and refusal to re-anchor `InputBuffer` at a lower tick. They do not call
//! `build_from_input_buffer` with an inverted range: in lightyear 0.28 that call can allocate
//! without a bound. If either tripwire fails after an upgrade, the §7 guard may be retirable;
//! verify the changed upstream behavior with a bounded reproduction before removing it.

use bevy::prelude::Reflect;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{Compressed, InputBuffer};
use lightyear_inputs_native::prelude::ActionState;
use serde::{Deserialize, Serialize};

/// Minimal action for this input buffer — the shape of our `TankCommand` without dragging the
/// game's input type into the pin.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default, Reflect)]
struct TestAction(u8);

type TestBuffer = InputBuffer<ActionState<TestAction>, TestAction>;

/// Enabler #1: `Tick` subtraction is a plain (non-saturating) `i32` difference. The wedge needs
/// the negative value; if upstream saturates at zero the `as usize` wrap is impossible.
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
    buffer.set_raw(Tick(313), Compressed::Input(ActionState(TestAction(1))));
    buffer.set_raw(Tick(20), Compressed::Input(ActionState(TestAction(2))));
    assert_eq!(
        buffer.start_tick,
        Some(Tick(313)),
        "lightyear's InputBuffer::set_raw now accepts/re-anchors below start_tick — re-evaluate \
         the production scheduling with a bounded reproduction before retiring the §7 guard in \
         src/net/client.rs (see module doc)"
    );
}
