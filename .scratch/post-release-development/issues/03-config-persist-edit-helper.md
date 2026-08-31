<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 03 — Replace the 24 hand-rolled config-rollback blocks with one `config::persist_edit` helper

**What to build:** Every mutating `Command` handler in the dispatch task must
persist its `Config` change through a single snapshot-and-restore helper instead
of its own bespoke `if result.is_err()` reversal. A `config.toml` write failure
must leave in-memory `Config` exactly matching disk — for every command, by
construction — so `GetConfig()` can never report a change that isn't saved.

Today `handle_command` carries 24 mutating arms, each following the same shape:
validate, mutate `config` while capturing the prior state, `persist().await`,
then a hand-written block that reverses precisely that mutation on failure. The
reversal is bespoke per arm and has to stay in lockstep with the do-logic above
it — `SetBinding` and `SetChordBinding` each carry ~40 lines of undo (restore the
binding, re-insert every stepper binding *and* chord binding a `Step` action
stole from elsewhere), `SetAxisAssignment` reverses a binding removal, a set of
chord removals, and an axis insert together, and `RenameProfile` already
snapshots the whole `profiles` map by hand because its cascade defeats
fine-grained undo. None of these rollback paths has any test coverage.

Generalise `RenameProfile`'s whole-map approach to every arm:

```rust
// lives in config.rs; `E: From<ConfigError>` keeps this module from
// naming command::CommandError. The private `persist` spawn_blocking
// wrapper moves here from dispatch.rs.
pub(crate) async fn persist_edit<T, E>(
    config: &mut Config,
    path: &Path,
    edit: impl FnOnce(&mut Config) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<ConfigError>,
{
    let snapshot = config.clone();
    let value = match edit(config) {
        Ok(v) => v,
        Err(e) => { *config = snapshot; return Err(e); }
    };
    match persist(config, path).await {
        Ok(()) => Ok(value),
        Err(e) => { *config = snapshot; Err(e) }
    }
}
```

Each mutating arm then moves its validation and mutation into the closure and
deletes its reversal block. On-success side effects that aren't config
(publishing the actuation snapshot, recomputing axis output, signalling the
capture supervisor, dropping/clamping a stepper cursor, stopping a Toggle,
emitting `ActiveProfileChanged`) stay inline in the arm, after the helper returns
`Ok` — the helper's contract is strictly "atomic config edit + persist", nothing
more. `switch_profile` folds only its `active_profile` replace + persist + rollback
into the helper and keeps its four post-commit effects inline; it stays a
separate function. `CreateMacro` / `CreateStepper` return their new id as the
helper's `T` instead of `result.map(|()| id)`.

Two knock-on cleanups belong in this same change: `take_stepper_direction_elsewhere`
and `take_stepper_direction_elsewhere_from_chords` currently return
`Vec<...>` of the bindings they moved *solely* so the two callers can re-insert
them on rollback — with a whole-`Config` snapshot those returns are dead, so both
drop to `()`. And `RenameProfile` keeps its existing `old_name == new_name`
short-circuit (return `Ok(())` with no disk write) as a guard in the arm, ahead
of the `persist_edit` call — it's the one arm with an `Ok` early-return, and
routing it through the helper would add a spurious write.

Convert all 24 arms in one pass, not incrementally — a half-converted
`handle_command` carrying two rollback idioms is harder to read than either
alone.

**Blocked by:** None — can start immediately.

**Status:** resolved

- [x] `config::persist_edit` exists with a generic `E: From<ConfigError>` error
      parameter; `config.rs` does not name `CommandError`; the `spawn_blocking`
      persist wrapper is private to `config.rs` and `dispatch.rs` no longer
      defines its own `persist`.
- [x] No `if result.is_err()` / manual-reversal block remains in `handle_command`
      or `switch_profile`; every mutating arm routes its edit through
      `persist_edit`.
- [x] `take_stepper_direction_elsewhere` and
      `take_stepper_direction_elsewhere_from_chords` return `()`.
- [x] On-success non-config side effects (actuation snapshot, axis recompute,
      capture-control signal, stepper-cursor cleanup, Toggle stop,
      `ActiveProfileChanged`) still fire, and only after a successful persist.
- [x] `RenameProfile` with `old_name == new_name` still performs no disk write.
- [x] Three unit tests on `persist_edit`: closure returns `Err` → `config`
      untouched, file unwritten, that error propagates; `write` fails (parent is
      a regular file) → `config` restored to its pre-closure value, error is
      `ConfigError::Io`; success → file written, `Ok(T)` carries the closure's
      value.
- [x] One dispatch-harness integration test: a persist failure on a `SetBinding`
      whose `Step` action steals a stepper direction from another Profile leaves
      both the target Layer and the donor Profile's binding intact.
- [x] Daemon test suite — including the D-Bus `*_persists` happy-path tests —
      stays green (384 tests); `cargo clippy` is clean.

## Comments

**2026-08-31** — Resolved in `8259fb9`. All 24 mutating sites (23 `handle_command`
arms + `switch_profile`) now route through `config::persist_edit`; the private
`persist` `spawn_blocking` wrapper moved to `config.rs` and now serializes on the
caller's task so it no longer double-clones `Config`. `/code-review` found no
correctness bug; its two cleanup notes were addressed (the double-clone) or
scoped out (`config::write` is still a non-atomic truncating `fs::write` — a
torn-file-on-crash durability concern orthogonal to snapshot-and-restore, noted
in `persist_edit`'s doc comment; worth its own ticket if it matters).

**2026-08-31** — Superseded by ticket 05. `config::persist_edit` no longer
exists: its job is now split across `edit::plan` (mutate a `Config` clone +
run `config::validate`) and `edit::apply` (`config::persist` the clone, then
assign on success). The snapshot-and-restore this ticket introduced collapsed
to "don't assign the clone on failure" once the edit stopped happening
in-place on the caller's `Config`. `config::persist` is now `pub(crate)` and
called only from `edit::apply`.
