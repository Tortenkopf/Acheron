Type: task
Blocked by: 02, 03, 04

## Question

Write **`.scratch/tartarus-status-leds/spec.md`** — the effort's destination — from the
decisions and findings resolved across this map. This is a writing task: everything it needs
is settled by the time it's unblocked; nothing new is decided here.

The spec is the hand-off to a separate implementation effort. It should cover:

- **Feature summary** — every Profile carries a Status-LED assignment (three fixed-colour
  on/off toggles); the daemon asserts it whenever that Profile becomes active, and on startup.
- **The wire frame** — the exact bytes, from [ticket 02](./02-research-status-led-wire-protocol.md)
  and confirmed by [ticket 01](./01-prototype-status-led-controllability.md): frame layout,
  read-back, the "off" frame, VARSTORE choice, driver-mode dependency, any per-unit deviation.
- **Daemon architecture** — from [ticket 03](./03-daemon-architecture-for-status-leds.md):
  hidraw ownership across both capture modes, the Profile-switch hook point, startup
  assertion, device-absent / reconnect handling, and the re-assert hook (if
  [ticket 01](./01-prototype-status-led-controllability.md) criterion 3 required one).
- **Config schema** — from [ticket 04](./04-gui-daemon-surface-for-status-leds.md): the
  `config.toml` table, the `Profile` field, `schema_version` bump + migration.
- **D-Bus surface** — the new `Command` variant, `GetState()` additions / signal (if any).
- **GUI** — the three-toggle "Status LEDs" group on Device Overview: placement, which Profile
  it edits, how it reflects state.
- **Startup / shutdown behaviour** — assert on startup; shutdown-clear per the resolved fog
  item (clear all LEDs if [ticket 01](./01-prototype-status-led-controllability.md) criterion
  5 showed all-off is reachable; otherwise the resolved alternative).
- **Out of scope** — carry the map's Out-of-scope list into the spec so the implementation
  effort inherits the boundary.
- **CONTEXT.md** — note the `Status LED` / `Status LED assignment` glossary entries (added
  during charting) so the implementation effort keeps the vocabulary.

Also: add the effort's spec to `.scratch/README.md`'s line for this effort, and flip the map
+ this effort to a state that says "spec ready, implementation is a fresh effort".

**If [ticket 01](./01-prototype-status-led-controllability.md) killed the effort, this ticket
is closed unresolved** — no spec — and the map is archived with the negative result.
