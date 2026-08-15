Type: grilling
Status: resolved

## Question

Decide what happens the first time Acheron runs with no existing `~/.config/acheron/config.toml`: does the Daemon create a config preloaded with the Tartarus Pro's onboard default mapping (using the Input table from [Enumerate physical inputs](./01-enumerate-physical-inputs.md)), start with an empty/minimal Profile, or prompt the user via the GUI — and what's the minimum viable default Profile/Layer structure to ship as that seed.

## Answer

Grilling session, 2026-08-14.

**Missing file** — the Daemon (never the GUI, per [Decide config file format](./03-decide-config-file-format.md)'s exclusive-ownership decision) creates `~/.config/acheron/` and `config.toml` itself on startup if missing, and writes it to disk **immediately** rather than lazily deferring to the first mutation — keeps the invariant "file on disk always matches in-memory state" with no window where `GetConfig()` reflects something unpersisted.

**Seed content** — one Profile named **`Default`**, `schema_version = 1`, both Layers (`Base`/`Held`) present at the type level but with **empty** Binding maps — every Input passthrough, nothing explicitly bound — `mode_key_role = "layer_switch"`, set as the active Profile. Explicit "echo" Bindings reproducing the onboard mapping for all 28 Inputs were rejected: behaviorally identical to [Decide Daemon data model](./06-decide-daemon-data-model.md)'s sparse-map passthrough, pure boilerplate, and unnecessary for discoverability since [Design GUI information architecture](./09-design-gui-information-architecture.md)'s Device Overview already lets you click any physical control — bound or not — to open its binding editor.

**Corrupt or unparseable file** (parse failure, or an unsupported `schema_version`) — the Daemon refuses to start: exits non-zero with a clear parse error to the journal, **no silent backup-and-reseed**. Same class of "genuine error, not a recoverable condition" that [Decide systemd service packaging](./10-decide-systemd-service-packaging.md) drew a line around for capture failures — silently overwriting risks discarding an in-progress hand-edit. The machinery to surface this already exists end-to-end: `journalctl --user -u acheron-daemon` shows the parse error, the GUI's `NameOwnerChanged` watch shows "Daemon not running," and its `ResetFailedUnit`+`StartUnit` safety net won't paper over it — it fails again immediately until the file is fixed by hand, which is correct.

**GUI first-run treatment** — no separate onboarding wizard or welcome dialog. The GUI opens straight to Device Overview against the seed `Default` Profile (an all-passthrough grid); that view, letting the user click any control to bind it, *is* the first-run experience. A dedicated wizard would be throwaway UI solving a problem [Design GUI information architecture](./09-design-gui-information-architecture.md) already solves.

No new tickets surfaced.
