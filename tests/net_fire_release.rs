//! Reproduction + mechanism proof for the "letting go of the MG still fires 1-2 more rounds" leak.
//! Replays lightyear's REAL input pipeline (`InputBuffer`, `NativeStateSequence`,
//! `build_from_input_buffer`, `update_buffer`, `get_predict`, `pop_keeping_last`) end to end and
//! counts the ticks on which fire is committed off a value the player never authored for that tick.

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
const REDUNDANCY: u32 = 5; // lightyear InputConfig default
const HISTORY_DEPTH: u32 = 20; // lightyear_inputs::HISTORY_DEPTH
/// `Tick` is a WRAPPING id — keep every tick well away from 0.
const BASE: i32 = 1000;

fn tk(t: i32) -> Tick {
    Tick((BASE + t) as u32)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fix {
    /// Shipping today (2ea6cf5): fail `fire_secondary` closed iff the buffer has NO entry for the
    /// tick — `get(tick).is_none() && get_last().is_some()`.
    HeldLast,
    /// Positive attestation: commit fire only if the command was authored FOR this exact tick.
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
}
