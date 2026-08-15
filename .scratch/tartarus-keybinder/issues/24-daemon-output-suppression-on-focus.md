# 24 — Daemon: output-suppression flag + D-Bus surface + disconnect safety

**What to build:** The Daemon gains a way for a connected D-Bus client to suppress all of its synthetic `uinput` output — Fire-once, Hold-to-repeat, and Toggle firings alike — for as long as that client says so, without altering any Trigger-mode firing logic, Macro looping, or Toggle's `active_toggles` state internally. Only the actual write to the virtual input device is withheld while suppression is on; everything resumes exactly where it logically was the instant suppression is cleared. If the client that turned suppression on disappears (crash, `kill -9`, any ungraceful disconnect) without explicitly clearing it, the Daemon must notice and auto-clear it itself — suppression must never be able to get stuck on and silently mute the entire physical device.

This ticket delivers a complete, independently testable Daemon-side capability against the existing `CaptureSource` fake-event seam; it does not require a real GUI to be driving it yet (that's ticket 25).

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Injector task (`executor.rs`, the single task that already serializes every `uinput` write) gates each write behind a suppression flag; when set, Fire-once/Hold-to-repeat/Toggle firing logic and internal Macro looping (including `run_toggle_loop`'s sleeps/iteration) continue completely unaffected — only the write to the virtual device is skipped.
- [ ] New `com.acheron.Daemon` D-Bus method for a client to push its current focus/suppression-desired state (level-set — reflects "should output be suppressed right now," not an edge-triggered toggle — so a client can call it redundantly with the same value with no ill effect).
- [ ] Suppression auto-clears if the connection that set it drops before it's explicitly cleared, via peer/name-disconnect detection on the Daemon's `zbus` connection.
- [ ] A running Toggle's state (`active_toggles`, its internal loop) is unaffected by suppression turning on or off — only whether its output reaches `uinput` changes. Confirmed via `GetState()` (or equivalent) showing the Toggle still active throughout a suppress/resume cycle.
- [ ] If suppression is requested by one client while another client (or no client) previously held it, behavior is well-defined and documented (recommend: last-write-wins on the flag, with disconnect-clear tied to whichever connection most recently set it).
- [ ] Test coverage at the existing `CaptureSource` fake-event seam:
  - [ ] Output is withheld while the flag is set, and resumes when explicitly cleared.
  - [ ] Output resumes automatically when the client that set the flag disconnects, without an explicit clear call.
  - [ ] A Toggle started before suppression continues looping/reporting as active throughout, and its buffered/next output reaches `uinput` again once suppression clears.
- [ ] `spec.md` and `CONTEXT.md` gain a new section documenting Daemon-output suppression, explicitly noted as separate from — and not an amendment to — the existing Toggle-stop-conditions list (ticket 04): a Toggle is never stopped by this mechanism, only its output delivery is gated.
