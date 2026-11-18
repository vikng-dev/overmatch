# Server-stream motion during missing snapshots

Subject: the game's netcode and damage model; game state only.

All hulls, attached turrets, guns and public paths share one server-domain view cursor.
Own and remote turret yaw and gun elevation present the same sampled `ServoAngles`; neither
integrates on the client. Camera, aim intent, reticle and trigger click remain client-owned input
feedback. The actual-pointing marker follows the streamed muzzle. A completed stamp carries frame
start and serialization departure. Frame start maps authority ticks to monotonic wall time;
departure diagnoses server work in `usual_work_ms`. Completion requires every mutate message of
that tick; stale, incomplete or duplicate arrivals do not invent production samples.

For each completed tick, `x = previous completion-frame Real - frame start` retains the unknown
process-epoch offset. The previous read excludes the completing client's frame duration. Server
work, network delivery and whole-tick completion delay enter one lateness term. The target is
`median(received frame-start interval) + max_W(x)`. Adjacent received stamps include gaps over
lost ticks. There is **no outlier exclusion, clipping or percentile**: every admitted sample can
be the maximum until its local receipt age exceeds W. W is required `view_window_s` data.

Applied delay slews toward the target at at most v seconds per Real second; timing data with v
outside `(0, 1)` is refused at load. Inverting the frame-start map at `now - delay`, clamped to
the first and newest retained stamps, gives the target point; equal frame stamps choose the
latest tick. A target before the oldest retained stamp, as at startup before the delay covers the
first samples, is that stamp, so the stream rate reads no time the map does not hold. A stamp
out of tick order, or a positive interval too short for ticks times T over it to stay finite,
refuses the stream at admission through the visible match close, so pacing only ever holds a
finite rate.

The cursor does not follow that target segment by segment, which would replay each tick's
production interval as playback speed. It advances about the stream rate q, steered by an exact
critically damped step of natural frequency `omega = v / (kappa T)`. q is learned from map facts
only: each advance of the target point filters the map's mean slope over that advance at the
same omega, so a hold, which does not advance the point, freezes q. Neither the cursor nor the
statistics window enters q. The first cursor frame reads the slope across the retained map.
Production phase telescopes out of a slope, so a regular server at any speed below `1+v` plays
at that speed with no steady lead. The cursor's rate is stored absolute, so a change of q never
steps playback.

With q and the loop both at omega, the rate's response to target phase is the kernel
`omega^2 (3 - 5x + x^2) e^-x`, `x = omega t`, of zero area and L1 norm `2 kappa omega`;
`kappa = 0.8754` has a closed form, stated in `bandwidth`. DERIVED from it: production phase
within p peak to peak moves the rate by at most `v p / T`; a step d in the stream's speed leads or
lags the target by at most `0.840 |d| / omega` (28.7 ms for `d = 0.5`) and accelerates playback by
at most `0.80 |d| omega`; a slewing delay offsets the cursor from the target by `2 v / omega`
(27.4 ms) and accelerates it by at most `v omega / e`. Progress is forward-only, because each
frame's advance is clamped nonnegative and retained facts never retire the newest tick below the
cursor, and is capped at `(1+v)` simulation seconds per Real second; a released backlog closes at
that cap without overshoot.

A frame is held when its rate-limited request passes the newest completed tick by more than a
cursor quantum; a request that lands on a clamped target is not held. The cursor and the inverse
are exact inside the clock, and only the published cursor rounds to U0F16, so rounding never
feeds back into the loop. A hold extrapolates nothing. The frame after a hold, and after a cut,
resumes at q by decision: a hold is a gap in facts, not a change in the stream's speed. Release
is therefore a rate step from 0 to q. Target backlog measured in simulation ticks above
`max(view_cut_s, admission_s)` cuts forward once, warns and counts the discontinuity. Server time
admission and this continuity floor read the same required `admission_s` field; no client
virtual-time default chooses it.

The target covers lateness, not production phase. A smooth cursor needs the production phase
above its mean at the tick that sets the lateness maximum, and the nearest-rank median of
alternating production intervals is the short one about half the time; on a link without
lateness slack such production holds. A sudden slowdown holds until the median interval turns
over, about `W / (1+s)` after the step. A law within the phase bound cannot stop as abruptly as
one that follows the target segment by segment: while decelerating it leads, and at a link
without slack any lead is a contact. A slowdown's last hold may therefore trail the
segment-by-segment rule's by up to `14.4 / omega` (DERIVED 0.98 s; MEASURED +0.125 s at half
speed, 0 at three quarters, +0.208 s at nine tenths).

A server stall that fills the debt cap can cut once on release, and this is accepted. The cut
floor and the server's debt cap both read `admission_s` (250 ms as shipped), so the capped debt
alone brings the backlog close to the floor: a cursor resuming at full speed peaks at 239.1 ms
over real UDP and 218.8 ms headless (MEASURED, segment-by-segment rule). q dips through the
stall's long map segment, so this cursor resumes slowly, and its ramp adds 25 to 30 ms (MEASURED):
a 1 s capped stall peaks at 263.7 ms over real UDP and cuts once, 16.9 ticks, and at 248.1 ms
headless, just under the floor. The measurements and the open estimator question are in
[the pacing report](../../scratch/view-rate-396.md).

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
