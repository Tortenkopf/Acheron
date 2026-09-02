Type: grilling
Blocked by: 01, 03

## Question

Decide the **GUI ↔ daemon surface** and the **config schema** for Status-LED assignments.
Grilling + domain-modeling against the real code. Decisions only — no build. Builds on the
architecture settled in [ticket 03](./03-daemon-architecture-for-status-leds.md).

Settle at least:

- **`config.toml` schema.** Confirmed shape (charting Q15): a named table per Profile,
  `status_leds = { orange = true, green = false, blue = false }` under `[profiles.<name>]`.
  Pin the exact key names, the Rust type on `Profile` (`daemon/src/config.rs:130` — e.g. a
  small `StatusLeds` struct vs `[bool; 3]`), the `#[serde]` attributes, and the
  `schema_version` bump + migration (all-off default for Profiles without the key). Check
  against the existing migration tests in `config.rs`.
- **D-Bus command.** New `Command` variant to set a Profile's Status-LED assignment (shape it
  like `SetBinding` / `SetActuationPoint` — Profile-scoped, atomically persisted). Name it;
  decide whether it sets the whole triple or one channel at a time.
- **`GetState()` exposure.** Does the GUI need the active Profile's current Status-LED state
  reported back (like `capture_mode` was added in `tartarus-input-expansion` ticket 17), and
  a change signal? Or is the config the single source of truth the GUI already has?
  ([Ticket 01](./01-prototype-status-led-controllability.md) found no on-device keymap switch,
  so the hardware can't diverge from config *behind the daemon's back* — but the brief
  orange-only window on every connect, before the daemon asserts, is a real transient state.)
- **Storage-mode config knob — likely drop.** Ticket 02's write-up floated a NOSTORE/VARSTORE
  config option; [ticket 01](./01-prototype-status-led-controllability.md) found the byte
  inert on our unit (nothing persists either way). Confirm there is nothing user-visible to
  expose and this knob is cut.
- **The GUI control.** Confirmed (charting Q4/Q14): three labelled colour toggles in a
  "Status LEDs" group on the Device Overview panel, near the Profile selector. Pin down which
  file (`gui/acheron_gui/...`), whether it edits the *active* Profile or the
  *currently-selected-for-editing* Profile, and how it reflects state (the toggles show the
  selected Profile's stored triple).
- **GUI rules mirror.** Does any of this need to land in the GUI's `rules` mirror module
  (`post-release-development` ticket 06)? Likely not — there's no validation beyond "three
  bools" — but confirm.

Output: `## Answer` with the settled surface + schema; append the gist to the map's Decisions
so far.
