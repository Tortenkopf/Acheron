Type: prototype
Status: resolved

## Question

Design where and how the GUI surfaces the Daemon-running and Device-connected status added in [Decide systemd service packaging](./10-decide-systemd-service-packaging.md) (`NameOwnerChanged` watch on `com.acheron.Daemon`; `device_connected`/`DeviceConnectionChanged` from [Decide D-Bus interface surface](./08-decide-dbus-interface-surface.md)) — a header badge on Device Overview, tray icon state, or both — and what each of the three reachable states (running+connected, running+disconnected, not running) actually looks like, building on the Device Overview/tray design from [Design GUI information architecture](./09-design-gui-information-architecture.md).

## Answer

Prototype session, 2026-08-14, via `/prototype` (UI branch). Three structurally different variants, built live in a running GTK4 app switchable via a floating bottom bar (arrow buttons/keys) plus a debug "Simulate:" row cycling the three reachable states — compared live against the user, not decided unilaterally. Asset: [prototype/12-daemon-device-status-indicators/prototype.py](../../../prototype/12-daemon-device-status-indicators/prototype.py), reusing (not duplicating) ticket 09's `DaemonStub`/Device Overview/tray-mock rather than rebuilding them. Kept in place on `main` rather than split to a throwaway branch, matching the precedent set by [Design GUI information architecture](./09-design-gui-information-architecture.md) — there's no real Daemon/GUI implementation yet for a "validated decision" to be folded into; the prototype file itself is the primary source until that exists.

**Winner: Variant C — both, plus the grid disables itself.** A status chip (colour dot + label) above Device Overview *and* a matching line in the tray mock, both reflecting all three states. This settles the header-badge-vs-tray-vs-both question in favor of "both" — they're cheap to keep in sync from the same `GetState()`/signal data, and a user glancing at either place should see the same answer. Rejected Variant A (badge only, tray silent) and Variant B (tray only, main window silent) as each leaving one surface blind to the other.

**Second question this variant forced, and the user confirmed**: whether the device grid should stay clickable while disconnected/daemon-down, or block editing. **Blocks editing** — whenever status isn't running+connected, the whole Device Overview grid is disabled (`set_sensitive(False)`) and dimmed under a translucent `Gtk.Overlay` with a centered message ("Daemon not running — start it to edit Bindings" / "Device disconnected — plug in the Tartarus Pro to edit Bindings"). Rejected leaving it clickable (Variant A/B's implicit stance, since Bindings are just config data and editing doesn't technically require a live Daemon/device) — the user preferred the harder line once they saw it live.

**Implementation note surfaced while building this** (worth carrying into the real GTK4 code, not just this prototype): centering a message inside a `Gtk.Overlay` dim-layer requires `hexpand=True`/`vexpand=True` on the label *in addition to* `halign`/`valign = Gtk.Align.CENTER` — `halign`/`valign` alone only position a widget within space it has already claimed, and without expand flags a label claims just its own natural size, so it visually left-aligns instead of centering. Caught and fixed live during the session.

No new tickets surfaced. All tickets on the map are now resolved.
