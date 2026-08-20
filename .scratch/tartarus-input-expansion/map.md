Label: wayfinder:map

## Destination

Acheron reaches **v1.0**: feature-complete and released as a public, open source Linux tool for the Linux gaming community — still scoped to the Razer Tartarus Pro specifically (the only hardware owned/tested), but built without gratuitous obstacles to someone later adapting it for other Tartarus variants.

- **Feature-complete** means rough parity with Razer Synapse's remap/macro feature set for this device, minus cloud sync and lighting (already out of scope), plus deliberate extras already underway (Stepper). The v1.0 feature list is locked (see [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md)):
  - **Required floor**: the shipped MVP (Profiles, Layers, Keypress/Macro Actions, Fire-once/Hold-to-repeat/Toggle Trigger modes) + **Chord** Bindings + **mouse-button/full-keyboard output** with a GUI picker (now widened to include multimedia/consumer-control keys) + **Stepper** + **Profile Switch** + two polish tickets (context-menu cleanup, GUI sizing) + **reusable/named Macro entities** (a macro library, replacing today's inline-only Macro) + **the analog data model** (see below).
  - **Non-blocking, welcomed if ready in time**: two open-ended R&D strands — **grid-key analog capture** (feasibility now *settled* — see [the prototype](./issues/13-task-standalone-analog-capture-prototype.md) — but the integration is a capture-path rework, not an additive feature) and **Controller/Joystick output emulation** (userspace-emulated, explicitly not a kernel-level virtual device). Neither gates v1.0: the destination is reached once the required floor's frontier is empty, whether or not these two have landed; either fast-follows into a later release if it isn't ready.
  - **The analog split** (settled in the charting pass that graduated the analog fog, 2026-08-16): analog capture is now known to work, and the *data model* it needs — how depth, actuation points, and the analog/digital device-mode distinction are represented in `PhysicalEvent`, `Binding`, and the config schema — is promoted **into the required floor**, because every remaining Binding-editor ticket writes the exact surface that model lands on. The **capture-path rework** built on that model, and the features above it (analog trigger points, the Analog-repeat Trigger mode, real analog axes), stay **non-blocking**. The point of the split is that v1.0 must not ship a Binding/config shape that analog will force us to break — not that v1.0 must ship analog.
- **Release-ready** means a clean, final git repo, built and installed from source (`install.sh`) with documentation sufficient for a stranger to build, install, and use it, under a chosen open source license. **The "nothing forces packaging complexity" claim no longer fully holds**: the GUI is still pure userspace, but analog capture needs a udev rule granting access to the device's `/dev/hidraw*` node (see [the research](./issues/12-research-linux-analog-grid-key-protocol.md) §4.3), so `install.sh` gains a privileged step the moment the capture rework lands. Still far short of distro packaging, which stays out of scope.

Like the previous map, this one carries execution.

## Notes

**This map carries execution** — resolving a ticket means actually building and testing against the real, connected Tartarus Pro, not only deciding (same discipline as the previous map). This is also the de facto quality bar for "feature-complete": every ticket's own live-hardware verification plus the existing daemon/GUI test suites (72+28 tests as of the last MVP ticket) *are* the stability bar — no separate quality-bar ticket exists or is needed.

**Grounding facts found before charting this map:**
- `Action::Keypress`'s `key` field is already a bare `evdev::KeyCode`, not a curated keyboard-only enum — nothing in the Daemon validates it against a keyboard allow-list.
- The virtual `uinput` device (`daemon/src/injector.rs::build_device`) already declares the *entire* standard `EV_KEY` range via `input::all_injectable_key_codes()`, and `BTN_*` codes (`BTN_LEFT`, `BTN_RIGHT`, `BTN_MIDDLE`, …) share the same numeric space as `KEY_*` in evdev — they're already advertised.
- evdev's `KeyCode::FromStr`/`Display` (the `evdev_enum!` macro) parses `"BTN_LEFT"` the same way it parses `"KEY_A"` — so `config.toml` can already represent a mouse-button target with zero Daemon changes.
- The GUI's key field (`gui/acheron_gui/binding_editor.py`) is a bare `Gtk.Entry` — no picker, no validation. This is the actual gap for mouse-button support, not the Daemon.
- Given the above, mouse-button *output* likely already works Daemon-side and mostly needs verification + a GUI picker, not new Daemon capability — confirm this empirically rather than assuming, since untested claims from reading code have been wrong before on this project (see the `ResetFailed`/`ResetFailedUnit` correction on the last map).

**Terminology (settled during charting, see CONTEXT.md):**
- **Chord** is reserved for the new simultaneous-Input concept. Keypress's existing modifier combination (Ctrl+Shift+T) is now called exactly that — "a modifier combination" — not "a chord," to avoid collision. CONTEXT.md's Keypress entry has been updated; full Chord/Stepper glossary entries are deliberately *not* added yet — each is a bare reserved name until its own ticket below settles the actual model, per domain-modeling's "update lazily, only when resolved" discipline. (Both have since resolved — see Decisions so far — and CONTEXT.md now carries full entries for both.)

**Skills to consult**: default to `/grilling` and `/domain-modeling` for decision tickets. `/prototype` is likely warranted for the mouse-button/key GUI picker and the Stepper list-editing UX — each ticket's own grilling session should decide whether to spin one up, per the "how should it look/behave" test.

**Scope boundary volunteered directly by the user, not re-litigated in a ticket**: mouse-wheel motion is *not* an output Action — the Tartarus Pro's own wheel already passes through scroll natively, so nothing needs to synthesize scroll output. Mouse-button output is clicks only, never cursor movement.

**Standing discipline — build against the settled analog data model**: [Decide the analog data model](./issues/17-decide-analog-data-model.md) is deliberately *not* wired as a blocker on tickets 01/02/03/05/15, because none of them needs depth to decide its own shape and blocking five tickets to protect against a merge is a bad trade. It is instead the ticket to take **next** (behind only its own fact-finding task, [16](./issues/16-task-analog-mode-hardware-facts.md)). Once it resolves, any ticket touching `Binding`, the config schema, or `gui/acheron_gui/binding_editor.py` builds against the model it settled rather than today's shape. Viable only because the map has a single builder who can hold the ordering in their head — it is a discipline, not a mechanism.

**Reserved names, no glossary entry yet** (same lazy discipline as Chord and Stepper): **Analog-repeat** is the settled name for a *fourth Trigger mode* — one that modulates the frequency with which a Binding re-fires according to how deep its grid key is pressed, for games (keyboard-driven driving sims and similar) where the player would otherwise interlace keypresses by hand to steer or accelerate. Named "Simulated Analog Key-Interlacing" when the user raised it; shortened to fit the existing Fire-once/Hold-to-repeat/Toggle pattern, with the longer phrase kept as the user-facing feature name for the README. It is a Trigger mode and not an Action because it governs *how* a Binding fires, which is exactly CONTEXT.md's definition — so it composes with any Action for free. CONTEXT.md's Trigger-mode entry still says "one of Fire-once, Hold-to-repeat, or Toggle" and is corrected to four when [ticket 20](./issues/20-decide-analog-repeat-trigger-mode.md) settles the actual model.

**Settled during the analog charting pass, not to be re-litigated in a ticket**: the digital (evdev) capture path **survives** as an automatic *degradation path*, never as a user preference — the Daemon attempts driver mode and silently falls back to evdev capture if the udev rule is missing, the `hidraw` open fails, or the unlock is rejected, reporting which mode it landed in. Separately there is an explicit user-facing **override that forces digital**, for debugging GUI behavior and as a safety valve for future users; the user never selects "analog" as a normal path, they only ever switch it off. The Daemon also re-locks the device to mode `0x00` on clean shutdown. Rationale: driver mode silences the grid keys' ordinary keycodes on every node, so an analog-only design hands anyone whose udev step failed a keypad whose 20 grid keys do nothing — and a dead Daemon currently still leaves a working keypad, a property worth not losing. [Ticket 16](./issues/16-task-analog-mode-hardware-facts.md) has since bounded the damage: only the grid is silenced, so the stranded state is a crippled keypad rather than a dead one, and an unclean death does leave the user in it.

**Standing discipline — keep the door open for other Tartarus variants**: this map's destination stays scoped to the Tartarus Pro only (no other hardware to test against), but avoid casually adding new hardcoded Tartarus-Pro-only assumptions while building the remaining features where a small amount of care would keep the door open for someone else to fork/adapt for other Tartarus variants later (V1/Chroma). Mirrors the previous map's "keep the capture layer swappable" discipline for analog input — a light habit to hold, not a reason to build or audit anything now. No ticket exists for this and none is expected before v1.0.

## Decisions so far

- [Determine GNOME/Wayland-specific assumptions](./issues/10-research-de-display-server-compatibility.md) — the Daemon is DE/display-server-agnostic by construction (kernel evdev/uinput + system D-Bus only, no compositor in the path); the GUI's real tray icon doesn't exist yet (only an in-window mock), but the already-decided design (`AppIndicator3`/`AyatanaAppIndicator3`) is itself the portable choice — it needs the `ubuntu-appindicators` extension only under GNOME specifically, and works natively on KDE/XFCE. Spawned [Decide the tray icon's look and behavior](./issues/11-decide-tray-icon-look-and-behavior.md) — the library choice is settled, but its look/feel and menu behavior are not, so it's a design ticket first, not a direct build.
- [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md) — settled against the Synapse catalog. Required additions: reusable/named Macro entities (new ticket). Non-blocking additions: grid-key analog capture, Controller/Joystick emulation (both new tickets, open-ended). Corrections folded into existing tickets: Chord gets a thumbstick-diagonal worked example (ticket 01); the key/mouse-button picker widens to multimedia keys + a canned-text note (ticket 02). Ruled out: Macro shell commands, Launch-a-program, an intrinsic Macro loop primitive.
- [Research the Linux analog grid-key protocol](./issues/12-research-linux-analog-grid-key-protocol.md) — implementation-ready; see [the write-up](./research/linux-analog-grid-key-protocol.md). Exact 91-byte unlock buffer (`transaction_id 0x01`, class `0x00`, cmd `0x04`, arg `0x03`, CRC `0x05`) + `HIDIOCSFEATURE(91) = 0xC05B4806` on the Interface-2 `hidraw` node, then `read()` on Interface 1 filtering report `0x06`. Our own hardware's descriptors independently corroborate the protocol (report `0x06`, 23 bytes, endpoint `0x82`/24-byte packets), and OpenRazer's Interface-2 control channel already round-trips real data from Linux on this unit — so only the set-device-mode command itself is unproven. Reset risk means a USB re-enumeration (firmware self-reboot), not a hang or brick — and the reported reconnect *loops* can't form on our stack, since both loop-forming senders now skip this device; it also traces to a single contributor's report in OpenRazer PR #2710, uncorroborated anywhere else in the device's paper trail, and is plausibly a wrong-`transaction_id` artefact. Two findings that outrank the plan: analog **has** been made to work on Linux before (OpenRazer PR #1868, Huntsman Mini Analog), and driver mode probably **silences the grid keys' ordinary keycodes** — if so, analog is a device-wide mode switch under Acheron's entire evdev capture path, not an additive stream. Also spends the map's "no privileged install step" property (`/dev/hidraw*` needs a udev rule). Unblocks [the prototype](./issues/13-task-standalone-analog-capture-prototype.md).

- [Standalone analog-capture prototype](./issues/13-task-standalone-analog-capture-prototype.md) — **it works, first attempt, on the real unit**; every byte of ticket 12's plan held with nothing re-derived or worked around. The unlock (`transaction_id 0x01`) produced the standby report 3 ms later; 6700 reports followed, all 20 keycaps spanning `0x00`–`0xFF`, **256 distinct depth values**, smooth monotonic press ramps — real analog, not on/off. No layout deviation at all (every report 24 bytes, depths at bytes 1–20, trailing bytes zero throughout). **The reset was not reproduced** across four `set_device_mode` sends in two sessions, by mode `0x03` *or* by the mode `0x00` re-lock — the device never left the bus, supporting §5's `transaction_id` hypothesis. Two findings beyond the questions asked: the stream is **event-driven, not polled** (silent between presses, ~1 ms per change while moving — much cheaper for an always-on Daemon than expected), and **§6's expensive branch is the real one, directly observed** — with the Daemon stopped so the evdev nodes were ungrabbed, driver mode gave 2911 analog reports and **0** evdev events, while the re-lock immediately restored 104 evdev events and 20 grid keycodes. The ordinary keyboard report `0x01` stops outright and the grid keys go dark on evdev. Analog is therefore a device-wide mode switch underneath Acheron's whole capture path, not an additive stream: while it's on, the Daemon must synthesise all 20 discrete keys from depth thresholds and every existing feature has to keep working on top of that. Feasibility is settled and the "may be dropped if infeasible" caveat is discharged — but the integration cost went **up**, which strengthens the case for analog fast-following v1.0 rather than blocking it. Prototype: `prototype/13-analog-grid-capture/`; raw capture in `assets/13-unlocked.jsonl`.

- [Collect the remaining driver-mode hardware facts](./issues/16-task-analog-mode-hardware-facts.md) — **the strand survives its own invalidation test: driver mode silences the 20 grid keys and nothing else.** The Mode key, the thumbstick's four directions and the wheel's three events all keep emitting evdev normally while the analog stream runs, so Layers and the thumbstick are never at risk and ticket 18's *hybrid* source (`hidraw` for the grid, evdev for the other 8) is confirmed rather than assumed. Four more facts, all on the real unit with the Daemon stopped: **a power cycle restores digital mode** (a genuine re-enumeration — HID IDs and evdev nodes both moved), but **driver mode survives suspend/resume with the `hidraw` fd still open and the stream simply resuming** — so the recovery trigger is USB re-enumeration alone, and the re-unlock belongs on the existing `connection_tx` reconnect path. **An unclean death strands the user with 20 dead grid keys** and everything else working, and **a re-lock from a fresh process that never sent the unlock does work**, which is what makes a Daemon restart recoverable. **`byte n = keycap n` is now confirmed per-key, pressed in a deliberate non-reading order** — ticket 13's inference rested on layout-order presses and no longer does. Two findings beyond the questions asked: **the device's own evdev autorepeat still fires in driver mode**, so ticket 18's Hold-to-repeat regression applies to the 20 grid keys only rather than to every Input; and the standby `0x06` report marks a mode *transition*, not every `set_device_mode`. Still no reset — nine clean sends now across both tickets. Raw capture in `assets/16-driver-mode-facts.jsonl`, now including evdev events. Unblocks [the analog data model](./issues/17-decide-analog-data-model.md).

- [Decide the analog data model](./issues/17-decide-analog-data-model.md) — `PhysicalEvent` widens with an optional `depth: Option<u8>` field (not a new variant or parallel channel); the model carries both an actuation point and a release point (hysteresis); `Input` stays unified, with depth-setting validated rather than typed away into a `GridKey`. Actuation points are scoped per-Input per-Profile (shared across Base/Held, avoiding the Binding-scoped asymmetry) with a Profile-level default (placeholder 128/112) individual keys can override, plus a GUI "reset all to default" affordance backed by a dedicated `ResetActuationPoints()` call. Device mode gets a `Config.force_digital` preference (`SetForceDigital`, mirrors `SetOutputSuppressed`) plus a live-reported `capture_mode` in `GetState()`/a new `CaptureModeChanged` signal, since ticket 16 proved the actual mode can change under a running Daemon. Five new D-Bus methods total, all active-Profile-scoped and atomically persisted like `SetBinding`. Purely additive to `config.toml` — `#[serde(default)]` throughout, no `SCHEMA_VERSION` bump. CONTEXT.md gained Depth/Actuation point/Release point/Capture mode glossary entries. Unblocks [ticket 18](./issues/18-rework-capture-path-for-analog.md), [ticket 19](./issues/19-prototype-trigger-point-ux-and-live-depth.md), and [ticket 20](./issues/20-decide-analog-repeat-trigger-mode.md).

- [Rework the capture path for analog](./issues/18-rework-capture-path-for-analog.md) — settled
  across ten decisions via a full grilling session against the real code and device (nothing
  built yet): the analog `CaptureSource` generalizes `evdev_source`'s node loop to a subset
  (Main+If02 unchanged) plus one new hidraw-based grid task in the same `JoinSet`; `hidraw`
  discovery walks `/sys/class/hidraw` per-(re)open with absence treated like evdev's existing
  non-fatal retry bucket; unlock gets a 500ms report-`0x06` timeout and only resends on a fresh
  reopen (reset-risk-aware backoff); Hold-to-repeat timing is read live from the real device's
  kernel autorepeat via `get_auto_repeat()` rather than hardcoded; per-key actuation-point
  thresholding happens in the capture layer via a `watch`-channel snapshot dispatch publishes,
  keeping `Config` single-owner; `force_digital`/`capture_mode` (ticket 17) get a genuine live
  source-swap in `main.rs`, retried only at startup/reconnect/explicit toggle, never on a
  background timer; fatality taxonomy is unchanged (dispatch/injector exit fatal, capture
  absence/swap is not); the udev rule targets `plugdev`/`MODE="0660"`, installed best-effort by
  `install.sh`; threshold/repeat logic is pure and unit-tested separately from the I/O loop. No
  second prototype ticket — everything left was architecture, not a look/feel question. The
  build itself was too large for one session and split into three sequential task tickets:
  [Apply the analog data model to code](./issues/21-task-apply-analog-data-model-to-code.md)
  (ticket 17's shapes were never actually written into `config.rs`/`command.rs`/`dbus`, AFK, no
  hardware needed), [Build the analog CaptureSource](./issues/22-task-build-analog-capture-source.md)
  (the hidraw grid task itself, HITL), and
  [Wire live source-swap, udev rule, and install.sh](./issues/23-task-wire-analog-supervisor-and-install.md)
  (`main.rs` integration and end-to-end verification, HITL). Tickets 19 and 20's `Blocked by`
  was corrected from `17` to `17, 23` — both need real depth events flowing through a running
  Daemon, not just the settled model.

- [Apply the analog data model to code](./issues/21-task-apply-analog-data-model-to-code.md) —
  landed ticket 17's decided shapes mechanically into `config.rs`/`command.rs`/`dispatch.rs`/
  `dbus` (`ActuationPoint`, `Config.force_digital`, `PhysicalEvent.depth`, five new D-Bus
  methods), no hardware needed. Code review caught and fixed a cross-module regression:
  `GetState()`'s new 5-tuple arity broke the GUI's positional unpacking; threaded `capture_mode`
  through `daemon_client.py`/`daemon_stub.py`/`app.py` to fix it. Unblocks ticket 22. *(This
  bullet was missed at the time ticket 21 resolved — added retroactively, caught by ticket 22's
  own code review.)*
- [Build the analog CaptureSource](./issues/22-task-build-analog-capture-source.md) — the
  `hidraw` grid task landed: `/sys/class/hidraw` discovery, an unlock/confirm lifecycle sharing
  `evdev_source`'s existing absence-retry bucket, a pure `observe()` hysteresis function and
  `RepeatSchedule` for synthesized Hold-to-repeat (both unit-tested exhaustively, no hardware),
  and `dispatch.rs` publishing a resolved per-key Actuation-point snapshot into a `watch`
  channel on every mutation that touches it. `evdev_source`'s node loop generalized to an
  explicit node subset so the grid task's Main+If02 evdev nodes share one `JoinSet`/presence
  view with the grid task itself, per ticket 18 §1. Code review caught and fixed two real bugs:
  `reject_release_above_actuation` accepted `release == actuation` (chatters Down/Up forever on
  a motionless key once `observe()` actually consumes it), and `poll_readable` only checked
  `POLLIN`, so a closed/hung-up fd spun in a tight busy-loop instead of ever detecting EOF —
  caught by this ticket's own new force-release regression test. HITL verification (unlock
  against the real device, 20-key threshold accuracy, repeat cadence) deferred to the user, who
  chose to run it themselves; `daemon/examples/analog_probe.rs` is the tool for that. Unblocks
  ticket 23.
- [Verify the analog CaptureSource on hardware](./issues/24-task-verify-analog-capture-source-on-hardware.md)
  — the joint HITL session ticket 22 skipped, done live. All five checklist items confirmed
  against the real Tartarus Pro: unlock/report `0x06`, all 20 grid keys thresholding correctly
  at 128/112, Hold-to-repeat cadence matching the live kernel `get_auto_repeat()` value,
  permission-denied degrading silently, and Mode key/thumbstick/wheel passing through evdev
  unaffected. No `capture/analog.rs` changes needed. Two tooling notes for
  [ticket 23](./issues/23-task-wire-analog-supervisor-and-install.md): the device needs
  re-locking between `analog_probe` runs (a second unlock while already in Analog Capture mode
  produces no fresh report `0x06`, per ticket 16), and `analog_probe`/`AnalogCaptureSource`'s
  capture tasks have no shutdown signal and block process exit forever once started (shared
  pre-existing behavior with `EvdevCaptureSource`, harmless under a real process-signal
  shutdown but worth knowing if ticket 23's shutdown path ever closes channels instead).
  Unblocks ticket 23 (now also clear of ticket 22).
- [Wire live source-swap, udev rule, and install.sh](./issues/23-task-wire-analog-supervisor-and-install.md)
  — the analog fast-follow strand's build is now feature-complete and live-verified end to end.
  New `capture::supervisor` owns which `CaptureSource` runs: startup attempts Analog with a
  6-second grace fallback to Digital, `SetForceDigital`/a genuine reconnect swap it live, none of
  it a background timer. Found and fixed two real bugs only reachable by actually running it: a
  `tokio::main`-shutdown hang on SIGTERM/SIGINT (fixed with `std::process::exit`, not a graceful
  return) and an infinite Digital/Analog thrash loop from a fresh Digital attempt's own
  multi-node presence convergence looking like a reconnect (fixed with an `ever_connected` gate).
  Also closed a design-time-only EVIOCGRAB hazard in ticket 18 §6's "stop one JoinSet and start
  the other" by adding real cooperative shutdown (non-blocking + poll, a shared flag,
  shutdown-aware draining) to both `evdev_source` and `analog`, confirmed live via zero leaked
  fds across repeated swaps. udev rule installed at `packaging/60-acheron-tartarus-pro.rules`,
  wired into `install.sh`'s new privileged step (not yet run for real on this machine — needs the
  user's own `sudo`). Full live checklist passed: analog/digital both drive all 20 grid keys plus
  thumbstick/wheel/Mode-key, `kill -9` recovery, a real power-cycle's permission reversion and
  self-heal, and a clean SIGTERM relock, all against the physical device. One accepted residual
  gap: an already-running Analog session that loses `hidraw` access without the process
  restarting doesn't self-fall-back (only a fresh attempt's grace period does) — moot once the
  udev rule is actually installed, since a replug then reapplies it automatically. Unblocks
  [ticket 19](./issues/19-prototype-trigger-point-ux-and-live-depth.md) and
  [ticket 20](./issues/20-decide-analog-repeat-trigger-mode.md) (both now also clear of 17).

- [Decide the D-Bus GetState() wire shape](./issues/25-decide-dbus-state-wire-shape.md)
  — `GetState()` moves off its positional 5-tuple onto a keyed dict, matching
  `GetConfig()`'s existing convention, since only a keyed shape structurally
  prevents the class of bug that broke `app.py`'s `rebuild()` in ticket 21 (a
  new field silently changing the tuple's arity). Built in this session, not
  deferred: `wire::state_to_dict()` mirrors `config_to_dict`'s pattern
  server-side; `daemon_client.py`/`daemon_stub.py`/`app.py` and 26 Rust/
  Python test call sites updated to match. All 171 Rust + 70 Python tests
  pass.

- [Design trigger-point UX and live-depth channel](./issues/19-prototype-trigger-point-ux-and-live-depth.md)
  — prototyped three variants (inline/one-marker/raw, inline/two-marker/percent+badge,
  separate all-20-key overview) in a throwaway GTK4 app, captured on
  `prototype/19-trigger-point-depth-ux` (not `main`). **Variant B won**, refined over two
  rounds of live reaction: inline in `binding_editor.py` below the existing controls, two
  independently draggable markers (green Actuation/amber Release, legend text colored to
  match), percentage units, a bar that spans the editor's full width and stays correct
  across a resize (two live-screenshot bugs caught and fixed in session), a badge that
  doubles as a *live* analog/digital capture-mode indicator (green/warm-red) rather than a
  static label, and a digital-mode fallback that greys the bar and overlays its warning
  centered on top rather than as a separate line below. Sketches the D-Bus shape via the
  prototype's `SimDepth`: a connection-scoped `StartDepthStream`/`StopDepthStream` pair
  plus a `DepthChanged` signal at ~30Hz, independent of `StopAllToggles`/output-suppression,
  live only while the editor is open. `GetConfig()` still doesn't serialize
  `default_actuation`/`actuation_overrides`. None of this is wired into the real GUI/Daemon
  yet — spawned [Build the trigger-point UX and live-depth channel for real](./issues/26-task-build-trigger-point-depth-ux.md).

- [Build the trigger-point UX and live-depth channel for real](./issues/26-task-build-trigger-point-depth-ux.md)
  — landed all four scope items. Daemon: `StartDepthStream`/`StopDepthStream`/`DepthChanged`
  modeled as a single current stream target (epoch/disconnect-watcher, mirroring
  `SetOutputSuppressed`'s shape, Config-free and bypassing dispatch); `AnalogCaptureSource`
  gained a `depth_tx` publishing all 20 keys' depth on every report, threaded through
  supervisor/main.rs opposite `actuation_tx`. `GetConfig()` now serializes
  `default_actuation`/`actuation_overrides` (ticket 21's deferred gap, closed). The real
  Actuation & release section in `binding_editor.py` ports the prototype's `DepthTrack`
  widget almost unchanged, gated to Grid Inputs only, wired to all five existing Set/Clear/
  Reset D-Bus methods plus the new depth pair. Found and fixed the same "eager rebuild"
  hazard twice, two different ways: depth streaming (~30Hz, must track one open popover)
  uses a client-side single-current-target routing seam started/stopped from the section's
  own `map`/`unmap`; capture-mode (rare, badge-only) is threaded as a plain parameter fed by
  a new app-level `CaptureModeChanged` subscription driving the existing full-`rebuild()`
  pattern instead — either approach done per-widget would have leaked one signal connection
  per rebuild. 176 Rust + 79 Python tests green; a `tokio::time::interval` first-tick-
  immediately race (letting a superseded `StartDepthStream` sneak one stray signal out) was
  caught by its own new test and fixed with `interval_at`. **Live-hardware verification not
  done this session** — a real Daemon and Tartarus Pro were available, but swapping the
  user's live input-device driver out from under them unasked was judged out of this
  session's call. Spawned [Verify the trigger-point UX and live-depth channel on hardware]
  (./issues/27-task-verify-trigger-point-depth-ux-on-hardware.md).

- [Verify the trigger-point UX and live-depth channel on hardware](./issues/27-task-verify-trigger-point-depth-ux-on-hardware.md)
  — all four of ticket 26's checklist items confirmed live against the real Tartarus Pro,
  after the user's own testing surfaced and this session fixed two real bugs. `GetConfig()`
  never serialized `Config.force_digital`, so the "Force digital capture" checkbox always
  constructed unchecked regardless of the Daemon's real persisted value — explained the
  exact reported asymmetry (checking closed the dialog via a real transition; reopening
  showed unchecked; checking again was a no-op since already-Digital, so no signal, no
  close). Fixed by serializing it and seeding the checkbox from it
  (`daemon/src/dbus/wire.rs`, `gui/acheron_gui/binding_editor.py`). Separately, "Set as
  Profile default"/"Reset all keys to Profile default" only took effect after a full GUI
  restart: every Grid key's popover is pre-built once from a single `GetConfig()` snapshot,
  and unlike `capture_mode` there's no Daemon signal for a `default_actuation`/override
  change, so nothing told the app its cache was stale. Fixed by threading the existing
  `on_saved` (popdown + app `rebuild()`) callback into `build_actuation_section` and calling
  it from those two handlers only — `set_actuation_point`/`clear_actuation_point` are
  deliberately left non-closing since they only affect the current key and must survive
  every drag-end. The Force-digital checkbox is confirmed a persistent override (stays
  Digital across an unplug/replug until explicitly unchecked, by design); the live badge
  check was done via the automatic unplug/replug Analog↔Digital fallback path instead of
  the checkbox, confirming the badge reads correctly on reopen. 177 Rust + 83 Python tests
  green. Ticket 26's build is now fully live-hardware-verified.

- [Fix the acheron-daemon udev startup race](./issues/28-task-fix-acheron-daemon-udev-startup-race.md)
  — the ticket's own working hypothesis (a race against `60-acheron-tartarus-pro.rules`
  landing on the Tartarus's `hidraw` nodes) was wrong: that path is already handled softly by
  the capture layer. Checking this system's real udev config instead (`getfacl /dev/uinput`,
  `/usr/lib/udev/rules.d/60-steam-input.rules`) found the actual cause is `/dev/uinput`'s own
  `uaccess`-tag ACL, granted when the login session activates — unordered against
  `graphical-session.target` — racing `main()`'s first, previously-unguarded
  `injector::build_device()` call. None of the ticket's three systemd/udev-ordering candidates
  would have reliably fixed this, so no packaging/unit/udev-rule changes were made. Fixed in
  code instead: `injector::retry_on_permission_denied`, generic and unit-tested, bounded at 5s
  (25×200ms), wired into `main()`'s device build — mirrors the capture layer's existing
  absence-retry precedent, just applied to the one startup call site that didn't have it yet.
  180 Rust tests green. Not yet verified live (needs a real cold reboot, can't substitute a
  live `systemctl --user restart`) — spawned
  [Verify the udev-startup-race fix on a real cold reboot](./issues/29-task-verify-udev-startup-race-fix-on-hardware.md).

- [Verify the udev-startup-race fix on a real cold reboot](./issues/29-task-verify-udev-startup-race-fix-on-hardware.md)
  — live-verified across two cold boots, after catching and correcting a false-negative first
  attempt: that boot's installed binary was stale (built before ticket 28's actual source edits,
  confirmed missing the `retry_on_permission_denied` symbol entirely via `nm`/`strings`), so it
  reproduced the pre-fix crash shape and wasn't a real test. After rebuilding and reinstalling
  the genuine fix, both following cold boots showed the race still being hit (the retry
  diagnostic fires once each time) but self-healing within the first attempt — one `Started`
  line, no `PermissionDenied` crash, no restart — matching ticket 28's own two-boot
  reproducibility bar. Ticket 28's fix is now fully live-hardware-verified.

- [Design Chord Bindings](./issues/01-decide-chord-bindings.md) — open-ended member count, Base/Held-scoped like an ordinary Binding (`Profile.chords_base`/`chords_held: HashMap<BTreeSet<Input>, Binding>`), reusing `Binding`/`Action` unchanged (not a new Action variant). Simultaneity via a hardcoded ~50ms window (dispatch.rs constant, not a user setting); on completion the Chord suppresses member Bindings entirely, on timeout the pending member's individual Binding fires retroactively (delayed). Releasing any member ends the Chord's held/toggle state as a whole. Trigger-modes apply unchanged. Overlapping Chord definitions are designed away — rejected at save time rather than arbitrated at runtime. GUI recording confirmed as live physical-press capture of membership only; exact interaction deferred to spawned [Prototype the Chord recording UX](./issues/30-prototype-chord-recording-ux.md), which also carries a debug-only slider for tuning the window constant. Thumbstick diagonals fall out for free as ordinary 2-member Chords. CONTEXT.md gained the Chord glossary entry.

- [Finalize mouse-button output and design the picker](./issues/02-decide-mouse-button-output-and-picker.md) — live-verified against the real Daemon + Tartarus Pro: mouse-button output (`BTN_LEFT` real left-click), the full non-alphanumeric keyboard range (lock/function/nav/misc/multimedia keys), and Ctrl+Click all work end-to-end with zero Daemon changes. The flagged double-`KeyDown`/`KeyUp` edge case (a modifier used as both main key and active modifier) is harmless — confirmed via raw evdev capture that the kernel's `EV_KEY` state-dedup collapses it to one clean down/up. Picker exposes mouse Left/Right/Middle/**Back**/**Forward** (`BTN_SIDE`/`BTN_EXTRA` relabeled by function) and the full keyboard with no exclusions; spawned [Prototype the key/mouse-button picker UX](./issues/32-prototype-key-mouse-button-picker-ux.md) to explore a graphical keyboard layout vs. a category-sorted menu. Canned-text macros stay manual for v1.0. **New finding escalated to its own ticket**: a bare modifier as a Fire-once/Hold-to-repeat main key is a near-useless instant pulse (every Trigger mode except Toggle only ever fires canned pulses, never a sustained hold) — and more seriously, an *unbalanced* Macro (`KeyDown` with no matching `KeyUp`) under Fire-once/Hold-to-repeat can strand a key held down forever, reproduced live requiring a full reboot to clear, since neither Trigger mode reacts to the physical Input's `Up` the way Toggle's force-release does. Spawned [Fix the Fire-once/Hold-to-repeat stuck-key gap](./issues/33-fix-fire-once-hold-to-repeat-stuck-key.md); documented as an immediate README footgun in the meantime. CONTEXT.md unchanged — no new glossary terms, all existing Action/Modifiers/Trigger-mode entries already covered this ground.

- [Design the Stepper list-stepping construct](./issues/03-decide-stepper-list-stepping.md) — opened by testing the user's own reframing ("isn't this just a wait-for-keypress step inside a Macro?"), which broke on two axes: a Binding is one Input→one Action, but Stepper needs two Bindings sharing one persistent cursor, and Macro has no state outliving a single firing. Landed as a new `Action::Step { stepper: StepperId, direction: Forward | Backward }` variant referencing a named list in one **global** library (mirroring the reusable-Macro-entities direction) — defined once, reassignable to a different forward/backward Input pair at any time (only one pair may reference a list at once; reassigning silently moves it, no reject-at-save step). List items are a dedicated type distinct from `Action`, restricted to a single fire-once keyboard key or mouse-button, structurally excluding Macro/Stepper items, designed to extend to controller/joystick buttons later. Stepping wraps at either end. Cursor position is per-list runtime state, independent of Profile/Layer, never persisted — resets to the list's start on every Daemon restart (mirrors `capture_mode`'s live-`GetState()` precedent over a `Config` field). Trigger mode applies to the step itself (Fire-once/Hold-to-repeat); Toggle is disallowed — the first exception to Trigger mode's "applies to every Binding" rule. GUI spawned [Prototype the Stepper library and list-editing UX](./issues/31-prototype-stepper-library-ux.md), deliberately blocked on [Design reusable Macro entities](./issues/15-decide-reusable-macro-entities.md) since both are the same "library entity reassignable to a Binding" shape and may share UX or collapse into one prototype; ticket 15 gained a forward-pointing note. Composition fog partly graduated for free: a Chord's Action can be `Action::Step` and vice versa, since Chord already fires any Action unchanged — no new ticket needed; the Profile-Switch↔Stepper half stays fog pending ticket 05. CONTEXT.md gained the Stepper glossary entry and a Toggle-exception note on Trigger mode.

- [Decide key-textfield context-menu items](./issues/04-decide-key-textfield-context-menu-items.md) — moot, superseded by [ticket 32](./issues/32-prototype-key-mouse-button-picker-ux.md): ticket 02's resolution already commits to replacing `key_entry`'s free-text `Gtk.Entry` with a full-coverage picker (no fallback text entry for uncovered cases), so once that lands the widget is no longer an editable `Gtk.Text`, and GTK's stock "Insert Emoji"/"Change Direction" context menu no longer applies. Closed without live GTK verification (confirmed with the user rather than testing a widget slated for replacement). The emoji-as-Action idea this ticket surfaced is unaddressed by the picker and unresolved — a future idea, not scoped here.

- [Design Profile Switch](./issues/05-decide-profile-switch-action.md) — `Action::ProfileSwitch { target: String }`, dispatch-special-cased rather than riding the shared `MacroStep` pipeline (it emits no keys): `handle_event` intercepts it before `fire()`/`executor::compile`, calling a function extracted from `Command::SwitchProfile`'s existing handler — so the force-stop-every-Toggle path, actuation-snapshot republish, and signal ordering all apply for free, no special-casing needed. Trigger mode is validation-locked to Fire-once (mirrors Stepper's Toggle-disallowed precedent). `RenameProfile` cascade-updates every cross-Profile `ProfileSwitch` reference; `DeleteProfile` refuses if still referenced — dangling references are structurally impossible, not just tolerated. Self-reference is allowed as a no-op that still force-stops Toggles (an incidental "stop all toggles" hotkey). Unrestricted across Base/Held. Resolved the "Composition between Stepper and Profile-Switch" fog for free: a Stepper item's type already excludes Profile-Switch, a Stepper Binding's Action can't be anything but `Action::Step`, and Stepper cursor state is Profile-independent by ticket 03's own design — no interaction to build. Spawned [Build Profile Switch](./issues/34-task-build-profile-switch.md).

- [GUI polish: grid-button sizing, default-binding labels, Mode-key width](./issues/06-gui-polish-grid-sizing-default-labels-mode-key-width.md) — landed, but live screenshots overturned the ticket's own hypotheses for two of the three items, and a later user reaction to the built GUI overturned a third. Grid height: the real defect was plain `wrap=True` unable to break unbroken modifier-chord runs at all (forcing width, not height, to blow out — a real Gtk-WARNING reproduced live); fixed with `Pango.WrapMode.WORD_CHAR` deliberately *without* a `max-width-chars` cap, since a first attempt at that mid-word-split the ordinary word "passthrough" on every unbound key (caught by an actual screenshot, not just code-reading). Default `h` raised 52→99, measured live to comfortably cover every case tried including a deliberately pathological 4-modifier+long-key-name Binding. Default labels: `action_summary()` now takes `inp` and shows the stock output via a new `INPUT_DEFAULT_LABEL` table in `inputs.py` (hardcoded, mirrors `daemon/src/input.rs`'s `GRID_KEYS`, now covering all 28 `ALL_INPUTS` entries) — **but bare** (`"Q"`, `"Alt"`), not wrapped in a `"passthrough (…)"` qualifier as first built: the user, after seeing the real GUI, pointed out the word is jargon, too long for the 52px buttons, and actively wrong once a running Daemon is in analog mode (the grid's 20 keys are then synthesized from depth thresholds, not literal evdev passthrough), so this session dropped it entirely rather than trying to make the label capture-mode-aware. Mode-key width: not a wrapping problem — `Gtk.MenuButton`'s default `halign=FILL` was stretching it to fill its parent `Box`'s full cross-width (the diamond's ~160px); fixed with `set_halign(Gtk.Align.CENTER)` in the shared `make_input_button`, confirmed live at 0px offset from the diamond's top lobe. The `.mode-key` CSS's pill-vs-circle shape issue (flagged, not fixed, per the ticket's own scope) still stands, as does `binding_editor.py`'s separate "Clear (passthrough)" button (a different, non-space-constrained surface the user didn't flag). 83 Python tests green, no Rust changes.

- [Choose an open source license](./issues/09-decide-open-source-license.md) — **GPLv3-or-later**, the user's copyleft gut call over permissive (MIT/Apache-2.0): derivative works of Acheron must stay open source. `LICENSE` added at the repo root with the canonical text fetched from gnu.org. Checked the real dependency tree: the Daemon's Rust crates are all MIT/Apache-2.0, the GUI's PyGObject/GTK4 is LGPL-2.1+ — both fine to depend on from a GPL app, nothing incompatible found. The "or later version" clause belongs in each source file's copyright header (standard GPL convention), deferred to the release-documentation pass rather than done here. Graduated the "release documentation" fog now both its blockers are settled — spawned [Write the release documentation](./issues/35-task-write-release-documentation.md).

- [Design Controller/Joystick output emulation](./issues/14-decide-controller-joystick-output-emulation.md) — first pass covers buttons only, every Input eligible (20 grid keys, Mode key, thumbstick's 4 directions, wheel's 2 directions); axes deferred to future fog, with guidance left for it (target real depth directly via the already-live `depth_tx`, not a throwaway digital-only model). New `Action::ControllerButton { button: KeyCode }`, an ordinary Action variant reusing Binding/Trigger-mode/dispatch unchanged (no special-casing like ProfileSwitch needed) — composes for free with Chord and Macro steps. A second, distinct `uinput` device advertises the full standard Linux Gamepad Spec capability set (named buttons + `BTN_TRIGGER_HAPPY1`–`40` + dpad), no hardcoded physical→button correspondence, `button` field reuses bare `KeyCode` with allowlist validation (mirrors ticket 02's mouse-button precedent). Spawned and resolved [research the legacy joystick API](./issues/37-research-legacy-joystick-api-compatibility.md) mid-session: `/dev/input/jsX` compatibility is a free kernel side effect of building a standard `evdev` gamepad device — zero work needed, question fully closed. CONTEXT.md gained the Controller glossary entry; also fixed the Action entry's stale "either a Keypress or a Macro" enumeration (missed when Stepper/Profile Switch were decided) to list all five current Action kinds. Spawned [Prototype the controller-button picker UX](./issues/38-prototype-controller-button-picker-ux.md).

- [Design reusable Macro entities](./issues/15-decide-reusable-macro-entities.md) — every Macro becomes a named library entry: `Config.macros: HashMap<MacroId, MacroDef>` (global, one library, `MacroDef { name, steps }`), `Action::Macro { steps }` replaced by `Action::Macro { macro_id }` with no inline form surviving. `MacroId` is a slug frozen at creation (not a random UUID — no direct `uuid` dependency in the daemon, stays legible in a hand-edited `config.toml`), decoupled from the editable `name`, so renaming needs no cascade. Unlike Stepper, reuse is many-to-one (any number of Bindings across any Profile may share one Macro, no exclusive-owner reassignment) since a Macro has no runtime cursor to contend over; deletion instead refuses while still referenced, mirroring `DeleteProfile`. Chord inherits the new shape for free (`Binding` reused unchanged); Profile Switch and Stepper are unaffected. No GUI decide-ticket spawned — folds into [Prototype the Stepper library and list-editing UX](./issues/31-prototype-stepper-library-ux.md), now unblocked and updated to cover both library pickers in one session. CONTEXT.md's Macro entry updated.

- [Decide the tray icon's look and behavior](./issues/11-decide-tray-icon-look-and-behavior.md) — minimize-to-tray (window-close hides, only the tray menu's Quit exits the GUI; the Daemon is unaffected, already an independent `systemd --user` service). Checked directly against GNOME's `ubuntu-appindicators` extension source: `AppIndicator3` has no click-to-activate at all — every click just opens the menu, so there's no left/right-click behavior to design. Menu: status line → Show Window → Switch Profile ▸ (submenu) → Pause/Resume Daemon (session-only `systemctl --user stop`/`start`, unit stays login-enabled) → Quit. Icon changes per the same 3-way `STATUS_STATES` used by the header badge, as placeholder filled-circle SVGs at the existing hex colors, bundled in-repo via `set_icon_theme_path` (no light/dark variants needed — non-symbolic). Tooltip mirrors `STATUS_STATES`' label text. No `/prototype` — a tray icon's design surface is OS-chrome-fixed, decided directly. `gir1.2-ayatanaappindicator3-0.1` (a standard Ubuntu `main`-repo package) is now installed on the dev machine. Spawned [Build and verify the real tray icon](./issues/36-task-build-tray-icon.md).

## Not yet specified

- **The Macro library's build ticket** — spawns once [Prototype the Stepper library and list-editing UX](./issues/31-prototype-stepper-library-ux.md) resolves (it now covers both Stepper's and Macro's picker UX in one session; ticket 15 deliberately left the build ticket unspawned until that GUI shape is settled). Chord, mouse-button/keyboard output, Profile Switch, and Controller-button output have all fully graduated: their decide tickets resolved and spawned build/prototype tickets ([Prototype the Chord recording UX](./issues/30-prototype-chord-recording-ux.md); [Prototype the key/mouse-button picker UX](./issues/32-prototype-key-mouse-button-picker-ux.md); [Build Profile Switch](./issues/34-task-build-profile-switch.md); [Prototype the controller-button picker UX](./issues/38-prototype-controller-button-picker-ux.md)).
- **The Fire-once/Hold-to-repeat stuck-key fix's implementation shape** — [ticket 33](./issues/33-fix-fire-once-hold-to-repeat-stuck-key.md) names the likely fix (held-key tracking + force-release-on-Up, mirroring Toggle) but the exact mechanism and whether the GUI also gets a save-time warning are still open within that ticket.
- **Analog's composition with the rest of the feature set** — residual fog left by the analog charting pass, past tickets 17–20. Does a Chord made of grid keys use each key's own actuation point or a shared one; can a Stepper step be driven by depth rather than a discrete press; does a Macro step ever want to *read* depth. Deliberately not ticketed: it is the same shape as the Chord/Stepper composition fog above and graduates on the same schedule, once both sides have settled their own models.
- **Controller/Joystick axis output** — the remaining fog past [Design Controller/Joystick output emulation](./issues/14-decide-controller-joystick-output-emulation.md), now that its buttons-only pass, device advertising, and `jsX` question are all settled. Not yet sharp enough to ticket: which axes (thumbstick as an analog stick? grid-key depth as a trigger axis?), deadzone/curve model, `ABS_*` code choices. Guidance already banked for whoever tickets it: target real depth directly (`depth_tx` is already live) rather than a digital-only placeholder, with a Digital-capture-mode fallback via press/release step-increment.
- **What ships in the public repo** — `.scratch/`, `prototype/`, `docs/adr/`, and `CONTEXT.md` are all currently assumed to ship as-is (they're a legible build record, not sensitive), but the user flagged a concern that this much process detail might overwhelm a user who just wants to game. Deliberately deferred rather than decided now; revisit closer to release.

## Out of scope

- Mouse cursor movement (pointer X/Y motion) as an output Action — mouse-button support is clicks only.
- A synthetic mouse-scroll-wheel output Action — the Tartarus Pro's own wheel already passes through scroll natively (see Notes).
- Capturing input from a real external mouse device — Acheron's capture surface remains the Tartarus Pro's three evdev nodes only; "mouse buttons" here is output-side exclusively.
- Distro packaging (AUR, `.deb`, Flatpak, etc.) for v1.0 — release means a clean git repo built from source via `install.sh`; packaging is a plausible fast-follow if the tool ever needs deeper system integration, not a v1.0 requirement.
- Distribution/promotion after release (posting to r/linux_gaming, submitting to package repos, etc.) — this map is about the tool being releasable, not about promoting it afterward.
- Generalizing hardware support to other Tartarus variants or other devices — Tartarus Pro is the only hardware owned and tested; see Notes for the lighter "keep the door open" discipline this map does carry.
- A capstone "cut v1.0" ticket — reaching v1.0 is this map's frontier going empty, not a decision to ticket.
- Shell-command execution as a Macro action (surfaced by the Synapse catalog) — security-sensitive, no articulated use case; see [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md).
- A Launch-a-program Action (surfaced by the Synapse catalog) — judged not worth having; see [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md).
- An intrinsic Macro loop/repeat-forever primitive distinct from Toggle (Synapse's "loop forever after an optional setup sequence") — a confirmed but narrow gap; Toggle already covers looping for the general case. See [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md).
