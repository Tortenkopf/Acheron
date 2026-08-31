<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 01 — GUI hard-crashes during a Daemon outage instead of showing the offline overlay

**What to build:** Launching the GUI while the Daemon is unreachable — genuinely
down, or still running but orphaned from the session bus — must show the existing
"Daemon not running" Device Overview overlay, not abort with an unhandled
`KeyError: 'default_actuation'` and leave no window at all.

Root cause: with the Daemon unreachable the GUI falls back to
`device_overview.PLACEHOLDER_CONFIG`, but that placeholder Profile omits the
`default_actuation` and `actuation_overrides` keys. The Actuation section of the
Binding editor reads both unconditionally while `build_main_view` eagerly builds
an editor for every grid Input, so the first Input build raises and the exception
propagates out of the application's `do_activate`. This is the same class of gap
already closed for the `axis_base` / `axis_held` keys in ticket 71 — the
ticket-17 Actuation keys were just missed when the placeholder was last widened.

Fix the placeholder so its Profile matches the shape the Daemon actually
serializes for a Profile (Actuation keys included, with sane default values), and
cover the daemon-down path with a GUI test so a future missing key fails in CI
instead of on a user's machine.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] With no Daemon on the bus, launching the GUI renders the Device Overview
      with the "Daemon not running" status treatment and no traceback.
- [ ] The placeholder Config's Profile carries every key the Daemon includes when
      it serializes a Profile, including `default_actuation` and
      `actuation_overrides`, with values consistent with the Daemon's own
      defaults.
- [ ] Opening a Binding editor for a grid Input while on the placeholder Config
      does not raise.
- [ ] A GUI test builds the main view against a daemon-down status and asserts
      the view builds without error; it would have failed before this change.
- [ ] Existing GUI, Daemon, and packaging test suites stay green.
