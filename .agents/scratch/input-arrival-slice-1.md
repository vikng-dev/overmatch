# Input by arrival, fate by link state — slice 1

Subject: Overmatch's fire control and damage model, game state only. Crew are HP pools at stations.

Implemented on `lane/input-arrival`. The tank-command native-input plugin is replaced by `TankInput` and owner-only `InputAnswer` on separate sequenced-unreliable channels. `PROTOCOL_REV` is 103 (base: 102); both wire manifest hashes are re-pinned and their production-registration tests pass. No vendor change is required.

## Laws and player feedback

| Law | Built behavior and owner |
|---|---|
| U1 | `input_arrival::send_inputs` sends once in PostUpdate, before MessageSystems::Send. Real elapsed time gates ordinary sends to the fixed interval; a newly issued edge forces that frame's send. Devices feed once per frame, including frames with no fixed tick. Scripted clicks enqueue independently so multiple fixed ticks cannot overwrite one another. |
| U2 | `ClientInput` retains every issued edge until its cumulative answer, resending the bounded pending set. No age, server estimate, synced flag or predicted tick decides fate. Backpressure visibly refuses issuance; it never voids an issued edge. |
| U3 | The installed PreUpdate receiver drains once after MessageSystems::Receive. Each connection independently advances slice 0's InputGap: arrival resets silence; an empty receive charges at most one fixed interval. |
| U4 | The existing Timing owner supplies the floor, ceiling and retention window from `assets/net_timing.ron`. S is the retained maximum plus the measurement grain, clamped to those bounds. Stale gaps enter capped at the ceiling. Awaiting-first-arrival records no silence or gap. |
| U5 | Recovery is classified against S before the new gap enters history. Non-fresh starts receive a fixed refused decision; fresh starts and withdrawals are admitted. |
| U6 | The connection's bounded ordered decision ledger fixes fate on arrival. Retransmitted ids already admitted or resolved do not enqueue again. A later refusal cannot acknowledge past an accepted edge still awaiting its command slot. |
| U7 | Parallel per-link preparation stages each command; parallel per-tank application uses ControlledBy.owner before GameplaySet. Absolute levels precede actions. Accepted edges apply once in id order. Duplicate command slots and order-sensitive cross-kind combinations wait for another tick. |
| U8 | The same InputGap stale state releases both trigger-route bits. Movement levels hold. A queued trigger action cannot keep a stale link firing. |
| U9 | TriggerUp and Swap(Cancel) bypass stale-start refusal. Three pending slots are reserved for these withdrawals; repeated pending withdrawals reuse the id unless a corresponding start intervenes. |
| U10 | Input silence beyond the pinned transport timeout produces ParsedTankCommand::hold, including brakes, even if authenticated keepalives maintain the connection. Disconnect keeps the existing session-lifetime tank despawn. |
| U11 | Packet size, contiguous-id admission, fresh index, trigger route and today's CommandError level checks precede mutation. Malformed network input gets InputError and a visible warning; checked levels remain intact. Malformed answers cannot resolve unissued ids or shift outside the refused mask. |

The primary physical press retains the existing immediate switch click; the secondary press gets the same feedback. Client device commands cannot create a shot: the shipped firing plugin gates its fire system to the authority. The report, flash, recoil and game round come from the server fire fact. Refusals produce a distinct dry click and brief `!` reticle mark. `NO LINK` is display only; held automatic re-presses produce no repeated refused cue. The new rendered strings pass bundled-font coverage.

Trigger addresses are Primary/Secondary binding routes, which weapon data selects, rather than weapon-slot ordinals. Round selection and fuze addressing retain their existing weapon-slot semantics.

## Source audit: every “Not verified” item

Registry citations below name the pinned packages under `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`; transport netcode uses the vendored fork.

| Question | Answer and source |
|---|---|
| Owner-only per-connection send | `lightyear_messages-0.31.0/src/send.rs::MessageSender::send` buffers onto the sender's entity Transport. `answer_inputs` sends on each connected ClientOf entity, with that link's answer. No broadcast target or message entity supplied by the peer is involved. |
| Receive once per frame | `lightyear_messages-0.31.0/src/plugin.rs` registers Receive in PreUpdate after TransportSystems::Receive, Send in PostUpdate and clear in Last. `receive.rs::MessageReceiver::receive` drains ready messages. Our receive runs after that set. `src/net/server_runner.rs::run_one_admitted_tick` runs at most one authority tick per frame. |
| Newest wins | `lightyear_transport-0.31.0/src/channel/receive.rs::RecvState::{prepare_receive,push_completed}` invalidates an undrained older completion when a newer sequenced message arrives. Application receipt is immediate, with no presentation-timeline registration. |
| Directional loss lever | Added test support only: `input_arrival/tests.rs::{Loss,condition,install_conditioner}` drops opaque received datagrams on either endpoint, after LinkReceiveSystems::ApplyConditioner and before decoding. Independent uplink/downlink blackouts and seeded loss are exercised over actual UDP sockets. |
| Input timestamps | `bevy_input-0.20.1/src/keyboard.rs::KeyboardInput` and `mouse.rs::MouseButtonInput` have state and window, with no press timestamp. ButtonInput exposes both transitions and the final level; the adapter preserves one sub-frame trigger cycle. It cannot reconstruct arbitrary event chronology from flags. |
| Order within a tick | `shooting::plugin` orders fire before apply_round_selection; that consumer applies selection before fuze changes. Repair/swap/respawn have no declared mutual order. Only known forward fire → selection → fuze combinations share a tick; other combinations take a tick boundary. Trigger down/up share a slot. |
| Tank on disconnect | `lightyear_replication-0.31.0/src/control.rs::ControlledBy::handle_disconnection` despawns Lifetime::SessionBased entities, the default used by `server::spawn_player_tank`. The bridge holds a disconnected owner's command during teardown. |
| Timeout counts the same silence? | No. `vendor/lightyear_netcode-0.31.0-admission/src/server_plugin.rs::NetcodeConfig::default` owns the transport timeout; `server.rs` checks authenticated connection traffic. Our `net::server::connection_timeout` reads that owner. Tank-input silence has its own counter, using the same timeout value, so U10 also works while keepalives continue. |
| Respawn timeout | Retained. `death_screen::{request_respawn,toggle_death_screen,respawn_timed_out}` waits for application admission or a live tank. InputAnswer “applied” only promises delivery into the tick's command, not successful respawn admission. Removing the application wait would conflate these outcomes. |
| Message sizes | Measured below using Lightyear's selected postcard payload serializer. The design's approximate sizes are replaced with actual fixture sizes and maximum-width shapes. |
| Assumed tank speed | Not used by any new law or assertion. No physical speed measurement is claimed; the design's blind-travel illustration remains an assumption. |
| #390 wording | The trigger down/up pair here is uplink input. Reliable server “started”/“stopped” burst presentation and #391 remain slice 4; this lane adds neither. |
| Atomic application of a tick | A stamp's arrival does not imply whole-tick application. `bevy_replicon-0.45.5/src/client/server_mutate_ticks.rs::MutateTickReceived` means all mutation messages applied. Slice 0's `view_clock::receive` already drains every completion and correlates through ReplicationCheckpointMap. Slice 2 must preserve that boundary and the two-part stamp. |
| c_c, c_s and R | Not separately measured. Input-to-answer measurements below do not establish display latency, a paced display budget, or the design's total click-to-screen formula. |
| Remaining local-clock readers | `protocol::publish_shot_clock`, `client::receive_fire_events`, deferred-fire expiry and `age_sanctioned_shots` still use LocalTimeline for presentation. `lightyear_sync-0.31.0/src/timeline/input.rs::LocalTimeline::sync_objective` adds error_margin to the objective itself. Infinity is therefore not a safe way to make this controller inert. |

## Design claims corrected

- A first input has no preceding arrival gap. The accepted slice 0 awaiting-first-arrival correction is preserved.
- A press on the first recovery frame alone does not guarantee admission. Its fresh first copy must be the newest payload when the server drains. A later non-fresh resend can supersede that undrained copy even without injected packet loss, and is then refused by U5. The fresh-frame UDP proofs explicitly await first-copy receipt before allowing that replacement. This follows the actual sequenced-channel semantics; the narrative guarantee was too strong.
- The claimed late bound is not a click-age bound. A first copy still marked fresh can spend arbitrarily long in transit; a fresh recovery press is also admitted after long silence. Accepted edges can additionally wait for command slots or a slow server. No client clock is sent, so neither age nor one-way transit delay is observable here.
- The strongest “server stall can never cause stale” prose exceeds U3. Its cap prevents charging the entire server stall to silence; it does not promise that a link already near its threshold, or missing packets across a stall, can never cross it. History still expires in real time. The required clean-link stalled-server case is proved; no hosted slow-server run is claimed.
- Edge kinds can be order-sensitive. Slot uniqueness alone does not preserve cross-kind order through today's consumers; the bridge adds the necessary tick boundaries.
- A refusal may be decided immediately while its cumulative answer waits behind an accepted earlier edge. Advancing resolved across that edge would falsely acknowledge an application.
- Per-weapon trigger bits do not match the existing input contract. The wire addresses binding routes, allowing generic weapon data to select them without assuming their slot order.
- Filling every pending slot with starts can strand a withdrawal during downlink loss. The bounded set reserves one slot per trigger release and one swap cancellation (DERIVED reserve: three). Starts backpressure at P minus that reserve. The link warning also shows at backpressure, since rapid presses can fill the window before its age warning.
- The proposed local-controller removal must follow the presentation readers into slice 2. **Input has no server-clock estimate or click-fate timer; the client as a whole still has its existing framework presentation estimate.** Making it free-run now would let a slow authority's current fire facts fail age admission. Input-specific estimation, configuration and synced gating are deleted. The scripted harness's local-tick reads are diagnostics, not send/fate decisions.
- The authored-number inventory omitted the refused cue's duration and pitch. They are required, labelled presentation tastes in net_timing.ron. The existing shared audio tastes are now owned by sfx.ron, and the firing module's existing recoil taste by shooting.ron; neither was tuned.
- An applied Respawn edge does not guarantee admission. The existing application-level wait remains.

## Required real-UDP proofs

Every row passes. The fixture mounts the production protocol/arrival installer, command core and authority firing plugin, with real client/server UDP transports. Shots are actual FireShell emissions, not a fabricated input counter. Endpoint message-id agreement is asserted. Combat-start below mounts the shipped binaries as an additional composition proof.

| Test in `src/net/input_arrival/tests.rs` | What it shows |
|---|---|
| stalled_frame_plus_click_applies_exactly_one_shot | A click polled after a stalled client frame produces one authority shot and one applied answer. |
| thirty_percent_uplink_loss_applies_one_click_exactly_once | Resends survive seeded opaque 30% uplink loss without replaying the shot. |
| uplink_blackout_refuses_an_old_click_and_answers_its_fate | Blackout beyond the shipped ceiling yields no shot and a refused answer for the old start. |
| uplink_recovery_applies_a_fresh_click | The delivered first recovery-frame copy applies once. The fixture waits for real socket receipt before allowing a later resend to replace its fresh marker. |
| downlink_blackout_never_refuses_an_applied_click | Upstream click applies once while answers are absent; warning is display only. Recovery says applied, never refused. |
| held_trigger_stops_by_stale_threshold_and_resumes_without_a_physical_press | Fire stops by S + T, remains stopped during silence, then the single answered-trigger rule resumes without a physical re-press. |
| a_release_lost_in_blackout_stays_released_after_recovery | Stale releases fire; the late withdrawal cannot restart it. |
| a_tap_inside_one_frame_holds_the_trigger_for_exactly_one_tick | Down/up in one frame yields one held tick and one fixture shot. |
| a_two_hundred_ms_server_stall_does_not_declare_a_live_client_stale | A clean live link retains its trigger state across the required 200 ms server-frame disturbance. |
| three_round_select_steps_under_loss_apply_exactly_three_steps | Three pending relative steps produce exactly three authority selections, read against the fixture weapon's own list. |
| malformed_message_is_refused_with_levels_unchanged | A malformed UDP level receives a typed refusal and log, preserves checked levels and produces no shot. |

Additional proofs in the same module:

- `refusals_never_acknowledge_past_an_accepted_edge_waiting_for_its_slot`: an accepted prefix blocks cumulative acknowledgement of a later refusal.
- `movement_holds_at_stale_and_brakes_at_input_loss_while_keepalives_continue`: stale holds throttle; lost input brakes while the authenticated link remains connected.
- `the_bounded_pending_window_reserves_withdrawals_and_reports_full_presses`: full starts are visibly refused and each withdrawal remains issuable, including repeated withdrawals.
- `wrapped_ids_keep_each_refusal_and_each_application_exact`: wrapping preserves individual fates and exactly-once application.
- `malformed_shapes_and_routes_have_typed_refusals_before_any_mutation`: edge count, skipped id, fresh index and route fail before mutation.
- `malformed_answers_leave_pending_fates_undecided`: an outside answer cannot acknowledge unissued ids or cause an out-of-width fate-mask shift.
- `trigger_refusal_cues_only_after_the_physical_button_is_released`: a refused tap gets feedback; held automatic retries remain quiet.
- `despawning_a_connection_resets_its_pending_edges_and_frame_state`: teardown ordering cannot carry old ids into another connection.
- `the_bridge_applies_only_the_controlling_links_command_and_holds_disconnected_owners`: owner isolation, bot-producer isolation and teardown hold.
- The three `measure_*` tests print payload sizes and loopback latency as observations, with no historical-output assertion.

`client::arrival_device_tests` proves edge drainage on frames without fixed ticks and retention of a sub-frame trigger tap. `shooting::tests::a_replica_never_fires_from_local_device_commands` mounts the shipped plugin and catches removal of its authority gate. Existing next-load and fuze tests retain the consumer-order evidence. The obsolete test expecting local client fire is deleted.

## Harness and source corrections

The extended keepalive proof exposed an incomplete UDP fixture: without ReplicationReceiver, `lightyear_replication-0.31.0/src/client.rs::receive_client_packets` does not drain raw replication channels before MessageSystems::Receive. Raw replication could then decode as generic metadata and fail duration conversion. The fixture now includes the production marker. The tagged probe was removed; no vendor workaround remains.

Connection teardown resets client frame/pending resources without querying for a Client marker that may already have been removed. Per-server-link state stays on its connection entity.

Read the abandoned lane with `git show lane/first-input:src/net/death_flow_input.rs`. Carried the stalled-frame idea and its actual FireShell observation, not its tick filing, attestation, coverage or local-fire implementation.

## Measurements

MEASURED postcard payload sizes:

| Shape | Bytes |
|---|---:|
| TankInput, aimed one-click fixture | 31 |
| TankInput, aimed idle fixture | 30 |
| TankInput, bounded maximum-width fixture | 98 |
| InputAnswer, zero-valued fields | 5 |
| InputAnswer, maximum-width fields | 11 |

MEASURED real-UDP manual-clock loopback: **4.921 ms wall time**; the client frame-clock observation was **15.625 ms**.

MEASURED final combat-start runs (scripted click issuance to answer processing):

| Run | Authority combat tick | First authority shot tick | Shots | Script ticks | Client exit | Answer latency, ms |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 99 | 118 | 1 | 256 | 0 | 20.058 |
| 2 | 99 | 100 | 1 | 256 | 0 | 76.273 |
| 3 | 100 | 106 | 1 | 256 | 0 | 11.959 |

All three first-fire input ticks equal their client release ticks; every authority shot is after combat release. These observed tick offsets are not regression targets. Logs and summary.json files are in `/tmp/input-arrival-combat-1`, `-2` and `-3`.

The payload measurements exclude message ids, channel framing, packet headers, netcode authentication and IP/UDP headers. The maximum TankInput fixture uses the bounded edge count, maximum ids, aim and the largest edge shape; it is not a promise of a fixed datagram size.

The loopback test advances a manual frame clock faster than wall time. Its wall measurement is an observation, not a paced 64 Hz or click-to-screen claim. Runner measurements use each process's actual Time<Real> click-to-answer log. All listed answers are applied, not refused.

## Verification

MEASURED results on this lane:

| Check | Result |
|---|---|
| Formatting and whitespace | clean |
| Locked workspace/all-target CI clippy | passed, warnings denied |
| net::, including protocol and all required UDP scenarios | 426 passed, 7 ignored, zero failures |
| tests::doc_citations | 3 passed, 0 ignored, zero failures |
| shooting:: | 36 passed, 0 ignored, zero failures |
| sfx:: | 8 passed, 0 ignored, zero failures |
| tests::ui_ascii | 4 passed, 0 ignored, zero failures |
| Full library suite | 2834 passed, 50 ignored, zero failures; 299.22 s |
| Final combat-start runner | 3/3 passed |

The full suite covers the final production implementation. Afterwards, the stalled-frame test adopted the existing explicit first-copy socket-delivery wait; all targeted checks were repeated. No production behavior changed after the full-suite and runner proofs. Final clippy, formatting and whitespace checks passed after that test-only adjustment.

Commands: `cargo fmt --all --check`; locked workspace/all-target clippy with `--features dev_ui,bitprobe -- -D warnings`; `scripts/serial-cargo test --lib --features dev_ui,bitprobe net::`; the doc-citation, shooting, sfx and UI coverage filters; full library suite; `python3 scripts/shot/run-combat-start.py --out /tmp/input-arrival-combat-N` for each run. All compiling cargo uses the serial queue. The runner owns OVERMATCH_UNATTENDED=1 and its headless window mode.

Builds of the development binaries emitted the macOS linker warning that the exception-unwind section exceeds compact-unwind encoding capacity; the build and every runner exited successfully. CI clippy is clean. No hosted deployment, display-latency measurement or visual/audio playtest is claimed. The orchestrator's six independent lens reviews and merge remain outside this lane.

## Deletions and data ownership

DERIVED from `git diff HEAD --numstat` against the lane base; gross removed lines include replaced lines.

| Entire file deleted | Removed lines | Retired purpose |
|---|---:|---|
| `src/net/fire_presentation.rs` | 983 | Local fire authorship, cadence, reconciliation and tests |
| `src/net/death_flow_input.rs` | 660 | Old native-input death-flow integration |
| `src/net/input_cadence.rs` | 353 | Old cadence model and tests |
| `src/net/tests/net_fire_release.rs` | 601 | Attestation/release variants and tests |
| `src/net/tests/net_input_buffer_wrap.rs` | 83 | Native input buffer wrap tripwire |

| Edited file | Gross removed lines | Replaced items |
|---|---:|---|
| `src/net/client.rs` | 688 | Native feed, cadence, stamping, buffer guards and their tests |
| `src/net/protocol.rs` | 554 | Native bridge/attestation, input wire layout and tests |
| `src/net/server_stats.rs` | 343 | Native input-buffer and attestation census |
| `src/net/sync_margin.rs` | 413 | Uplink estimator/configuration, statistics and tests |
| `src/net/harness.rs` | 117 | Native/synced input filing and its tests |
| `src/shooting.rs` | 177 | Fused/local replica fire path and obsolete client-fire tests |
| `src/command.rs` | 54 | Tick attestation, native mapping and secondary wire level |
| `.agents/docs/adr/0022-input-attestation-not-detection.md` | 78 | Retired mechanism instructions, replaced by a superseded record |

Total gross deleted lines across tracked edits: **5396 DERIVED**. This is an inventory, not a quality metric.

Deleted contracts and their callers/tests: `TankCommand::for_tick`, `TankCommand::fail_consumables_closed`, `ParsedTankCommand::attest`, the native-input MapEntities implementation, `protocol::attest` and `bridge_action_state_to_tank_command`, `claim_input_slot`, `feed_action_state`, `stamp_input_tick`, `SHIPPING_INPUT_DELAY_TICKS`, `shipping_input_delay`, `InputSendCadence`, its install/decision/count systems, the native buffer guard, AuthoredIntent, pending_local and the fire_secondary network level. `fire_secondary` remains only the real physical binding name. Internal trigger bits are authority-held state; they are absent from InputLevels.

Removed the input_native Cargo feature and direct lightyear_inputs/lightyear_inputs_native dev dependencies. Removed the uplink deriving configuration, state, constants, estimator statistics and tests; the downlink estimator still has presentation users. Native input buffer/attestation censuses are gone; per-link input-gap census remains. ADR-0022 is a concise superseded record, and product/current architecture descriptions name server-origin shot presentation.

DERIVED by one source comparison: the eight migrated audio values retain their previous values. The recoil momentum scale also retains its previous value; one before/after comparison of five registered fixture bindings produced identical software recoil observations. No output pin was added. shooting.ron is included in Compatibility's content record, so a differing response refuses at connect.

Closed backlog rows net-12 (deleted retired test path) and view-12 (audio data ownership). net-22's turret-on-arrival finding is deliberately left for slice 3/#187, which this task forbids building. Citation-only and mechanical adapter edits preserve pre-existing behavior and findings in their surrounding items; they do not claim to repair unrelated modules.

## What slice 2 needs

Drive presentation from view_clock, preserving both TickStamp facts (frame start and departure) and whole-tick MutateTickReceived completion. Replace the remaining local-clock shot admission, deferred expiry, path retirement and clock publication readers before disabling/removing the framework presentation controllers. Then delete the downlink estimator, interp_delay, resync and their tests where their users have moved; do not leave an alternative timing path.

Turret rendering and cursor-driven gauges/crew status remain slice 3. Reliable automatic-fire started/stopped presentation remains slice 4. Movement never releases merely because the link is stale.
