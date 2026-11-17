# Server-stream motion during missing snapshots

Subject: the game's netcode and damage model; game state only.

All hulls and public paths share one server-domain view cursor. A completed stamp carries frame
start and serialization departure. Frame start maps authority ticks to monotonic wall time;
departure diagnoses server work in `usual_work_ms`. Completion requires every mutate message of
that tick; stale, incomplete or duplicate arrivals do not invent production samples.

For each completed tick, `x = previous completion-frame Real - frame start` retains the unknown
process-epoch offset. The previous read excludes the completing client's frame duration. Server
work, network delivery and whole-tick completion delay enter one lateness term. The target is
`median(received frame-start interval) + max_W(x)`. Adjacent received stamps include gaps over
lost ticks. There is **no outlier exclusion, clipping or percentile**: every admitted sample can
be the maximum until its local receipt age exceeds W. W is required `view_window_s` data.

Applied delay slews toward the target at at most v seconds per Real second. Inverting the
frame-start map gives a target cursor clamped to received endpoints. Ordinary cursor progress
is forward-only and capped at `(1+v)` simulation seconds per Real second. Equal frame stamps
choose the latest tick. A buffer exhaustion holds its newest completed tick; it extrapolates
nothing. Target backlog measured in simulation ticks above `max(view_cut_s, admission_s)` cuts
forward once, warns and counts the discontinuity. Server time admission and this continuity
floor read the same required `admission_s` field; no client virtual-time default chooses it.

The retained maximum makes server-work spikes and network spikes equally expensive. A spike can
produce a hold, a forward cut, then slow playback while delay grows and fast playback after the
sample expires. This is a feel decision, recorded with the median-compute alternative in
[playtest fork F21](../../scratch/playtest-forks/README.md#f21--server-work-and-link-lateness-share-one-view-budget).
The report records observed durations; tests pin monotonicity, received endpoints and ordinary
pace bounds rather than those emergent values.

Unknown recoil and collision impulses, gravity without supporting contact, contact on different
materials, and angular acceleration prevent a traction coefficient from certifying a positive
projection horizon. No projected pose or correction blend accompanies the held stream.
A future projection proposal must supply a complete motion, impulse and angular-error contract.

Before a completed stamp pair there is no view epoch: presentation samplers wait, announcements
hold and trace ticks are null. Disconnect resets the epoch; connect clears old receipts. Client
local transport counts remain independent, named separately in diagnostic rows. Current API
checks, fixtures and validation are recorded in [the lane report](../../scratch/view-clock-slice-2.md).
