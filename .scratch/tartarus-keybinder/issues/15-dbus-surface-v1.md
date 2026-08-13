# 15 — D-Bus surface v1 (live mutation, no restart)

**What to build:** The Daemon exposes its Base-layer Fire-once Keypress bindings over D-Bus, so a client can read and mutate config live without restarting the Daemon or hand-editing TOML. See `.scratch/tartarus-keybinder/spec.md` ("D-Bus surface (ADR-0004)") for the full design — this ticket implements the subset needed for ticket 14's scope (Base layer, Keypress, Fire-once); the remaining methods/fields (Macro, other Trigger modes, Layers, Profiles, `device_connected`) are added by later tickets reusing the same object/interface/encoding conventions rather than redesigning them.

**Blocked by:** 14

**Status:** ready-for-agent

- [ ] One flat D-Bus object `/com/acheron/Daemon` on bus name `com.acheron.Daemon`, one combined interface (also `com.acheron.Daemon`) — no ObjectManager hierarchy.
- [ ] `GetConfig() -> a{sv}` returns the entire config document (the `Default` Profile's Base-layer bindings, at this ticket's scope).
- [ ] `GetState() -> (profile: s, layer: s, active_toggles: as, device_connected: b)` returns the live runtime snapshot — `layer` and `active_toggles` can be fixed/stub values at this ticket's scope (`"base"`, empty list) since Layers/Toggles don't exist yet; `device_connected` can be hardcoded `true` (real detection is ticket 20's scope).
- [ ] `SetBinding` and `ClearBinding` are atomic, per-entity methods: each call validates, applies in-memory, and rewrites `config.toml` immediately — no draft/save step, no buffered state.
- [ ] `Input` marshals as a plain string reusing its `Display`/`FromStr` form (ticket 14). `Action`/`MacroStep`/`Binding` marshal as `a{sv}` dicts with a `"type"` tag key (hand-written `Serialize`/`Deserialize`, not a JSON-string fallback) — build this encoding to already accommodate the `Macro`/other-Trigger-mode shapes described in spec.md even though only `Keypress`/`FireOnce` are exercised yet.
- [ ] A small set of named errors under `com.acheron.Daemon.Error.*` exists: `NotFound`, `AlreadyExists`, `InvalidBinding`, `IoError`.
- [ ] `ActiveProfileChanged`, `ActiveLayerChanged`, and `ActiveTogglesChanged` signals are wired (even though nothing yet changes Profile/Layer/Toggles at this ticket's scope — they should fire correctly once later tickets add the triggering behavior).
- [ ] Live demo: a small script/CLI (e.g. `dbus-send` or a throwaway Python snippet) calls `SetBinding` for a grid key, the Daemon's live remap changes with no restart, and `config.toml` on disk reflects the change; `GetConfig()` reflects it too.
- [ ] Automated tests exercise the D-Bus methods directly (real `zbus` server, in-process) combined with ticket 13's fake `CaptureSource` to assert the full path: D-Bus mutation → dispatch state → injected output.
