//! Arrival-owned input for the game's fire control and damage model (game state only).
//! Each link evaluates independently; the fixed bridge commits its ordered command before play.

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{
    Client, Connected, ControlledBy, MessageReceiver, MessageSender, MessageSystems,
};
use serde::{Deserialize, Serialize};

use super::input_gap::InputGap;
use super::timing::Timing;
use crate::command::{
    AimIntent, CommandError, CrewSwap, ParsedTankCommand, RoundSelect, TankCommand,
};
use crate::state::GameplaySet;

/// Resolution: one answer-mask bit per pending edge bounds both datagrams and per-link work.
pub(super) const PENDING: usize = u16::BITS as usize;
/// Resolution: one withdrawal slot for secondary fire and one for crew-swap cancellation.
/// Starts cannot consume these slots while answers are missing.
const WITHDRAWAL_RESERVE: usize = 1 + 1;

/// Absolute device levels, parsed once at the receiving edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct InputLevels {
    pub throttle: f32,
    pub steer: f32,
    pub brake: bool,
    pub aim: Option<AimIntent>,
    pub range: f32,
}

impl From<TankCommand> for InputLevels {
    fn from(c: TankCommand) -> Self {
        Self {
            throttle: c.throttle,
            steer: c.steer,
            brake: c.brake,
            aim: c.aim,
            range: c.range,
        }
    }
}
impl InputLevels {
    fn parse(self) -> Result<ParsedTankCommand, CommandError> {
        ParsedTankCommand::try_from(TankCommand {
            throttle: self.throttle,
            steer: self.steer,
            brake: self.brake,
            aim: self.aim,
            range: self.range,
            ..default()
        })
    }
}

/// Primary clicks and secondary trigger transitions follow the device bindings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(super) enum InputEdge {
    Fire,
    TriggerDown,
    TriggerUp,
    Select(RoundSelect),
    Fuze(u8),
    Swap(CrewSwap),
    Repair,
    Respawn,
}
impl InputEdge {
    fn slot(self) -> usize {
        match self {
            Self::Fire => 0,
            Self::TriggerDown | Self::TriggerUp => 1,
            Self::Select(_) => 2,
            Self::Fuze(_) => 3,
            Self::Swap(_) => 4,
            Self::Repair => 5,
            Self::Respawn => 6,
        }
    }
    /// Firing reads the loaded round before selection and fuze changes. Other action consumers
    /// have no declared mutual order, so a tick boundary preserves their id order.
    fn shares_tick_after(self, previous: Self) -> bool {
        matches!(
            (previous, self),
            (Self::Fire, Self::Select(_) | Self::Fuze(_)) | (Self::Select(_), Self::Fuze(_))
        )
    }
    fn withdrawal(self) -> bool {
        matches!(self, Self::TriggerUp | Self::Swap(CrewSwap::Cancel))
    }
    fn write(self, command: &mut TankCommand, secondary: &mut bool) {
        match self {
            Self::Fire => command.fire_primary = true,
            Self::TriggerDown => *secondary = true,
            Self::TriggerUp => *secondary = false,
            Self::Select(value) => command.select_round = Some(value),
            Self::Fuze(value) => command.toggle_fuze = Some(value),
            Self::Swap(value) => command.crew_swap = Some(value),
            Self::Repair => command.repair = true,
            Self::Respawn => command.respawn = true,
        }
    }
}

/// Cumulative unanswered edges use contiguous wrapping ids; no client clock crosses the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct TankInput {
    pub seq: u16,
    pub levels: InputLevels,
    pub first_id: u16,
    pub edges: Vec<InputEdge>,
    /// Edges from this index were first sent in this message or the previous one.
    pub fresh_from: u8,
}

/// The owner's cumulative application/refusal record, read immediately on arrival.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct InputAnswer {
    pub seq: u16,
    pub resolved: u16,
    pub refused_mask: u16,
    pub fire_secondary: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum InputError {
    TooManyEdges(usize),
    SkippedId { first: u16, next: u16 },
    FreshIndex(u8),
    Levels(CommandError),
    AnswerBeyondIssued(u16),
    RegressedAnswer(u16),
}
impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InputError {}

/// Half-range ordering; the outstanding window is bounded far below half the id space.
fn after(a: u16, b: u16) -> bool {
    (a.wrapping_sub(b) as i16) > 0
}

struct Decided {
    edge: InputEdge,
    refused: bool,
}

#[derive(Component, Default)]
pub(super) struct ArrivalInput {
    answer: InputAnswer,
    levels: ParsedTankCommand,
    queue: VecDeque<Decided>,
    tank: Option<Entity>,
    staged: ParsedTankCommand,
    last_refusal: Option<InputError>,
}
impl ArrivalInput {
    fn accept(&mut self, input: TankInput, recovery: bool) -> Result<(), InputError> {
        if input.edges.len() > PENDING {
            return Err(InputError::TooManyEdges(input.edges.len()));
        }
        if usize::from(input.fresh_from) > input.edges.len() {
            return Err(InputError::FreshIndex(input.fresh_from));
        }
        let next = self.answer.resolved.wrapping_add(1);
        if after(input.first_id, next) {
            return Err(InputError::SkippedId {
                first: input.first_id,
                next,
            });
        }
        let levels = input.levels.parse().map_err(InputError::Levels)?;
        // Validate the whole packet before mutating levels, fate, or the echo.
        let admitted = self.answer.resolved.wrapping_add(self.queue.len() as u16);
        let new: Vec<_> = input
            .edges
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, _)| after(input.first_id.wrapping_add(*i as u16), admitted))
            .collect();
        if self.queue.len() + new.len() > PENDING {
            return Err(InputError::TooManyEdges(self.queue.len() + new.len()));
        }
        self.levels = levels;
        self.answer.seq = input.seq;
        for (index, edge) in new {
            self.queue.push_back(Decided {
                edge,
                refused: recovery && index < usize::from(input.fresh_from) && !edge.withdrawal(),
            });
        }
        Ok(())
    }

    fn command(&mut self, stale: bool, lost: bool) -> ParsedTankCommand {
        let mut command = self.levels.command();
        let mut occupied = [false; 7];
        let mut previous = None;
        while let Some(front) = self.queue.front() {
            if !front.refused
                && (occupied[front.edge.slot()]
                    || previous.is_some_and(|previous| !front.edge.shares_tick_after(previous)))
            {
                break;
            }
            let front = self
                .queue
                .pop_front()
                .expect("the inspected queue front exists");
            self.answer.resolved = self.answer.resolved.wrapping_add(1);
            self.answer.refused_mask = (self.answer.refused_mask << 1) | u16::from(front.refused);
            if !front.refused {
                occupied[front.edge.slot()] = true;
                previous = Some(front.edge);
                front
                    .edge
                    .write(&mut command, &mut self.answer.fire_secondary);
            }
        }
        if stale || lost {
            self.answer.fire_secondary = false;
        }
        command.fire_secondary = self.answer.fire_secondary;
        if lost {
            ParsedTankCommand::hold()
        } else {
            self.levels.with_actions(command)
        }
    }
}

/// Raw local frame state. Scripts and the device feed each own a distinct composition.
#[derive(Resource, Default)]
pub(super) struct FrameInput {
    pub command: TankCommand,
    /// Both physical transitions were polled in this frame; the final level chooses their order.
    pub secondary_cycle: bool,
}

struct PendingEdge {
    id: u16,
    edge: InputEdge,
    copies: u8,
    pressed_s: f64,
}

#[derive(Resource, Default)]
pub(super) struct ClientInput {
    pending: VecDeque<PendingEdge>,
    issued: u16,
    seq: u16,
    answer: InputAnswer,
    sent: VecDeque<(u16, f64)>,
    last_send_s: Option<f64>,
    button: bool,
    pub warning: bool,
    pub refused: u64,
}
impl ClientInput {
    fn enqueue(&mut self, edge: InputEdge, now: f64) -> bool {
        // Repeated withdrawals already covered by a pending id need no second request. Stop
        // searching at the corresponding start: a later start requires its own withdrawal.
        // With no matching pending action, the withdrawal still needs its own id.
        if edge.withdrawal()
            && self
                .pending
                .iter()
                .rev()
                .find_map(|pending| match (edge, pending.edge) {
                    (InputEdge::TriggerUp, InputEdge::TriggerUp) => Some(true),
                    (InputEdge::TriggerUp, InputEdge::TriggerDown) => Some(false),
                    (InputEdge::Swap(CrewSwap::Cancel), InputEdge::Swap(swap)) => {
                        Some(matches!(swap, CrewSwap::Cancel))
                    }
                    _ => None,
                })
                .unwrap_or(false)
        {
            return true;
        }
        let limit = if edge.withdrawal() {
            PENDING
        } else {
            PENDING - WITHDRAWAL_RESERVE
        };
        if self.pending.len() >= limit {
            return false;
        }
        self.issued = self.issued.wrapping_add(1);
        self.pending.push_back(PendingEdge {
            id: self.issued,
            edge,
            copies: 0,
            pressed_s: now,
        });
        true
    }
    pub(super) fn issue(&mut self, edge: InputEdge, now: f64) -> bool {
        let issued = self.enqueue(edge, now);
        if !issued {
            self.refused += 1;
        }
        issued
    }
    fn receive(&mut self, answer: InputAnswer, now: f64) -> Result<(), InputError> {
        if after(answer.resolved, self.issued) {
            return Err(InputError::AnswerBeyondIssued(answer.resolved));
        }
        if after(self.answer.resolved, answer.resolved) {
            return Err(InputError::RegressedAnswer(answer.resolved));
        }
        if usize::from(answer.resolved.wrapping_sub(self.answer.resolved)) > self.pending.len() {
            return Err(InputError::AnswerBeyondIssued(answer.resolved));
        }
        while self
            .pending
            .front()
            .is_some_and(|edge| !after(edge.id, answer.resolved))
        {
            let edge = self
                .pending
                .pop_front()
                .expect("the inspected pending edge exists");
            let refused = answer.refused_mask & (1 << answer.resolved.wrapping_sub(edge.id)) != 0;
            if refused && !matches!(edge.edge, InputEdge::TriggerDown if self.button) {
                self.refused += 1;
            }
            let latency = (now - edge.pressed_s) * 1e3;
            info!(
                id = edge.id,
                refused,
                latency_ms = latency,
                "input answer: MEASURED click-to-answer"
            );
        }
        self.answer = answer;
        while self
            .sent
            .front()
            .is_some_and(|&(seq, _)| !after(seq, answer.seq))
        {
            self.sent.pop_front();
        }
        Ok(())
    }
    fn secondary_held(&self) -> bool {
        let mut held = self.answer.fire_secondary;
        for edge in &self.pending {
            match edge.edge {
                InputEdge::TriggerDown => held = true,
                InputEdge::TriggerUp => held = false,
                _ => {}
            }
        }
        held
    }
    fn message(&mut self, levels: InputLevels, now: f64, fresh_copies: u8) -> TankInput {
        self.seq = self.seq.wrapping_add(1);
        let fresh_from = self
            .pending
            .iter()
            .position(|edge| edge.copies < fresh_copies)
            .unwrap_or(self.pending.len()) as u8;
        let message = TankInput {
            seq: self.seq,
            levels,
            first_id: self
                .pending
                .front()
                .map_or(self.issued.wrapping_add(1), |edge| edge.id),
            edges: self.pending.iter().map(|edge| edge.edge).collect(),
            fresh_from,
        };
        for edge in &mut self.pending {
            edge.copies = edge.copies.saturating_add(1).min(fresh_copies);
        }
        // Resolution: the first unechoed send must survive a blackout. Later sends may be
        // omitted from this display-only ring; an echo beyond them also retires its prefix.
        if self.sent.len() < 64 {
            self.sent.push_back((self.seq, now));
        }
        self.last_send_s = Some(now);
        message
    }
}

pub(super) struct TankInputChannel;
pub(super) struct InputAnswerChannel;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct InputBridge;

pub(super) fn install(app: &mut App) {
    app.init_resource::<Timing>()
        .init_resource::<FrameInput>()
        .init_resource::<ClientInput>()
        .add_observer(connect)
        .add_observer(reset)
        .add_systems(
            PreUpdate,
            (receive_inputs, receive_answers).after(MessageSystems::Receive),
        )
        .add_systems(
            FixedUpdate,
            (prepare_commands, apply_inputs)
                .chain()
                .in_set(InputBridge)
                .after(crate::command::ParseCommands)
                .before(GameplaySet),
        )
        .add_systems(FixedPostUpdate, answer_inputs)
        .add_systems(PostUpdate, send_inputs.before(MessageSystems::Send));
}
fn connect(event: On<Add<Connected>>, links: Query<(), With<ClientOf>>, mut commands: Commands) {
    if links.contains(event.entity) {
        commands
            .entity(event.entity)
            .insert((InputGap::default(), ArrivalInput::default()));
    }
}
fn reset(_: On<Remove<Connected>>, mut state: ResMut<ClientInput>, mut frame: ResMut<FrameInput>) {
    // Only the client uses these resources; the dedicated server's state lives on each link.
    // Entity teardown can remove Client before Connected, so no role query may gate this reset.
    *state = default();
    *frame = default();
}
fn receive_inputs(
    time: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    timing: Res<Timing>,
    mut links: Query<
        (
            Entity,
            &mut InputGap,
            &mut ArrivalInput,
            Option<&mut MessageReceiver<TankInput>>,
        ),
        (With<ClientOf>, With<Connected>),
    >,
) {
    links
        .par_iter_mut()
        .for_each(|(link, mut gap, mut input, receiver)| {
            let message = receiver.and_then(|mut receiver| receiver.receive().last());
            let now = time.elapsed_secs_f64();
            let tick = fixed.timestep().as_secs_f64();
            gap.expire(now, &timing);
            let recovery = gap.silence_s() > gap.threshold(tick, &timing);
            let arrived = match message {
                Some(message) => match input.accept(message, recovery) {
                    Ok(()) => {
                        input.last_refusal = None;
                        true
                    }
                    Err(cause) => {
                        if input.last_refusal.as_ref() != Some(&cause) {
                            warn!(?link, %cause, "input refused: malformed TankInput; levels unchanged");
                            input.last_refusal = Some(cause);
                        }
                        false
                    }
                },
                None => false,
            };
            gap.receive(arrived, now, time.delta_secs_f64(), tick, &timing);
            if gap.stale() {
                input.answer.fire_secondary = false;
            }
        });
}
fn prepare_commands(
    mut links: Query<(Entity, &InputGap, &mut ArrivalInput), (With<ClientOf>, With<Connected>)>,
    tanks: Query<(Entity, &ControlledBy)>,
) {
    links.par_iter_mut().for_each(|(link, gap, mut input)| {
        let tank = tanks
            .iter()
            .find_map(|(tank, owner)| (owner.owner == link).then_some(tank));
        if input.tank != tank {
            input.answer.fire_secondary = false;
            input.tank = tank;
        }
        input.staged = input.command(
            gap.stale(),
            gap.silence_s() > super::server::connection_timeout().as_secs_f64(),
        );
    });
}
fn apply_inputs(
    links: Query<&ArrivalInput, With<Connected>>,
    mut tanks: Query<(&ControlledBy, &mut TankCommand, &mut ParsedTankCommand)>,
) {
    tanks
        .par_iter_mut()
        .for_each(|(owner, mut raw, mut parsed)| {
            *parsed = match links.get(owner.owner) {
                Ok(input) => input.staged,
                // The owner may disconnect before its session-lifetime tank despawns.
                Err(_) => ParsedTankCommand::hold(),
            };
            *raw = parsed.command();
        });
}
fn answer_inputs(
    mut links: Query<
        (&ArrivalInput, &mut MessageSender<InputAnswer>),
        (With<ClientOf>, With<Connected>),
    >,
) {
    links
        .par_iter_mut()
        .for_each(|(input, mut sender)| sender.send::<InputAnswerChannel>(input.answer));
}
fn receive_answers(
    time: Res<Time<Real>>,
    mut state: ResMut<ClientInput>,
    mut links: Query<&mut MessageReceiver<InputAnswer>, (With<Client>, With<Connected>)>,
) {
    for mut receiver in &mut links {
        if let Some(answer) = receiver.receive().last()
            && let Err(cause) = state.receive(answer, time.elapsed_secs_f64())
        {
            warn!(%cause, "input answer refused: malformed server answer");
        }
    }
}
pub(super) fn send_inputs(
    time: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    timing: Res<Timing>,
    mut frame: ResMut<FrameInput>,
    mut state: ResMut<ClientInput>,
    mut links: Query<&mut MessageSender<TankInput>, (With<Client>, With<Connected>)>,
) {
    if links.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let command = frame.command;
    if command.fire_primary {
        state.issue(InputEdge::Fire, now);
    }
    if let Some(value) = command.select_round {
        state.issue(InputEdge::Select(value), now);
    }
    if let Some(value) = command.toggle_fuze {
        state.issue(InputEdge::Fuze(value), now);
    }
    if let Some(value) = command.crew_swap {
        state.issue(InputEdge::Swap(value), now);
    }
    if command.repair {
        state.issue(InputEdge::Repair, now);
    }
    if command.respawn {
        state.issue(InputEdge::Respawn, now);
    }
    if frame.secondary_cycle {
        let (first, last) = if command.fire_secondary {
            (InputEdge::TriggerUp, InputEdge::TriggerDown)
        } else {
            (InputEdge::TriggerDown, InputEdge::TriggerUp)
        };
        if state.issue(first, now) {
            state.issue(last, now);
        }
        frame.secondary_cycle = false;
    }
    let wanted = command.fire_secondary;
    if wanted != state.secondary_held() {
        let issued = state.enqueue(
            if wanted {
                InputEdge::TriggerDown
            } else {
                InputEdge::TriggerUp
            },
            now,
        );
        if !issued && wanted && !state.button {
            state.refused += 1;
        }
        assert!(
            wanted || issued,
            "the pending window reserves the trigger withdrawal"
        );
    }
    state.button = wanted;
    frame.command.clear_edges();
    state.warning = state.pending.len() >= PENDING - WITHDRAWAL_RESERVE
        || state
            .sent
            .front()
            .is_some_and(|&(_, at)| now - at > timing.link_warning_s);
    if state.pending.iter().any(|edge| edge.copies == 0)
        || state
            .last_send_s
            .is_none_or(|last| now - last >= fixed.timestep().as_secs_f64())
    {
        let message = state.message(command.into(), now, timing.recovery_fresh_copies);
        for mut sender in &mut links {
            sender.send::<TankInputChannel>(message.clone());
        }
    }
}

#[cfg(test)]
mod tests;
