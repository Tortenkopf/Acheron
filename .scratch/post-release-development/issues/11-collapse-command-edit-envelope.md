<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 11 — Collapse the `Command` edit envelope into one `Command::Apply`

**What to build:** `daemon/src/command.rs`'s `Command` enum drops its 24
mutating variants for a single `Apply { edit: edit::Edit, reply }`, and
`dispatch::handle_command`'s 24 field-for-field translation arms collapse to
one. The D-Bus method layer builds `edit::Edit` values directly and hands
them to the dispatch task through two new helpers that absorb the channel
round-trip. `CommandError` moves from `command.rs` to `edit.rs`.

`Command` after the carve carries four variants:

```rust
pub enum Command {
    GetConfig(oneshot::Sender<Config>),
    GetState(oneshot::Sender<State>),
    StopAllToggles { reply: oneshot::Sender<()> },
    Apply {
        edit: edit::Edit,
        reply: oneshot::Sender<Result<Option<edit::CreatedId>, CommandError>>,
    },
}
```

No behaviour changes. Every mutating path runs the exact same
`edit::plan` → persist → `run_effects` sequence it does today; only the
message shape between `dbus` and `dispatch` changes.

## The friction

`command.rs` (330 lines) and `dispatch::handle_command` (250 lines) are a
shallow pass-through layer over `edit::Edit`. `edit.rs`'s own module doc
names the shape outright — _"one data-only variant per mutating `Command`
(24), carrying the same fields minus the `reply` sender"_ — and
`CONTRIBUTING.md`'s "Adding a new mutating `Command`" recipe is five steps,
two of which (`command.rs` variant, `handle_command` arm) are pure
mechanical echo:

- **`Command`'s 24 mutating variants** re-declare `edit::Edit`'s fields
  verbatim, wrapped in a `oneshot` reply sender. The two enums must be kept
  in lockstep by hand; a field added to one and forgotten on the other is a
  silent bug the type checker only catches at the `handle_command` arm.
- **`handle_command`'s 24 arms** are almost entirely
  `commit!(reply, edit::Edit::SameName { same, fields })`. Two carry a
  minted-id early return (`CreateMacro` / `CreateStepper`); one carries a
  same-name guard (`RenameProfile`).
- **`dbus/mod.rs`'s ~22 mutating methods** each repeat the identical
  `let (reply, rx) = oneshot::channel(); self.commands.send(Command::X {
  … }).await.map_err(dispatch_gone)?; rx.await.map_err(dispatch_gone)?
  .map_err(DaemonError::from)` tail — the duplication `command.rs` was
  meant to factor out, pushed one layer down.
- **The per-variant doc comments** (~180 lines of failure-mode contract on
  `Command`) sit on the type that *forwards* the request, not the type
  (`edit::Edit` / `edit::plan`) that *enforces* it — where the matching
  variants have no docs at all.

Deletion test: removing `Command`'s mutating half concentrates complexity
into `edit` (the one module that already owns every rejection path and every
effect) and shrinks `dbus` — nothing moves sideways.

## Relationship to tickets 03–10

Ticket 03 made *edit + persist* atomic; 04 single-sourced *validation*; 05
lifted the whole config transaction into the pure `edit` module and
deliberately made `Edit` its own data-only type (_"not reused `Command`"_) —
it accepted the 24-arm `handle_command` translation as the cost of that
split, without ruling out removing it later. 06 built the contract-tested
GUI `rules` mirror; 07/08 carved the Chord and Trigger-mode state machines
into pure modules; 09 concentrated the dispatch task's runtime state; 10
carved the last two Depth-fed engines out of `dispatch.rs`. This ticket
removes the shallow envelope 05 left standing between `dbus` and `edit` — it
does not touch `edit::Edit`, `edit::plan`, `edit::apply`, or the `Effect`
surface, all of which 05 settled.

## What moves, what stays

- **Into `edit.rs`:**
  - `CommandError` (name unchanged — still "the reason an `Edit` was
    rejected") and its `impl From<config::ConfigError>`. Breaks the
    `command` ↔ `edit` module cycle that `Command::Apply { edit }` would
    otherwise create: final DAG is `dbus → command → edit → config`.
  - The per-variant failure-mode doc comments from `Command`'s 24 mutating
    variants, onto the matching `edit::Edit` variants, beside the `plan` arm
    that produces each `Err`.
- **Into `dbus/mod.rs`** (two `impl Daemon` helpers, next to `parse_input`):
  - `async fn apply(&self, edit: Edit) -> Result<(), DaemonError>` — used by
    the 22 non-create mutating methods. Absorbs the
    `oneshot::channel` + `send` + `dispatch_gone` + `rx.await` +
    `DaemonError::from` tail.
  - `async fn apply_creating(&self, edit: Edit) -> Result<CreatedId,
    DaemonError>` — used by `create_macro` / `create_stepper`. Same tail,
    plus the `Option<CreatedId>` unwrap (`expect` on `None`, mirroring
    today's `let Some(…) else { unreachable!() }` in `handle_command`).
  - Each mutating method reduces to: parse wire args →
    `self.apply(Edit::X { … }).await` (or `apply_creating`, then map the
    `CreatedId` to its wire string).
- **Stays in `dbus/mod.rs`:** `get_config` / `get_state` inline (infallible
  reply, no shared shape); `set_output_suppressed` / `start_depth_stream` /
  `stop_depth_stream` untouched (they never went through the `Command`
  channel — injector calls and spawned tasks).
- **Stays in `dispatch.rs`:** `run`, the `select!` loop, `run_effects`,
  `run_chord_effects`, and the three surviving `handle_command` arms
  (`GetConfig`, `GetState`, `StopAllToggles`). The `Apply` arm is the
  current `commit!` body inlined once (the macro had one user; drop it):
  `edit::apply` → `reply.send(Ok(outcome.created))` **before**
  `self.run_effects(outcome.effects, config).await`. Reply-before-effects
  ordering unchanged.
- **`RenameProfile` same-name guard — dropped.** Today `handle_command`
  intercepts `old_name == new_name` and replies without calling
  `edit::apply`, to skip a no-op `config.toml` rewrite. In the collapsed
  arm that guard needs a `Config` read the D-Bus layer doesn't have and a
  special-case in the arm that is meant to be generic. A same-name rename
  now flows through `edit::plan` normally: it is still existence-checked
  (→ `NotFound` for a missing Profile), and the only consequence for a
  Profile that *does* exist is one rewrite of byte-identical `config.toml`
  content, on an operation a user essentially never performs. **This is a
  deliberate simplification — do not restore the guard.**
- **`command.rs` after:** `State`, `Command` (4 variants), and
  `use crate::edit::{Edit, CreatedId, CommandError}`. Still the message
  contract on the `dbus` ↔ `dispatch` channel. Module doc updated — it no
  longer "never touches `Config` directly" (it holds an `Edit`, which
  carries `Binding` / `AxisTarget` / … ).
- **Unchanged:** `edit::Edit`, `edit::plan`, `edit::apply`, `edit::Outcome`,
  `edit::CreatedId`, every `edit::Effect` variant and its `run_effects`
  handler; `DaemonError` and its `com.acheron.Daemon.Error.*` wire set;
  `wire.rs` (still marshals `Binding` / `Action` / `State`); the entire GUI
  side (`daemon_client.py`, `wire.py`, `daemon_stub.py`) — no wire protocol
  change. `SwitchProfile` needs no special handling: it is already an
  `edit::Edit` variant and flows through `Apply` like the rest.

## `CONTRIBUTING.md`

The "Adding a new mutating `Command`" recipe collapses from five steps to
three:

1. Add the `edit::Edit` variant.
2. Add the `edit::plan` arm — mutate the `Config` clone, early-return
   `Err(CommandError::…)` for each operation precondition, push any
   `Effect`s. Add its failure-mode doc comment on the variant.
3. Add the D-Bus method in `dbus/mod.rs`: parse the wire args, call
   `self.apply(Edit::… )` (or `self.apply_creating` if it mints an id).

Plus the existing "only if the mutation needs post-commit runtime work" step
for a new `Effect` variant + `run_effects` handler. No `command.rs` step, no
`handle_command` step.

## Landing

One PR (ticket 03–10 precedent). This is the **command path**, not the
latency-critical capture/injector path, and every mutating operation is
already covered end-to-end by `CommandHarness` integration tests and by
`edit::tests`' synchronous rejection tables (ticket 05) — so the strict
line-by-line body-diff protocol tickets 07/08/10 used is not warranted here.
The 24 arms are already one-liners; the only translation risk is a dropped
field, which the type checker and the per-operation harness tests catch
directly.

- **`/code-review` (Standards + Spec)**, findings dispositioned, as tickets
  05 / 07 / 08 / 10 did.
- Full Daemon suite green; `cargo fmt --check` clean; `cargo clippy
  --all-targets -- -D warnings` clean. GUI and packaging suites untouched
  and green (no wire / D-Bus signature / catalog change).

## Tests: adjust in place, don't rewrite

- **`CommandHarness`'s ~20 typed helper methods** (`set_binding`,
  `set_default_actuation`, …) keep their signatures and return types; each
  builds `Command::Apply { edit: Edit::X { … }, reply }` internally. The
  ~49 call sites are untouched.
- **`edit::tests`** already owns the pure precondition/rejection coverage
  (ticket 05). Unchanged.
- **The dispatch harness** keeps its integration tests — effects,
  persistence rollback, live-apply, cross-profile stepper steal — now
  reaching `edit` through `Command::Apply`.
- **Check for a same-name-rename assertion.** No current test names it
  (only the `handle_command` comment does), but if one exists asserting "a
  same-name rename does not rewrite `config.toml`", update it to the new
  behaviour (the rewrite is allowed).
- No new test files. The carve removes a layer; it does not add a surface.

## Out of scope

- **Any change to `edit::Edit` / `edit::plan` / `edit::apply` / `Effect`**
  or `config::validate`. Ticket 05 settled that surface.
- **Folding `CommandError` into `DaemonError`.** Considered; rejected —
  `edit` is the deep domain module and must not depend on `zbus::DBusError`
  derives. The `impl From<CommandError> for DaemonError` stays one
  conversion in `dbus`.
- **A `query` helper for `GetConfig` / `GetState`.** Three short methods, no
  shared `Result` shape — inline is fine.
- **Any GUI-side change.** The wire protocol is byte-identical.
- **Merging `command.rs` into `dispatch.rs`.** `Command` is the channel
  contract between two modules; a 4-variant enum earns its file alongside
  `State`.

**Blocked by:** None — tickets 05 (`edit`) and 09 (`DispatchState`) are
resolved (`9a59e37`).

**Status:** resolved

- [x] `Command` has exactly four variants: `GetConfig`, `GetState`,
      `StopAllToggles`, `Apply { edit, reply }` where `reply` is
      `oneshot::Sender<Result<Option<edit::CreatedId>, CommandError>>`. The
      24 mutating variants no longer exist.
- [x] `dispatch::handle_command` has four arms; the `Apply` arm is the sole
      mutating path — `edit::apply`, `reply.send` before `run_effects`, no
      per-operation special-casing. The `commit!` macro is gone.
- [x] `CommandError` and its `impl From<config::ConfigError>` live in
      `edit.rs`. `edit.rs` imports nothing from `command`; `command.rs`
      imports `edit::{Edit, CreatedId, CommandError}`. No module import
      cycle.
- [x] `dbus/mod.rs` exposes `Daemon::apply` and `Daemon::apply_creating`;
      every mutating method is `parse args → self.apply*(Edit::… ).await`
      with no inline `oneshot::channel` / `commands.send` / triple
      `map_err`. `get_config` / `get_state` unchanged.
- [x] The `RenameProfile` same-name guard is removed from `handle_command`;
      a same-name rename of an existing Profile succeeds (with a `config.toml`
      rewrite), of a missing one fails `NotFound`.
- [x] Every mutating `edit::Edit` variant carries the failure-mode doc
      comment moved from its old `Command` variant; `command.rs`'s 24
      variant docs are gone and its module doc is updated.
- [x] `CONTRIBUTING.md`'s "Adding a new mutating `Command`" recipe is the
      three-step form; no `command.rs` / `handle_command` steps remain.
- [x] `CommandHarness`'s typed helpers keep their signatures; no dispatch
      test call site changed except where it referenced a removed
      `Command::` variant name.
- [x] Full Daemon suite green (367); `cargo fmt --check` + `cargo clippy
      --all-targets -- -D warnings` clean. GUI suite green (397); packaging
      untouched (no wire / D-Bus signature / desktop / service change).
- [x] `/code-review` (Standards + Spec) run and its findings dispositioned.

## Comments

**2026-09-01** — Filed from an architecture-review grilling session
(candidate 1 of a review whose other candidates were: single-sourcing the
D-Bus wire tag vocabulary through the `schema.rs` fixture, and a GUI
`read_model` module — both left unfiled, the second already covered by ADR
0005's "fold opportunistically" note). Design tree settled over three
rounds:

- **Worth doing:** yes — largest remaining shallow module in the 03–10 hot
  spot, clean deletion test, and it removes a hand-maintained lockstep
  between `Command` and `edit::Edit`. Ticket 05 chose "`Edit` is its own
  type, not reused `Command`"; that holds — `Edit` stays the data type, only
  `Command`'s mirror of it goes.
- **`Command::Apply { edit, reply }`**, reply
  `Result<Option<CreatedId>, CommandError>` — effects never cross back to
  D-Bus today and shouldn't start; `Option<CreatedId>` is `None` for 22
  edits, `Some` for the two creates.
- **Two `dbus` helpers**, not one (`apply` / `apply_creating`) — keeps the
  common path free of `Option<CreatedId>` noise, localizes the
  "a create always mints an id" invariant.
- **`RenameProfile` same-name guard dropped.** First recommendation was to
  move it to the `rename_profile` D-Bus method; that fails — the `NotFound`
  branch needs a `Config` read the D-Bus layer doesn't own. Dropping it
  costs one idempotent `config.toml` rewrite on an operation nobody
  performs, versus a special-case in the one generic arm.
- **`CommandError` moves to `edit.rs`** to break the `command` ↔ `edit`
  cycle `Command::Apply { edit }` introduces. It is now produced entirely by
  `edit::plan`, so it belongs there.
- **`command.rs` kept** (not merged into `dispatch`) — it is the channel
  message contract; `State` + a 4-variant enum is a fine size.
- **Docs move to `edit::Edit` variants** — the failure-mode contract
  belongs on the enforcing type, next to the `plan` arm.
- **Typed `CommandHarness` helpers kept** — ticket 05 already did the big
  harness shrink; churning 49 call sites buys no readability.
- **Lighter landing protocol** than 07/08/10 — command path not input path,
  arms already one-liners; single `/code-review`, existing suites as the
  gate, no line-by-line body diff.
- **No CONTEXT.md change, no ADR, no `domain-modeling` run** (07/08/10
  precedent — `Command::Apply` is implementation vocab). One `CONTRIBUTING.md`
  recipe edit.

Facts dug from the code during the grilling (not asked of the user):

- Only `dbus/mod.rs` (production) and the `dispatch` test harness ever
  construct a `Command`; no external Rust consumer, no `main.rs` call site.
- `edit.rs` imports `crate::command::CommandError` today — so
  `Command::Apply { edit: edit::Edit }` would close a `command` ↔ `edit`
  module cycle (legal in Rust, but a smell the 03–10 arc has avoided).
- `handle_command`'s `commit!` macro has exactly one user (itself, 22×);
  the two create arms and the `RenameProfile` arm are written longhand.
- No test asserts the same-name-rename no-write behaviour; only the
  `handle_command` comment at `dispatch.rs:699` documents it.
- `StopAllToggles` is the only mutating-looking `Command` with no `Edit`
  (pure runtime — drains the `toggles` map, no `Config` touch), so it stays
  a distinct variant. `SwitchProfile` *does* go through `edit`.
