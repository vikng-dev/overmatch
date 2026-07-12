//! Reproduction + mechanism proof for the "letting go of the MG still fires 1-2 more rounds" leak.
//! Replays lightyear's REAL input pipeline (`InputBuffer`, `NativeStateSequence`,
//! `build_from_input_buffer`, `update_buffer`, `get_predict`, `pop_keeping_last`) end to end and
//! counts the ticks on which fire is committed off a value the player never authored for that tick.

use core::time::Duration;
use std::collections::HashMap;

use bevy::prelude::Reflect;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{Compressed, InputBuffer};
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
const REDUNDANCY: u32 = 5; // lightyear InputConfig default
const HISTORY_DEPTH: u32 = 20; // lightyear_inputs::HISTORY_DEPTH
/// `Tick` is a WRAPPING id — keep every tick well away from 0.
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
                "CLIENT t={t} authored={:?} raw={:?}",
                authored.get(&t),
                client.get_raw(tk(t))
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
        if let Some(snap) = server.get_predict(tick) {
            action = snap.clone();
        }
        // FixedUpdate: our bridge.
        let c = bridge(s.fix, &server, tick, &action);
        if c.fire_secondary && !authorized(&t) {
            out.server_leak.push(t);
            out.notes.push(format!(
                "SERVER t={t} authored={:?} raw={:?} get={:?} held_last={}",
                authored.get(&t),
                server.get_raw(tick),
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
/// buffer tick entirely. `InputBuffer::set_raw` gap-fills the skipped tick with
/// `Compressed::SameAsPrecedent` — a FABRICATED repeat of the last command, on a tick the player
/// never authored at all. `get()` resolves `SameAsPrecedent` back to `Some(pressed)`, so
/// `held_last` is false on BOTH ends: the client fires the phantom round itself AND ships the
/// fabrication to the server, which fires it too.
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
/// HELD trigger produce the byte-identical `Compressed::SameAsPrecedent`, and both resolve through
/// `get` to `Some(pressed)`. Provenance — a value that knows which tick it was authored for — is
/// the only thing that separates them.
#[test]
fn same_as_precedent_cannot_distinguish_fabrication_from_a_held_trigger() {
    type PlainBuf = InputBuffer<ActionState<PlainCmd>, PlainCmd>;
    let pressed = ActionState(PlainCmd {
        fire_secondary: true,
        aim: 0,
    });
    let released = ActionState(PlainCmd {
        fire_secondary: false,
        aim: 0,
    });

    // A genuinely HELD trigger: `set` compresses the repeat at tick 11.
    let mut held: PlainBuf = PlainBuf::default();
    held.set(tk(10), pressed.clone());
    held.set(tk(11), pressed.clone());

    // A delay JUMP (2→3): buffer tick 11 is never authored — `set_raw` gap-fills it.
    let mut jump: PlainBuf = PlainBuf::default();
    jump.set(tk(10), pressed.clone());
    jump.set(tk(12), released.clone());

    assert_eq!(
        format!("{:?}", held.get_raw(tk(11))),
        "SameAsPrecedent",
        "a held trigger compresses to SameAsPrecedent"
    );
    assert_eq!(
        format!("{:?}", jump.get_raw(tk(11))),
        "SameAsPrecedent",
        "a FABRICATED gap-fill is the identical buffer shape"
    );
    // Both resolve to a pressed trigger, and neither is `held_last` (get() is Some for both).
    assert!(held.get(tk(11)).unwrap().0.fire_secondary);
    assert!(
        jump.get(tk(11)).unwrap().0.fire_secondary,
        "the player NEVER authored tick 11, yet the buffer hands back a pressed trigger"
    );
    assert!(
        jump.get(tk(11)).is_some(),
        "so `held_last` is false — blind"
    );
}

/// MECHANISM C — an `Absent` entry ANCHORS the server's buffer and FREEZES its `ActionState`.
///
/// This is the case that defeats even the retired `held_last` detector, and it is why the fix had to
/// become an attestation rather than a better detector. Verified here against the real
/// `InputBuffer`; upstream this is lightyear issue #1559 ("presses work, holds freeze"), still open.
///
/// Once an `Absent` sits in the buffer with a `SameAsPrecedent` tail behind it (which is what a HELD
/// button produces — nothing changes, so nothing is worth encoding):
///
/// - `get(tick)` recurses back through the `SameAsPrecedent`s, hits the `Absent`, and returns `None`
///   for the WHOLE tail.
/// - `get_last()` does the same — it DEAD-ENDS on the `Absent` and returns `None` too, even though
///   the buffer is manifestly non-empty. This is the killer: `held_last`'s second conjunct
///   (`get_last().is_some()`) goes FALSE exactly when the freeze bites, so the detector reports "not
///   extrapolating" at the precise moment the server is most lost.
/// - `get_predict(tick)` returns `None`, so lightyear's `update_action_state` SKIPS the apply
///   (server.rs:707) and the server's `ActionState` FREEZES at whatever it last held — a trigger-down
///   command, forever.
/// - `pop_keeping_last` degrades to a plain `pop` (its `get_last_with_tick()` is `None`), and `pop`'s
///   "repair" step re-writes the new front with the value it popped — which is the `Absent`. So the
///   anchor PROPAGATES FORWARD one tick per server tick. The poison sustains itself.
///
/// No stamp is needed to see any of this. That is the point: `for_tick` never asks WHY a value is
/// wrong.
#[test]
fn absent_anchor_freezes_the_server_and_blinds_the_held_last_detector() {
    let mut buf: Buf = Buf::default();
    let pressed = ActionState(Cmd {
        fire_secondary: true,
        aim: 0,
        for_tick: tk(10).0,
    });
    buf.set(tk(10), pressed.clone());
    // The Absent (however it got seeded), then the SameAsPrecedent tail a HELD button produces.
    buf.set_empty(tk(11));
    buf.set_raw(tk(12), Compressed::SameAsPrecedent);
    buf.set_raw(tk(13), Compressed::SameAsPrecedent);

    // The whole tail reads as "no input", even though the buffer is full of entries.
    assert!(buf.get(tk(13)).is_none(), "get dead-ends on the Absent");
    assert_eq!(buf.len(), 4, "…while the buffer is manifestly non-empty");

    // THE KILLER: get_last() is None too — it recurses back and dead-ends on the same Absent.
    assert!(
        buf.get_last().is_none(),
        "get_last must dead-end on the Absent — this is what blinds `held_last`"
    );

    // So the retired detector reports "not extrapolating" …
    let held_last = buf.get(tk(13)).is_none() && buf.get_last().is_some();
    assert!(
        !held_last,
        "held_last is FALSE precisely when the server is most lost — the detector is blind here"
    );

    // … while lightyear's server would SKIP the ActionState apply and freeze on the pressed command.
    assert!(
        buf.get_predict(tk(13)).is_none(),
        "get_predict returns None → update_action_state skips → ActionState FROZEN at pressed"
    );

    // And attestation sees it without knowing any of the above: the frozen command names tick 10.
    let frozen = pressed.0;
    assert_ne!(
        frozen.for_tick,
        tk(13).0,
        "the frozen command attests to tick 10, not tick 13 — consumables fail closed"
    );
}

/// The `Absent` anchor PROPAGATES: `pop_keeping_last` degrades to `pop`, whose repair step rewrites
/// the new front with the popped value — the `Absent` itself. So the freeze does not age out; the
/// server carries it forward one tick at a time.
#[test]
fn absent_anchor_propagates_forward_through_pop() {
    let mut buf: Buf = Buf::default();
    let pressed = ActionState(Cmd {
        fire_secondary: true,
        aim: 0,
        for_tick: tk(10).0,
    });
    buf.set(tk(10), pressed);
    buf.set_empty(tk(11));
    for t in 12..16 {
        buf.set_raw(tk(t), Compressed::SameAsPrecedent);
    }

    // The server simulating tick 12 pops up to tick 11 — straight through the Absent.
    buf.pop_keeping_last(tk(11));

    assert_eq!(
        format!("{:?}", buf.get_raw(tk(12))),
        "Absent",
        "pop's repair step rewrote the new front with the Absent it popped — the anchor MOVED"
    );
    assert!(
        buf.get_last().is_none(),
        "…so the buffer is still blind, one tick later, and will be next tick too"
    );
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

/// SCOPE of the freeze (the "stuck throttle" question). The `Absent` anchor freezes the server's
/// WHOLE `ActionState`, not just the fire fields — `get_predict` returns `None`, so lightyear's
/// `update_action_state` skips the apply for every field at once. So could a poisoned buffer strand
/// a THROTTLE and run the tank away?
///
/// It is bounded, and this pins why: the freeze can only PERSIST while the client's command is
/// byte-identical tick over tick, because that is what produces the all-`SameAsPrecedent` tail
/// behind the `Absent` (`set` only compresses a value equal to its precedent). The instant ANY field
/// changes, the client encodes a real `Compressed::Input`, `update_buffer` writes it (it is past
/// `last_remote_tick`), `get_predict` resolves it — and the `ActionState` un-freezes.
///
/// Which bounds the hazard sharply for us: `TankCommand::aim` is a HULL-LOCAL point, so it changes
/// every tick whenever the player moves the mouse, or the hull moves, or the hull rotates, or the
/// turret slews. In any gameplay that could run a tank away, the command already differs every tick
/// and the chain cannot form. And when the command IS bit-identical — parked, not aiming, not
/// touching anything — the frozen command is the command the player is still holding, so freezing it
/// is a no-op. Hold-last on the levels is lightyear's intended semantics and ours (see
/// `bridge_action_state_to_tank_command`); only the CONSUMABLES are gated, by `for_tick`.
#[test]
fn a_changed_command_unfreezes_the_absent_anchored_buffer() {
    let mut buf: Buf = Buf::default();
    let held = ActionState(Cmd {
        fire_secondary: true,
        aim: 0,
        for_tick: tk(10).0,
    });
    buf.set(tk(10), held.clone());
    buf.set_empty(tk(11)); // the Absent
    buf.set_raw(tk(12), Compressed::SameAsPrecedent); // …and the tail a HELD command produces
    buf.set_raw(tk(13), Compressed::SameAsPrecedent);
    assert!(
        buf.get_predict(tk(13)).is_none(),
        "frozen while the command does not change"
    );

    // Any change to ANY field — here `aim`, which our hull-local aim point moves every tick — is
    // encoded as a real `Input`, not a `SameAsPrecedent`.
    buf.set(
        tk(14),
        ActionState(Cmd {
            fire_secondary: true,
            aim: 7, // the aim moved
            for_tick: tk(14).0,
        }),
    );

    assert_eq!(
        format!("{:?}", buf.get_raw(tk(14))).split('(').next(),
        Some("Input"),
        "a changed command encodes a real Input, never a SameAsPrecedent"
    );
    assert!(
        buf.get_predict(tk(14)).is_some(),
        "…so the ActionState UN-FREEZES: the freeze cannot outlive the first changed command"
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

/// Replay the pipeline with the REAL Absent seed route: `buffer_action_state` (FixedPreUpdate)
/// writes at `t + d_fix`, but `prepare_input_message` (PostUpdate) computes `end_tick = t + d_post`.
/// When the delay INCREMENTS between those two points, `end_tick` lands one tick past the client's
/// own buffer end and `build_from_input_buffer` encodes that trailing tick as `Compressed::Absent`.
///
/// Returns (server_fire_ticks_unauthored, server_throttle_mismatch_ticks, froze_at_all)
fn run_freeze(
    d_fix: &dyn Fn(i32) -> i32,
    d_post: &dyn Fn(i32) -> i32,
    press: i32,
    release: i32,
    moving_aim: bool,
    last: i32,
) -> (Vec<i32>, Vec<(i32, u8, u8)>, usize) {
    let cmd_at = |t: i32| Cmd2 {
        throttle: if t >= press && t < release { 100 } else { 0 },
        fire_secondary: t >= press && t < release,
        aim: if moving_aim { t as u16 } else { 0 },
        for_tick: 0,
    };

    let mut authored: HashMap<i32, Cmd2> = HashMap::new();
    let mut client: Buf2 = Buf2::default();
    let mut wire: Vec<(i32, Tick, Seq2)> = Vec::new();

    for t in 0..last {
        let b = t + d_fix(t);
        let mut c = cmd_at(t);
        c.for_tick = tk(b).0;
        client.set(tk(b), ActionState(c));
        authored.insert(b, c);

        // PostUpdate: end_tick uses the delay AS OF POSTUPDATE.
        let e = t + d_post(t);
        if let Some(seq) = Seq2::build_from_input_buffer(&client, REDUNDANCY, tk(e)) {
            wire.push((t, tk(e), seq));
        }
        client.pop(tk(t - HISTORY_DEPTH as i32));
    }

    let mut server: Buf2 = Buf2::default();
    let mut action = ActionState::<Cmd2>::default();
    let mut fire_leak = Vec::new();
    let mut throttle_bad = Vec::new();
    let mut frozen = 0usize;

    for t in 0..last {
        for (_, end_tick, seq) in wire.iter().filter(|(a, _, _)| *a == t) {
            seq.clone().update_buffer(&mut server, *end_tick, TICK);
        }
        let tick = tk(t);
        let applied = server.get_predict(tick).cloned();
        if applied.is_none() {
            frozen += 1; // update_action_state SKIPS the apply -> ActionState frozen
        } else {
            action = applied.unwrap();
        }
        // the bridge: for_tick attestation gates CONSUMABLES only; levels ride through (hold-last)
        let mut c = action.0;
        if c.for_tick != tick.0 {
            c.fire_secondary = false; // fail_consumables_closed
        }
        if let Some(auth) = authored.get(&t) {
            if c.fire_secondary && !auth.fire_secondary {
                fire_leak.push(t);
            }
            if c.throttle != auth.throttle {
                throttle_bad.push((t, auth.throttle, c.throttle));
            }
        }
        server.pop_keeping_last(tick - 1);
    }
    (fire_leak, throttle_bad, frozen)
}

/// THE SCOPE QUESTION. Player HOLDS throttle+trigger (the "holds freeze" case), then RELEASES both.
/// The input delay increments between FixedPreUpdate and PostUpdate on one tick, seeding a trailing
/// `Absent` in the message — the real seed route from upstream report #10.
#[test]
fn scope_absent_freeze_throttle_and_fire() {
    let press = 40;
    let release = 60;
    println!(
        "\n{:>5} {:>6} {:>7} {:>10} {:>14} {:>26}",
        "seed", "aim", "frozen", "fire-leak", "throttle-bad", "throttle detail"
    );
    for moving_aim in [false, true] {
        for seed in 50..66 {
            // delay is 2 everywhere; on tick `seed` PostUpdate sees 3 (it incremented mid-frame),
            // and from `seed+1` the FixedPreUpdate write also uses 3.
            let d_fix = move |t: i32| if t > seed { 3 } else { 2 };
            let d_post = move |t: i32| if t >= seed { 3 } else { 2 };
            let (fire, thr, frozen) = run_freeze(&d_fix, &d_post, press, release, moving_aim, 100);
            if !fire.is_empty() || !thr.is_empty() || frozen > 0 {
                let detail = thr
                    .iter()
                    .take(3)
                    .map(|(t, a, g)| format!("t{t}:want{a}/got{g}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!(
                    "{seed:>5} {:>6} {frozen:>7} {:>10} {:>14} {detail:>26}",
                    moving_aim,
                    format!("{:?}", fire),
                    thr.len(),
                );
            }
        }
    }
}
