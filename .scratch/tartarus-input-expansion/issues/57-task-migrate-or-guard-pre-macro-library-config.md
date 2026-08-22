Type: task
Blocked by: None — can start immediately

## Question

Spawned live from [ticket 53](./53-task-verify-macro-library-on-hardware.md)'s own hardware-verification session: ticket 51's Answer flagged, twice, that a pre-ticket-51 `config.toml` with an inline `Action::Macro { steps }` Binding would fail to parse under the new `Action::Macro { macro_id }` shape, and left migrating it explicitly out of scope. Ticket 53 hit exactly this, live, against the user's real `config.toml` — not a hypothetical: the Daemon refused to start at all (`missing field 'macro_id'`), which under `systemd --user`'s restart policy crash-looped it (`Start request repeated too quickly`) rather than just failing once with a readable message. Verification was unblocked by hand-editing the one affected Binding into the new `[macros.*]`/`macro_id` shape directly in the user's `config.toml` — a data fix, not a code fix — but the underlying gap is unfixed: anyone else upgrading across this same boundary with an inline-Macro Binding on disk hits the identical crash-loop.

Decide and build one of:

- A real migration: on load, detect the old inline `Action::Macro { steps: [...] }` shape (still `type = "macro"`, but with a `steps` array and no `macro_id`) and synthesize a `MacroDef` for it into `Config.macros` (name = e.g. `"Migrated Macro"` + collision-suffixed via the existing `unique_macro_id` slug algorithm), rewriting the Binding to reference it — then persist the migrated file back to disk once, the same way `SCHEMA_VERSION` bumps already rewrite-on-load elsewhere in this codebase (check `config.rs` for the existing precedent before designing a new one).
- Or, at minimum, a guard: `parse()` recognizes the old shape specifically (rather than falling through to serde's generic "missing field" error) and refuses to start with a clear, actionable message naming the affected Binding(s) and what to do — and the systemd unit's restart policy is reviewed so this failure mode doesn't crash-loop (e.g. `Restart=no` or a backoff long enough to be readable in `systemctl status`, rather than the current rapid-restart-then-give-up).

Whichever direction: no live hardware required to decide or build (config-parsing/systemd-unit work only, mirrors ticket 51/54's own AFK Daemon-only precedent) — hardware only needed if the settled direction is later re-verified end-to-end, which can be its own follow-up if warranted.
