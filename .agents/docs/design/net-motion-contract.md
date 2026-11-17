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
frame-start map gives a target cursor clamped to received endpoints; equal frame stamps choose
the latest tick. The cursor does not follow that target segment by segment, which would replay
each tick's production interval as playback speed. It advances at the stream's measured rate,
steered by a critically damped phase loop of natural frequency `omega = e v / T`. The measured
rate is the window's: T times the ticks its retained samples span over the stamp time they span,
capped at `1+v`. Production phase telescopes out of it, so a regular server at any speed up to
`1+v` plays at that speed with no steady lead, and alternating intervals do not reach the rate;
a nearest-rank median interval would flip between them and replay them as speed. After a speed
change the window mixes both speeds until it turns over, W and a link time later; the loop
carries the rate error meanwhile, with at most `2 |error| / omega` of lead or lag in the
continuous limit.
The loop's rate response to target phase has L1 norm `2 omega / e`, so production phase noise
within one tick peak to peak moves the rate by at most v, and a slewing delay changes the rate
by at most `v^2 / T` per second instead of stepping it. From zero rate deviation its response to
a phase step does not overshoot. Progress is forward-only, because each frame's advance is
clamped nonnegative and retained facts never retire the newest tick below the cursor, and is
capped at `(1+v)` simulation seconds per Real second; a released backlog closes at that cap. A
frame is held when the newest completed tick stops the cursor by more than a cursor quantum:
the cursor's request lies past that tick, or the inverse clamped the target to it. A hold stops
the cursor, which accelerates from rest on release; it extrapolates nothing. Target backlog measured in
simulation ticks above `max(view_cut_s, admission_s)` cuts forward once, warns and counts the
discontinuity. Server time admission and this continuity floor read the same required
`admission_s` field; no client virtual-time default chooses it.

The target covers lateness, not production phase. A smooth cursor needs the production phase
above its mean at the tick that sets the lateness maximum, and the nearest-rank median of
alternating production intervals is the short one about half the time; on a link without
lateness slack such production holds. A sudden slowdown holds until the window turns over: the
median interval and the window's rate both lag the new speed. The measurements and the open estimator
question are in [the pacing report](../../scratch/view-rate-396.md).

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
checks, fixtures and validation are recorded in [the lane report](../../scratch/view-clock-slice-2.md)
and, for pacing, [the pacing report](../../scratch/view-rate-396.md).
