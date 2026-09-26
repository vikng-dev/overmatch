//! Reproduction + mechanism proof for the "letting go of the MG still fires 1-2 more rounds" leak.
//! Replays lightyear's REAL input pipeline (`InputBuffer`, `NativeStateSequence`,
//! `build_from_input_buffer`, `update_buffer`, `predict`, `pop_keeping_last`) end to end and
//! counts the ticks on which fire is committed off a value the player never authored for that tick.
//!
//! Pinned against lightyear 0.30, whose `InputBuffer` stores MATERIALIZED `Option` values (wire
//! compression — `Compressed::SameAsPrecedent` — is re-derived at message-build time, never
//! stored). So an `Absent` entry is a single-tick hole: it neither dead-ends the reads behind it
//! nor travels through `pop` (upstream issue #1559 describes the stored-chain buffer where it
//! did; the record is `upstream/lightyear-absent-anchor-input-freeze.md`).

use core::time::Duration;
use std::collections::HashMap;

use bevy::prelude::Reflect;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::InputBuffer;
use lightyear_inputs::input_message::ActionStateSequence;
use lightyear_inputs_native::prelude::{ActionState, NativeStateSequence};
use serde::{Deserialize, Serialize};

/// Stand-in for `TankCommand`: the automatic-fire LEVEL, an absolute, and (for the `ForTick` fix
/// evaluation) the destination tick the command was authored for.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default, Reflect)]
struct Cmd {
    fire_secondary: bool,
    aim: u16,
    for_tick: u32,
}

type Buf = InputBuffer<ActionState<Cmd>, Cmd>;
type Seq = NativeStateSequence<Cmd>;

const TICK: Duration = Duration::from_nanos(15_625_000); // 64 Hz
const REDUNDANCY: usize = 5; // lightyear InputConfig default
const HISTORY_DEPTH: u32 = 20; // lightyear_inputs::HISTORY_DEPTH
/// Tick arithmetic SATURATES at 0 (lightyear's `Tick` is monotonic) — keep every tick well away
/// from it so a history-depth subtraction stays exact.
const BASE: i32 = 1000;

fn tk(t: i32) -> Tick {
    Tick((BASE + t) as u32)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fix {
    /// The RETIRED detector (commit 2ea6cf5): fail `fire_secondary` closed iff the buffer has NO
    /// entry for the tick — `get(tick).is_none() && get_last().is_some()`. Kept here only as the
    /// baseline the sweep measures against.
    HeldLast,
    /// SHIPPING: positive attestation — commit a consumable only if the command was authored FOR
    /// this exact tick (`TankCommand::for_tick`, stamped by `net::client`'s `stamp_input_tick`).
    ForTick,
}

struct Scenario {
    /// Input delay per LOCAL client tick. `InputDelayConfig::balanced()` recomputes this every
    /// sync from live RTT+jitter, so on a real link it WOBBLES (0..=3 for a sub-50 ms ping).
    delay: Box<dyn Fn(i32) -> i32>,
    /// Server tick on which the message sent at local tick `t` arrives.
    m: i32,
    press: i32,
    release: i32,
    /// Local ticks whose input packet is lost.
    drop: Vec<i32>,
    /// Deliver messages in reverse within an arrival tick (reordering).
    reorder: bool,
    moving_aim: bool,
    last_tick: i32,
    fix: Fix,
}

impl Scenario {
    fn base() -> Self {
        Self {
            delay: Box::new(|_| 3),
            m: 0,
            press: 40,
            release: 60,
            drop: vec![],
            reorder: false,
            moving_aim: false,
            last_tick: 100,
            fix: Fix::HeldLast,
        }
    }
}

#[derive(Default, Debug)]
struct Outcome {
    /// Server ticks where fire was committed on a value the player never authored as `true` FOR
    /// that tick.
    server_leak: Vec<i32>,
    /// Same, on the client's own predicted tank.
    client_leak: Vec<i32>,
    notes: Vec<String>,
}

/// The bridge rule (`net::protocol::bridge_action_state_to_tank_command`).
fn bridge(fix: Fix, buf: &Buf, tick: Tick, action: &ActionState<Cmd>) -> Cmd {
    let mut c = action.0;
    match fix {
        Fix::HeldLast => {
            if buf.get(tick).is_none() && buf.get_last().is_some() {
                c.fire_secondary = false;
            }
        }
        Fix::ForTick => {
            if c.for_tick != tick.0 {
                c.fire_secondary = false;
            }
        }
    }
    c
}

fn run(s: &Scenario) -> Outcome {
    let mut out = Outcome::default();

    // Ground truth: what the player AUTHORED for each buffer tick (last writer wins, exactly like
    // `InputBuffer::set` on the client). A buffer tick absent from this map was authored by NOBODY.
    let mut authored: HashMap<i32, bool> = HashMap::new();
    for t in 0..s.last_tick {
        authored.insert(t + (s.delay)(t), t >= s.press && t < s.release);
    }
    let authorized = |t: &i32| authored.get(t) == Some(&true);

    // ---------------- client
    let mut client: Buf = Buf::default();
    let mut client_action = ActionState::<Cmd>::default();
    let mut wire: Vec<(i32, Tick, Seq)> = Vec::new();

    for t in 0..s.last_tick {
        let b = t + (s.delay)(t);
        let cmd = Cmd {
            fire_secondary: t >= s.press && t < s.release,
            aim: if s.moving_aim { t as u16 } else { 0 },
            for_tick: tk(b).0,
        };
        // FixedPreUpdate: lightyear `buffer_action_state` — writes the DELAYED tick.
        client.set(tk(b), ActionState(cmd));
        // FixedPreUpdate: lightyear `get_action_state` — exact `get` of the CURRENT tick.
        if let Some(snap) = client.get(tk(t)) {
            client_action = snap.clone();
        }
        // FixedUpdate: our bridge, on the client's own predicted tank.
        let c = bridge(s.fix, &client, tk(t), &client_action);
        if c.fire_secondary && !authorized(&t) {
            out.client_leak.push(t);
            out.notes.push(format!(
                "CLIENT t={t} authored={:?} stored={:?}",
                authored.get(&t),
                client.get(tk(t)).map(|a| a.0.fire_secondary)
            ));
        }
        // PostUpdate: `prepare_input_message` + `clean_buffers`.
        if !s.drop.contains(&t)
            && let Some(seq) = Seq::build_from_input_buffer(&client, REDUNDANCY, tk(b))
        {
            wire.push((t + s.m, tk(b), seq));
        }
        client.pop(tk(t - HISTORY_DEPTH as i32));
    }

    // ---------------- server
    let mut server: Buf = Buf::default();
    let mut action = ActionState::<Cmd>::default();
    for t in 0..s.last_tick {
        let mut arriving: Vec<_> = wire.iter().filter(|(a, _, _)| *a == t).collect();
        if s.reorder {
            arriving.reverse();
        }
        // PreUpdate: lightyear `receive_input_message`.
        for (_, end_tick, seq) in arriving {
            seq.clone().update_buffer(&mut server, *end_tick, TICK);
        }
        let tick = tk(t);
        // FixedPreUpdate: lightyear `update_action_state`. NOTE: on `None` the ActionState is left
        // STALE — lightyear does not touch it.
        if let Some(snap) = server.predict(tick, TICK) {
            action = snap;
        }
        // FixedUpdate: our bridge.
        let c = bridge(s.fix, &server, tick, &action);
        if c.fire_secondary && !authorized(&t) {
            out.server_leak.push(t);
            out.notes.push(format!(
                "SERVER t={t} authored={:?} get={:?} held_last={}",
                authored.get(&t),
                server.get(tick).map(|a| a.0.fire_secondary),
                server.get(tick).is_none() && server.get_last().is_some(),
            ));
        }
        server.pop_keeping_last(tick - 1);
    }
    out
}

fn step(before: i32, after: i32, switch: i32) -> Box<dyn Fn(i32) -> i32> {
    Box::new(move |t| if t < switch { before } else { after })
}

/// MECHANISM A — the input delay SHRINKS (RTT improves), so `end_tick` STALLS: two consecutive
/// local ticks author the SAME buffer tick. The client's own `InputBuffer::set` overwrites its
/// entry with the newer (RELEASED) command and re-sends it, but lightyear's `update_buffer`
/// refuses to write any tick `<= last_remote_tick`, so the SERVER can never learn the correction
/// and keeps the stale PRESSED value. `get(tick)` returns a real `Some(Input(..))` — the
/// `held_last` detector is blind.
///
/// The client does NOT fire (its own buffer holds the correction); the SERVER does. That is the
/// belt snapping down and the target taking hits the player never asked for.
#[test]
fn delay_shrink_strands_a_stale_pressed_tick_on_the_server() {
    let out = run(&Scenario {
        delay: step(3, 2, 60), // the delay drops on the very tick the player releases
        fix: Fix::HeldLast,
        ..Scenario::base()
    });
    assert_eq!(
        out.server_leak,
        vec![62],
        "expected the server to fire on stranded tick 62; notes: {:?}",
        out.notes
    );
    assert!(
        out.client_leak.is_empty(),
        "the client's own buffer holds the correction, so it must NOT fire"
    );
}

/// MECHANISM B — the input delay GROWS (RTT worsens), so `end_tick` JUMPS: the client skips a
/// buffer tick entirely. `InputBuffer::set_raw` gap-fills the skipped tick with a copy of the last
/// stored command (hold-last) — a FABRICATED repeat, on a tick the player never authored at all.
/// `get()` returns it as `Some(pressed)`, so `held_last` is false on BOTH ends: the client fires
/// the phantom round itself AND ships the fabrication to the server, which fires it too.
///
/// A 1→3 delay jump fabricates TWO ticks — literally "one or two more shots".
#[test]
fn delay_growth_fabricates_unauthored_pressed_ticks_on_both_ends() {
    let out = run(&Scenario {
        delay: step(1, 3, 59),
        fix: Fix::HeldLast,
        ..Scenario::base()
    });
    assert_eq!(out.server_leak, vec![60, 61], "notes: {:?}", out.notes);
    assert_eq!(
        out.client_leak,
        vec![60, 61],
        "the client fires the fabricated rounds itself — the owner sees his OWN muzzle flash"
    );
}

/// TODAY's `TankCommand` shape — no provenance. This is the wire as shipped.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default, Reflect)]
struct PlainCmd {
    fire_secondary: bool,
    aim: u16,
}

/// Why no buffer-SHAPE rule can close this on today's wire: a fabricated gap-fill and a genuinely
/// HELD trigger materialize the byte-identical stored value, both read back through `get` as
/// `Some(pressed)`, and both re-encode on the wire as the same `SameAsPrecedent`. Provenance — a value
/// that knows which tick it was authored for — is the only thing that separates them.
#[test]
fn a_fabricated_gap_fill_is_indistinguishable_from_a_held_trigger() {
    type PlainBuf = InputBuffer<ActionState<PlainCmd>, PlainCmd>;
    type PlainSeq = NativeStateSequence<PlainCmd>;
    let pressed = ActionState(PlainCmd {
        fire_secondary: true,
        aim: 0,
    });
    let released = ActionState(PlainCmd {
        fire_secondary: false,
        aim: 0,
    });

    // A genuinely HELD trigger: the player authored ticks 10 and 11, both pressed.
    let mut held: PlainBuf = PlainBuf::default();
    held.set(tk(10), pressed.clone());
    held.set(tk(11), pressed.clone());
    held.set(tk(12), released.clone());

    // A delay JUMP (2→3): buffer tick 11 is never authored — `set_raw` gap-fills it.
    let mut jump: PlainBuf = PlainBuf::default();
    jump.set(tk(10), pressed.clone());
    jump.set(tk(12), released.clone());

    assert_eq!(
        held.get(tk(11)),
        jump.get(tk(11)),
        "a FABRICATED gap-fill stores the identical value a held trigger does"
    );
    assert!(
        jump.get(tk(11)).is_some_and(|a| a.0.fire_secondary),
        "the player NEVER authored tick 11, yet the buffer hands back a pressed trigger — so \
         `held_last` (get() is Some) is blind"
    );
    let held_wire = PlainSeq::build_from_input_buffer(&held, 3, tk(12)).expect("held encodes");
    let jump_wire = PlainSeq::build_from_input_buffer(&jump, 3, tk(12)).expect("jump encodes");
    assert_eq!(
        format!("{held_wire:?}"),
        format!("{jump_wire:?}"),
        "…and the wire carries the two byte-identically"
    );
}

/// What an `Absent` entry does in 0.30: it is a ONE-TICK hole, not an anchor. A stored `None` reads
/// as no input at exactly its own tick — `get` and `predict` both return `None` there, so
/// lightyear's server `update_action_state` skips the apply and the `ActionState` holds the previous
/// command for that one tick — while every stored tick behind it resolves on its own.
///
/// (A buffer that stored the compressed chain would let a `SameAsPrecedent` tail behind an
/// `Absent` dead-end every read for the WHOLE tail and freeze the server indefinitely — upstream
/// issue #1559, "presses work, holds freeze"; record in
/// `upstream/lightyear-absent-anchor-input-freeze.md`.)
///
/// Attestation needs none of this: the held command names the tick it was authored for, so the
/// one held tick fails consumables closed.
#[test]
fn an_absent_entry_is_a_one_tick_hole() {
    let pressed = |t: i32| {
        ActionState(Cmd {
            fire_secondary: true,
            aim: 0,
            for_tick: tk(t).0,
        })
    };
    let mut buf: Buf = Buf::default();
    buf.set(tk(10), pressed(10));
    buf.set_empty(tk(11));
    buf.set(tk(12), pressed(12));
    buf.set(tk(13), pressed(13));

    assert!(buf.get(tk(11)).is_none(), "the hole reads as no input");
    assert!(
        buf.predict(tk(11), TICK).is_none(),
        "predict returns None at the hole → update_action_state skips → ActionState held one tick"
    );
    assert!(
        buf.get(tk(13)).is_some() && buf.get_last().is_some(),
        "the ticks behind the hole resolve on their own — no dead-ending"
    );
    assert_eq!(
        buf.predict(tk(13), TICK).map(|a| a.0.for_tick),
        Some(tk(13).0),
        "…and the very next stored tick re-applies a command authored for it"
    );

    // Attestation: the command held across the hole names tick 10, not 11.
    assert_ne!(
        pressed(10).0.for_tick,
        tk(11).0,
        "the held command attests to tick 10 — consumables fail closed on tick 11"
    );
}

/// The hole does not travel. `pop` drops independent materialized values and repairs nothing, so
/// popping through an `Absent` leaves the next tick exactly as stored — a front rewritten with
/// the popped `Absent` would carry a freeze forward one tick per server tick.
#[test]
fn popping_through_an_absent_entry_does_not_move_it() {
    let pressed = ActionState(Cmd {
        fire_secondary: true,
        aim: 0,
        for_tick: tk(10).0,
    });
    let mut buf: Buf = Buf::default();
    buf.set(tk(10), pressed.clone());
    buf.set_empty(tk(11));
    for t in 12..16 {
        buf.set(tk(t), pressed.clone());
    }

    // The server simulating tick 12 pops up to tick 11 — straight through the Absent.
    buf.pop_keeping_last(tk(11));

    assert!(
        buf.get(tk(12)).is_some(),
        "the new front is the value stored at tick 12, not the popped Absent"
    );
    assert!(buf.get_last().is_some(), "…and the buffer reads normally");
}

/// The client never PUTS a `SameAsPrecedent` behind an `Absent`: the encoder emits it only between
/// two EQUAL stored values, so the tick after a hole is always encoded as a real `Input`. That is
/// the property that keeps a held button from materializing a server-side `None` tail — the decode
/// itself would still resolve a `SameAsPrecedent` after an `Absent` to `None`
/// (`Compressed::resolve`).
#[test]
fn the_encoder_never_compresses_across_an_absent_entry() {
    let held = ActionState(Cmd {
        fire_secondary: true,
        aim: 0,
        for_tick: 0,
    });
    let mut buf: Buf = Buf::default();
    buf.set(tk(10), held.clone());
    buf.set_empty(tk(11));
    buf.set(tk(12), held.clone());
    buf.set(tk(13), held.clone());

    let wire = format!(
        "{:?}",
        Seq::build_from_input_buffer(&buf, 4, tk(13)).expect("buffer encodes")
    );
    // The four states in order: the tick after the hole must be a real `Input`, and only the
    // equal pair behind it compresses.
    let mut from = 0;
    for needle in ["Input", "Absent", "Input", "SameAsPrecedent"] {
        let at = wire[from..].find(needle).unwrap_or_else(|| {
            panic!("expected Input, Absent, Input, SameAsPrecedent in order; got {wire}")
        });
        from += at + needle.len();
    }
}

/// Before/after table: today's `held_last` guard vs. the candidate fixes, swept over delay wobble,
/// packet loss, reordering and arrival skew.
#[test]
fn sweep() {
    let deltas = [
        (3, 2),
        (2, 1),
        (3, 1),
        (2, 3),
        (1, 2),
        (1, 3),
        (0, 3),
        (3, 3),
    ];
    let mut table: Vec<(String, usize, usize, usize)> = Vec::new();

    for fix in [Fix::HeldLast, Fix::ForTick] {
        for const_delay in [false, true] {
            let (mut srv, mut cli, mut cases) = (0usize, 0usize, 0usize);
            for moving_aim in [false, true] {
                for m in [-2, -1, 0, 1] {
                    for reorder in [false, true] {
                        for burst in [0usize, 1, 3, 6] {
                            for (before, after) in deltas {
                                for switch in 50..66 {
                                    let delay: Box<dyn Fn(i32) -> i32> = if const_delay {
                                        Box::new(move |_| before)
                                    } else {
                                        step(before, after, switch)
                                    };
                                    let out = run(&Scenario {
                                        delay,
                                        m,
                                        reorder,
                                        moving_aim,
                                        drop: (0..burst as i32).map(|i| 55 + i).collect(),
                                        fix,
                                        ..Scenario::base()
                                    });
                                    cases += 1;
                                    srv += out.server_leak.len();
                                    cli += out.client_leak.len();
                                }
                            }
                        }
                    }
                }
            }
            table.push((
                format!(
                    "{fix:?} + {}",
                    if const_delay {
                        "CONST delay"
                    } else {
                        "balanced() delay"
                    }
                ),
                cases,
                srv,
                cli,
            ));
        }
    }
    println!(
        "\n{:<32} {:>7} {:>12} {:>12}",
        "config", "cases", "server-leak", "client-leak"
    );
    for (name, cases, srv, cli) in &table {
        println!("{name:<32} {cases:>7} {srv:>12} {cli:>12}");
    }

    // THE SHIPPING CONFIGURATION, pinned: positive attestation (`TankCommand::for_tick`, checked by
    // `net::protocol`'s bridge) on top of a CONSTANT input delay (`net::client`'s
    // `SHIPPING_INPUT_DELAY_TICKS`). Across every combination of delay wobble, burst loss,
    // reordering and arrival skew in the sweep, the number of rounds fired off input the player
    // never authored is ZERO — on the server and on the client's own predicted tank alike.
    let (_, cases, srv, cli) = table
        .iter()
        .find(|(name, ..)| name.starts_with("ForTick + CONST"))
        .expect("the shipping configuration is in the table");
    assert_eq!(
        (*srv, *cli),
        (0, 0),
        "SHIPPING CONFIG LEAKS. Across {cases} scenarios the server fired {srv} and the client \
         {cli} rounds off input the player never authored. Something re-opened a seed the constant \
         input delay was closing, or weakened the for_tick attestation in the bridge.",
    );
}

// ===================== SCOPE EXPERIMENT: the Absent freeze on the SERVER path =====================

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default, Reflect)]
struct Cmd2 {
    throttle: u8,
    fire_secondary: bool,
    aim: u16,
    for_tick: u32,
}
type Buf2 = InputBuffer<ActionState<Cmd2>, Cmd2>;
type Seq2 = NativeStateSequence<Cmd2>;

/// THE SCOPE QUESTION (Yan): can the `Absent` freeze stick a HELD THROTTLE / turret traverse, not
/// just fire? If it can, `for_tick` does NOT cover it — the movement levels legitimately hold-last,
/// so gating discrete actions would not help, and the remedy would be a buffer re-anchor watchdog.
///
/// **Answer: no — it is bounded to ONE tick, and that bound is structural, not lucky.**
///
/// A held (unchanged) command is exactly what the stored value already is, so holding it through a
/// skipped apply is a NO-OP for every field. The first tick whose command CHANGES is encoded as a
/// real `Compressed::Input`, lands past `last_remote_tick`, and is applied. And a release is a
/// change. So a stale `ActionState` cannot outlive the input it is holding — whatever seeded it,
/// and however long the player holds.
///
/// The one tick that CAN differ is the transition itself: the delay jump opens a GAP tick that nobody
/// authored, sitting between the last pressed tick and the first released one, and the frozen value
/// there is the OLD (pressed) one. Measured below: exactly 1 tick, in the one seed position that
/// lines up (seed 59), with and without a moving aim.
///
/// **And that single tick is why consumables are gated and levels are not.** One tick is 15.6 ms.
/// For a LEVEL it is 15.6 ms of extra throttle on a 57-tonne vehicle — a centimetre of travel, wiped
/// out by the very next tick's real input, and physically beneath the contact model's noise floor. For a
/// CONSUMABLE it is a round out of the barrel: the MG's reload timer sits at 0 after a burst, so a
/// single stray trigger-true tick fires immediately, spends ammo and deals damage — and there is
/// nothing to take back. Same tick, same freeze, categorically different consequence. That asymmetry
/// is the entire design, and this test pins both halves of it.
///
/// No re-anchor watchdog is warranted.
#[test]
fn scope_can_the_freeze_stick_a_held_throttle() {
    let (press, release) = (40, 60);
    let mut worst_drive = 0;
    let mut worst_fire = 0;
    // …and the same sweep with the attestation gate REMOVED, to show what it is buying.
    let mut worst_fire_ungated = 0;

    println!(
        "\n{:>5} {:>6} {:>20} {:>20} {:>20}",
        "seed", "aim", "drive-after-release", "fire-after (gated)", "fire-after (UNGATED)"
    );
    for moving_aim in [false, true] {
        for seed in 50..66 {
            let d_fix = move |t: i32| if t > seed { 3 } else { 2 };
            let d_post = move |t: i32| if t >= seed { 3 } else { 2 };

            let cmd_at = |t: i32| Cmd2 {
                throttle: if t >= press && t < release { 100 } else { 0 },
                fire_secondary: t >= press && t < release,
                aim: if moving_aim { t as u16 } else { 0 },
                for_tick: 0,
            };

            let mut client: Buf2 = Buf2::default();
            let mut wire: Vec<(i32, Tick, Seq2)> = Vec::new();
            // The LAST buffer tick the player authored as pressed. Every tick after this one is a
            // tick the player is no longer asking for anything on — so any driving or firing the
            // server does past it is driving/firing nobody asked for. (Computing this from the
            // player's RELEASE instead would skip the GAP ticks a delay jump opens up between the
            // last pressed tick and the first released one — which are exactly the leak.)
            let mut last_pressed = i32::MIN;
            for t in 0..100 {
                let b = t + d_fix(t);
                let mut c = cmd_at(t);
                c.for_tick = tk(b).0;
                if c.fire_secondary {
                    last_pressed = last_pressed.max(b);
                }
                client.set(tk(b), ActionState(c));
                let e = t + d_post(t);
                if let Some(seq) = Seq2::build_from_input_buffer(&client, REDUNDANCY, tk(e)) {
                    wire.push((t, tk(e), seq));
                }
                client.pop(tk(t - HISTORY_DEPTH as i32));
            }

            let mut server: Buf2 = Buf2::default();
            let mut action = ActionState::<Cmd2>::default();
            let (mut drive_after, mut fire_after, mut fire_ungated) = (0, 0, 0);
            for t in 0..100 {
                for (_, end_tick, seq) in wire.iter().filter(|(a, _, _)| *a == t) {
                    seq.clone().update_buffer(&mut server, *end_tick, TICK);
                }
                let tick = tk(t);
                // lightyear `update_action_state`: on None the apply is SKIPPED — ActionState FROZEN.
                if let Some(snap) = server.predict(tick, TICK) {
                    action = snap;
                }
                let raw = action.0;
                // our bridge: attestation gates CONSUMABLES; the levels ride through on hold-last.
                let mut c = raw;
                if c.for_tick != tick.0 {
                    c.fire_secondary = false;
                }
                if t > last_pressed {
                    if c.throttle > 0 {
                        drive_after += 1;
                    }
                    if c.fire_secondary {
                        fire_after += 1;
                    }
                    if raw.fire_secondary {
                        fire_ungated += 1;
                    }
                }
                server.pop_keeping_last(tick - 1);
            }
            worst_drive = worst_drive.max(drive_after);
            worst_fire = worst_fire.max(fire_after);
            worst_fire_ungated = worst_fire_ungated.max(fire_ungated);
            if drive_after > 0 || fire_after > 0 || fire_ungated > 0 {
                println!(
                    "{seed:>5} {moving_aim:>6} {drive_after:>20} {fire_after:>20} {fire_ungated:>20}"
                );
            }
        }
    }
    println!(
        "WORST: drive-after-release={worst_drive} ticks | fire-after-release {worst_fire} gated vs \
         {worst_fire_ungated} UNGATED"
    );

    // THE ASYMMETRY — and the whole reason `for_tick` gates the consumables and NOT the levels.
    // The very same freeze that CANNOT strand a throttle for even one tick DOES fire rounds.
    assert!(
        worst_fire_ungated > 0,
        "expected the ungated freeze to leak fire — otherwise this sweep is not exercising it",
    );

    assert_eq!(
        worst_fire, 0,
        "attestation must fire ZERO rounds after the player let go, under every seed position",
    );
    // The freeze CANNOT outlive the first CHANGED command: the change encodes a real
    // `Compressed::Input`, which lands past `last_remote_tick` and is applied. The release IS that
    // change. So a stuck throttle is bounded by the ticks between the `Absent` and
    // the release — and while the command is unchanged, the frozen value IS the value the player is
    // still holding, so holding it is a no-op. The only tick that can differ is the transition
    // itself.
    assert!(
        worst_drive <= 1,
        "a held throttle stuck for {worst_drive} ticks after release — the freeze CAN outlive the \
         release, so hold-last on the levels is unsafe and a re-anchor watchdog is needed",
    );
}
