# 16 — GUI MVP wired to the real Daemon

**What to build:** A running GTK4 GUI that lets the user see and edit Base-layer Fire-once Keypress bindings against the real Daemon over D-Bus — the first end-to-end "click a key in the GUI, see it take effect on hardware" slice. Build from `prototype/09-gui-information-architecture/prototype.py` directly: it already implements Device Overview, the Action Table sidebar, and the shared Binding editor against an in-memory `DaemonStub` — this ticket's job is adapting that structure to a real D-Bus client backend, not redesigning the layout. See `.scratch/tartarus-keybinder/spec.md` ("GUI information architecture (GTK4)") for the full design; at this ticket's scope, only Keypress/Fire-once editing needs to actually work end-to-end (Macro/other-Trigger-mode UI can exist inert, matching ticket 15's D-Bus scope).

**Blocked by:** 15

**Status:** ready-for-agent

- [ ] Device Overview (the main view) mirrors the physical device layout exactly as built in `prototype/09-gui-information-architecture/prototype.py` — grid, wheel-as-column-5-continuation, rotated thumbstick diamond, circular Mode key, key-20 paddle.
- [ ] The GUI's D-Bus client layer replaces the prototype's `DaemonStub` with a real connection to `com.acheron.Daemon` (PyGObject's `Gio.DBusProxy`, or `dbus-fast`/`dbus-next` — not `dbus-python`), while preserving a swappable-fake-backend seam for tests (same shape as `DaemonStub`).
- [ ] Clicking any device control opens the shared Binding editor in a popover; setting a Keypress binding calls `SetBinding` on the real Daemon and the popover reflects `GetConfig()`'s current state on open.
- [ ] The Action Table sidebar (collapsible, closed by default, no independent Profile/Layer pickers, "Show all inputs" checkbox, sidebar-open state surviving re-renders) is wired to the same real backend and the same shared Binding editor.
- [ ] On launch, the GUI opens straight to Device Overview reflecting `GetConfig()`/`GetState()` from the real Daemon — no separate onboarding wizard, even on a fresh seed `Default` Profile with everything passthrough.
- [ ] Live demo: with the real Daemon running, click a grid key in the GUI, set it to a Keypress remap, and immediately (no restart) see the remapped output when physically pressing that key.
- [ ] Automated GUI tests use a fake Daemon object implementing `com.acheron.Daemon`'s interface (the `DaemonStub` pattern from the prototype) to exercise Device Overview rendering, click-to-edit, and the Action Table's filter/show-all/open-state behavior without a real Daemon process.
