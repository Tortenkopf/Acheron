<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 16 — Concentrate the epoch-guarded disconnect-cleanup protocol into one `ConnectionScopedTask`

**What to build:** `SuppressionState` and `DepthStreamState` — two hand-rolled
`{ epoch: u64, watcher: Option<JoinHandle<()>> }` structs in
`daemon/src/dbus/mod.rs` — and the abort-and-supersede race written out around
them in **six** places collapse behind one module,
`daemon/src/dbus/connection_scoped_task.rs`:

```rust
// A background task tied to the lifetime of one D-Bus connection: superseded
// when the same method re-arms it, torn down when the arming connection
// drops. Owns the epoch guard, the JoinHandle, and watch_disconnect.
// Nothing outside `dbus` needs it — `mod connection_scoped_task;`, private.

pub(super) struct ConnectionScopedTask { /* Arc<Mutex<Inner>> */ }

impl ConnectionScopedTask {
    pub(super) fn new() -> Self;

    /// Bump the epoch and abort the current watcher in one locked section.
    /// The returned token is valid only until the next `supersede`; dropping
    /// it unfinished leaves this task disarmed.
    #[must_use]
    pub(super) fn supersede(&self) -> ArmToken;

    /// `supersede` then drop — bump + abort, no re-arm.
    pub(super) fn disarm(&self);
}

impl ArmToken {
    /// Spawn the scoped task and store its handle — but only if no
    /// `supersede` has run since this token was minted (checked against the
    /// epoch captured at `supersede` time, exactly as the current
    /// store-if-current check does across its `.await`). The spawned task
    /// races `watch_disconnect(connection, sender)` against `pump`; whichever
    /// resolves first ends it, and then — iff this arming is still current —
    /// the watcher slot is cleared and `on_clear` is awaited.
    pub(super) fn finish(
        self,
        connection: zbus::Connection,
        sender: Option<String>,
        pump: impl Future<Output = ()> + Send + 'static,
        on_clear: impl FnOnce() -> F + Send + 'static,
    ) where F: Future<Output = ()> + Send;
}
```

Each D-Bus method becomes a `supersede()` / `finish(…)` / `disarm()` call:

| method | today | after |
| --- | --- | --- |
| `set_output_suppressed(true)` | supersede block + injector call + spawn + store-if-current (`mod.rs:337–380`) | `let t = self.suppression.supersede(); injector.set_suppressed(true).await?; t.finish(conn, sender, pending(), \|\| async { injector.set_suppressed(false).await; })` |
| `set_output_suppressed(false)` | supersede block + injector call (`337–349`) | `injector.set_suppressed(false).await?; self.suppression.disarm()` (order-independent — the aborted watcher only ever drives toward `false`) |
| `start_depth_stream` | supersede block + spawn + store-if-current (`799–826`) | `let t = self.depth_stream.supersede(); t.finish(conn, sender, run_depth_stream(conn2, depth_rx, input), \|\| async {})` |
| `stop_depth_stream` | supersede block (`838–844`) | `self.depth_stream.disarm()` |

**No behaviour change**, with one deliberate, unobservable deletion: the
`DepthStreamState.input` field (`mod.rs:96`) is **write-only** — assigned at
`230`, `805`, `843` and read nowhere (no `GetState` path, no test, no
introspection). It is removed. `run_depth_stream` already takes its `input`
as a plain argument, not from the shared state. Everything else — the
last-writer-wins epoch attribution, the "capture the epoch before the
`.await`, re-check after" store guard, the disconnect auto-clear, the
`interval_at`-not-`interval` stray-signal guard — is preserved exactly.

## The friction

`set_output_suppressed` (ticket 24) and `start_depth_stream` /
`stop_depth_stream` (ticket 26) implement the same protocol: a
connection-scoped background task that must be **superseded** when the same
method is called again and **torn down** when the calling connection drops.
The mechanism is:

> bump the epoch and abort the old watcher **in one locked section**; store
> the new handle (or run the disconnect-clear) **only if `state.epoch` still
> matches** the value captured in that section.

It is spelled out six times:

- `set_output_suppressed` — supersede at `mod.rs:337–344`, disconnect-clear-
  if-current in the spawned watcher at `357–365`, store-handle-if-current at
  `375–380`.
- `run_depth_stream` tail — disconnect-clear-if-current at `228–232`.
- `start_depth_stream` — supersede at `799–807`, store-if-current at
  `821–826`.
- `stop_depth_stream` — supersede at `838–844`.

Both structs carry the same two fields (`SuppressionState` at `81–84`,
`DepthStreamState` at `94–98`, the latter plus the now-dead `input`). The
emphatic 8-line comment at `mod.rs:329–336` — "the epoch bump and the old
watcher's take/abort must happen in one atomic critical section, and `epoch`
must be *this* call's own bumped value, not whatever `state.epoch` happens to
read moments later after an `.await`" — documents a race that has no single
home. `watch_disconnect` (`247–270`) is already shared between the two paths;
it is the only part that was ever factored out.

**Deletion test.** Delete `ConnectionScopedTask` and the epoch/watcher
bookkeeping plus the atomic-supersede reasoning scatter back across two
structs and four methods. Concentrating them into one module with a
two-method surface — where the race is unit-tested directly with fake tasks
instead of only through the live `zbus` server — is the win.

## Re-verification note (corrects the first pass)

The first-pass review argued a *third* connection-scoped subscription was
imminent and would make this a 3-copy problem. **It is not** —
`tartarus-dual-stage-keys/map.md` Q14 and `issues/01-staged-event-pipeline.md`
both confirm the dual-stage feature adds **no new D-Bus signal and no new
`GetState()` field**. The justification here is **six in-place copies of a
subtle abort-and-supersede race**, on its own merits. It stays a **Strong**
recommendation and is **schedule-independent** — nothing waits on it, and it
touches only `dbus/mod.rs` (disjoint from ticket 15's
`dispatch.rs`/`chord.rs`/`trigger.rs`).

## Not an IPC change

ADR-0004 (D-Bus for Daemon↔GUI IPC) fixes the wire protocol and the
interface surface. This ticket is **internal structure inside the D-Bus
layer** — no method signature, signal, or wire shape changes. The
`com.acheron.Daemon` interface is byte-identical before and after; ADR-0004
is not reopened.

## The module

`daemon/src/dbus/connection_scoped_task.rs`, beside `wire.rs`, declared `mod
connection_scoped_task;` — **private to `dbus`**, not `pub(crate)` (unlike
ticket 14's `config::binding`): no module outside `dbus` names it.

- `ConnectionScopedTask` wraps `Arc<Mutex<Inner>>` where `Inner { epoch: u64,
  watcher: Option<JoinHandle<()>> }`. Derives `Clone` (an `Arc` bump); the
  spawned task closes over a clone. The `Mutex`, the `u64`, and the
  `JoinHandle` are module-private — no caller ever names them.
- `new()` replaces the two hand-spelled `Arc::new(Mutex::new(SuppressionState
  { epoch: 0, watcher: None }))` / `…DepthStreamState…` in `Daemon::new`
  (`mod.rs:123–132`).
- `supersede(&self) -> ArmToken` — `{ lock; epoch += 1; if let Some(h) =
  watcher.take() { h.abort() } }`, returns `ArmToken { inner:
  Arc<Mutex<Inner>>, epoch }` (owned, `'static`, holds no lock — safe to
  carry across the injector `.await`). `#[must_use]`.
- `disarm(&self)` — `{ self.supersede(); }`; the drop is the point.
- `ArmToken::finish(self, connection, sender, pump, on_clear)` — no `.await`
  before the spawn (so the supersede→store window stays inherently atomic):
  `tokio::spawn` a wrapper that `select!`s `watch_disconnect(connection,
  sender)` against `pump`, then `{ lock; if epoch == self.epoch { watcher =
  None; drop(lock); on_clear().await } }`; back on the calling side, `{ lock;
  if epoch == self.epoch { watcher = Some(handle) } else { handle.abort() }
  }`.
- `watch_disconnect` (`mod.rs:247–270`) **moves in verbatim**, with its
  NameOwnerChanged / p2p doc comment — it is the lifetime-detection half of
  "a task scoped to a connection's lifetime". Its two current callers
  (`run_depth_stream:209`, `set_output_suppressed:356`) both become
  module-internal; no other reference exists.

### `pump` / `on_clear` — sentinels, not `Option`

The grilling settled these as "complementary — exactly one meaningful per
caller." Represent the empty side as a **no-op value**, not `Option<_>`: a
bare `None` needs an unnameable type annotation at the call site
(`None::<std::future::Pending<()>>`). So:

- `pump`: a future whose completion ends the task early. `run_depth_stream`'s
  loop never returns, so `start_depth_stream` passes it directly;
  `set_output_suppressed` passes `std::future::pending()` (the race reduces to
  `watch_disconnect` alone).
- `on_clear`: run iff still-current at teardown. `set_output_suppressed`
  passes the injector-off closure; `start_depth_stream` passes `|| async {}`
  (the module already nulls its own `watcher`; with `input` gone there is
  nothing else to clear).

Both-noop (`pending()` + `|| async {}`) is well-defined — a bare
disconnect watch with no cleanup — and is a legitimate future base case.

## `run_depth_stream` sheds three parameters

New signature: `run_depth_stream(connection: zbus::Connection, depth_rx:
watch::Receiver<HashMap<Input, u8>>, input: Input)`.

- **Drops `sender`** — the module owns `watch_disconnect` now.
- **Drops `state` + `epoch`** — the module owns teardown; the tail block
  (`mod.rs:224–232`) is deleted.
- **Drops the `select!` against `disconnect`** (`212–222`) — the loop becomes
  `loop { interval.tick().await; if let Some(d) = depth_rx.borrow().get(&input).copied() { emit } }`.
  The module races the whole loop against `watch_disconnect` and aborts it.
- **Keeps** `interval_at`-not-`interval` and its comment (`199–207`): a
  since-superseded arming still has a ~33ms window between `tokio::spawn` and
  `finish`'s abort, and `interval`'s immediate first tick would reliably emit
  one stray `DepthChanged` into it;  `interval_at` closes that.

## What moves, what stays

- **Into `connection_scoped_task.rs`:** `ConnectionScopedTask`, `ArmToken`,
  `Inner`, `watch_disconnect`, and the epoch-guard doc prose currently split
  between `SuppressionState`'s comment (`74–80`), `DepthStreamState`'s
  (`86–93`), and the `mod.rs:329–336` block.
- **Deleted:** `SuppressionState`, `DepthStreamState`, `DepthStreamState.input`
  and its three write sites, `run_depth_stream`'s disconnect `select!` arm and
  its tail cleanup, the free `watch_disconnect` in `mod.rs`.
- **Stays in `mod.rs`:** `run_depth_stream` (slimmer), the four D-Bus methods
  (bodies rewritten to the table above), `Daemon.depth_rx`, every doc comment
  on the *methods* (`set_output_suppressed:305–319`,
  `start_depth_stream:774–784`, `stop_depth_stream:830–835`) — the
  user-facing "last write wins / auto-clears on disconnect" contract is
  unchanged and stated where the method is.
- **`DaemonError`, `injector_gone`, `dispatch_gone`** — untouched.

## Landing in one pass

Create the module, move `watch_disconnect`, add `supersede`/`disarm`/`finish`,
delete the two structs, rewrite the four method bodies and
`run_depth_stream`, then the test sweep — one PR (ticket 03–15 precedent). A
half-migration with `set_output_suppressed` on the new module and the depth
methods still hand-rolled is harder to read than either end state.

## Behaviour-preservation protocol

Not the latency-critical input path, but a stuck watcher silently mutes the
whole physical device (`set_output_suppressed`) or leaks a signal pump
(`start_depth_stream`):

- **The store-if-current epoch check must compare against the value captured
  in `supersede`'s locked section**, carried through `ArmToken.epoch` —
  *not* a fresh `inner.lock().epoch` read inside `finish`. This is the exact
  property `mod.rs:329–336` spells out; the token exists to preserve it
  across `set_output_suppressed`'s injector `.await`.
- **`finish` must not `.await` before `tokio::spawn`** — the supersede
  (in `supersede`) and the store (in `finish`) are both lock-only, so with no
  await between them a concurrent full `supersede`+`finish` either lands
  entirely before (this token is stale, handle aborted) or entirely after
  (this handle stored, then superseded and aborted) — never interleaved.
- **Diff `watch_disconnect` byte-for-byte after the move** — the
  subscribe-before-check ordering (`255–260`) and the `is_bus()` / p2p split
  are load-bearing and untested in isolation today.
- **`cargo test -p acheron-daemon` fully green before any test moves** —
  especially `output_suppression_auto_clears_when_the_setting_client_disconnects`
  (`2570`) and `start_depth_stream_over_real_dbus_retargeting_replaces_the_previous_stream`
  (`2954`).
- **`/code-review` on both the Standards and Spec axes**, as tickets 05–15
  did. Concurrency code: the Spec axis must confirm the
  capture-epoch-before-await / check-after invariant survived.

## Tests: replace, don't layer

- **New `connection_scoped_task::tests`** — the primary surface, the race
  driven directly:
  - **supersede aborts the prior task** — `finish` a fake future that blocks
    on an `mpsc`; `supersede()` + `finish` a second; assert the first is
    aborted (a drop-guard sentinel fires / it never completes).
  - **stale token does not store** — take a token from `supersede()`, run a
    second `supersede()` to bump past it, `finish()` the stale token, assert
    its handle was aborted, not stored (the live watcher is still the
    second's / `None`).
  - **disconnect runs `on_clear` iff current** — a bare `UnixStream::pair`
    p2p `zbus::Connection` (no dispatch, no injector, no config file — far
    lighter than the `mod.rs:2570` harness); drop the client; assert
    `on_clear` fired. Then: `finish` arming A, `supersede` with B, drop A's
    connection, assert A's `on_clear` **never runs** — the stale-cleanup
    guard, which nothing exercises today.
  - **`pending()` pump + client drop** ends the task via `watch_disconnect`
    alone.
- **Kept as `dbus` integration** (each proves a distinct *wiring* path, not
  the race):
  - `output_suppression_auto_clears_when_the_setting_client_disconnects`
    (`2570`) — proves `set_output_suppressed` wires `finish` + the injector-
    off `on_clear` through a real client drop. **Keep as-is.**
  - `start_depth_stream_over_real_dbus_retargeting_replaces_the_previous_stream`
    (`2954`) — demoted to one wiring smoke that `start_depth_stream` calls
    `supersede`. **Keep.**
  - `start_depth_stream_over_real_dbus_pushes_depth_changed_for_the_requested_input`
    (`2927`), `stop_depth_stream_over_real_dbus_ends_further_signals`
    (`2975`), `start_depth_stream_over_real_dbus_rejects_a_non_grid_input`
    (`3001`), and the three injector-gating suppression tests (`2337`,
    `2415`, `2465`) — **untouched** (depth-semantics / signal-emission /
    input-validation / injector-gating, not the scoped-task protocol).
- **Net:** gain the fake-task epoch tests and the new "stale `on_clear`
  suppressed" coverage; **delete no integration test** — unlike ticket 14's
  matrix rows, each surviving `dbus` test proves a separate wiring path. The
  race's *primary* coverage moves to the unit module; the integration tests
  demote to smoke. Coverage strictly increases.

## Decisions from the grilling

- **No type parameter.** `DepthStreamState.input` — the only candidate for a
  per-subscription payload `S` — is write-only dead state and is deleted. The
  module is flat `{ epoch, watcher }`; the depth pump closes over its `input`
  as it already does. Deletion flagged as a deliberate behaviour-neutral
  cleanup. (Q1)
- **`watch_disconnect` moves into the module.** Detecting the connection's
  lifetime ending is the module's core mechanism, not an incidental input;
  `finish(connection, sender, …)` builds it internally rather than every
  caller constructing it first. (Q2)
- **No `CONTEXT.md` entry** — matching ticket 14 (Q10). An internal mechanism
  of the IPC layer, not domain vocabulary the GUI or spec docs need; the
  module doc-comment carries the explanation. Quick `/domain-modeling` inline
  check during the write; record the deliberate "no". (Q3)
- **The module owns `Arc<Mutex<Inner>>`.** `ConnectionScopedTask` is `Clone`,
  `Daemon` holds two by value, `supersede`/`disarm` take `&self`. The
  `Arc<Mutex<>>`, the `u64`, and the `Option<JoinHandle>` become invisible to
  callers. (Q4)
- **`daemon/src/dbus/connection_scoped_task.rs`**, `mod
  connection_scoped_task;` private (not `pub(crate)`), type
  `ConnectionScopedTask`. (Q6)
- **Two-phase token.** `supersede() -> ArmToken` (`#[must_use]`) does the
  locked bump+abort and captures the epoch; `ArmToken::finish(…)` spawns and
  stores-if-still-current against that captured epoch; dropping the token
  unfinished == disarmed; `disarm()` is the named convenience. This keeps the
  "capture epoch before `.await`, re-check after" invariant *inside* the
  module rather than trading it for the racing-clients behaviour shift a
  one-shot await-free `arm` + caller-side reordering would cause. (Q5)
- **`pump` / `on_clear` as no-op sentinels, not `Option<_>`** — same
  complementary-use semantics, but `std::future::pending()` / `|| async {}`
  avoid the unnameable `None::<_>` type annotation at the call site. A
  faithful refinement of the Q5 decision. (Q5)
- **No integration test deleted.** The race's primary coverage moves to
  `connection_scoped_task::tests` (fake channel-blocked tasks + a bare p2p
  pair); the two touching `dbus` tests demote to wiring smoke; the rest are
  untouched. (Q7)

## Facts dug from the code during the grilling (not asked of the user)

- **`DepthStreamState.input` has zero readers.** Written at `mod.rs:230`,
  `805`, `843`; never read — not in `GetState`/`wire::state_to_dict`, not in
  any test, not in `run_depth_stream` (which takes `input` as parameter
  `mod.rs:192`). Its struct doc frames it as introspectable state that
  nothing introspects.
- **`watch_disconnect` has exactly two callers**, both in `dbus/mod.rs`
  (`run_depth_stream:209`, `set_output_suppressed:356`), plus one doc mention
  (`180`). It moves cleanly.
- `dbus/` is already a directory module — `mod.rs` (3350 lines) + `wire.rs`
  (`pub mod wire;`). The new file is a sibling.
- **`set_output_suppressed` is the only site with an `.await` between
  supersede and store** — `injector.set_suppressed(suppressed).await`
  (`346–349`), fired for both `true` and `false`. The depth methods have no
  such middle step. This is *why* the store-if-current epoch dance exists;
  the two-phase token is the minimal shape that preserves it.
- **`run_depth_stream`'s tail** (`224–232`) is a verbatim structural copy of
  `set_output_suppressed`'s spawned-watcher disconnect-clear (`357–365`) —
  same `if state.epoch == epoch` guard, different fields nulled.
- The existing disconnect test (`output_suppression_auto_clears…`, `2570`)
  hand-rolls a `UnixStream::pair` p2p `zbus::Connection` and a full
  `dispatch::run` + `injector::spawn` just to drop one client mid-test — the
  module's own test can drop the client against a bare connection with no
  server object, far cheaper.
- `Daemon` also holds `commands: mpsc::Sender<Command>`, `injector:
  Injector`, `depth_rx: watch::Receiver<…>` — none of which this ticket
  touches; only the `suppression` and `depth_stream` fields change type.
- No test constructs two genuinely-simultaneous `SetOutputSuppressed(true)`
  and `(false)` calls; the "last write wins across clients" doc (`mod.rs:319`)
  is stated but unexercised. The two-phase token holds the current
  attribution (this call's own captured epoch) unchanged.

**Blocked by:** None — tickets 24 and 26 (the two features being refactored)
shipped in 1.0; ticket 15 (`trigger::Slots<K>`) touches disjoint files and
needs no ordering.

**Status:** resolved

- [x] `daemon/src/dbus/connection_scoped_task.rs` exists, `mod
      connection_scoped_task;` (private) in `dbus/mod.rs`; exposes
      `ConnectionScopedTask` (`new`, `supersede`, `disarm`) and `ArmToken`
      (`finish`) `pub(super)` and nothing else; `Inner`, `watch_disconnect`
      module-private.
- [x] `ConnectionScopedTask` wraps `Arc<Mutex<Inner>>`, derives `Clone`;
      `Inner { epoch: u64, watcher: Option<JoinHandle<()>> }`.
- [x] `supersede` does bump + abort-old in one locked section and returns an
      `ArmToken` carrying that epoch; `#[must_use]`. `disarm` is
      `{ let _ = self.supersede(); }` (`let _` to satisfy `#[must_use]`).
- [x] `ArmToken::finish` has no `.await` before `tokio::spawn`; the spawned
      task `select!`s `watch_disconnect` against `pump`; teardown clears the
      watcher slot and awaits `on_clear` iff `epoch == token.epoch`; the
      calling side stores the handle iff `epoch == token.epoch`, else aborts.
- [x] `watch_disconnect` moved (its `NameOwnerChanged`/p2p body verbatim; the
      one-line opening generalised from "sent a `SetOutputSuppressed(true)`
      call" to "armed the scoped task" per the Standards review); no
      `watch_disconnect` definition remains in `mod.rs` (two prose mentions in
      method doc comments stay, as the spec keeps those).
- [x] `SuppressionState`, `DepthStreamState`, and `DepthStreamState.input`
      (all three write sites) deleted; `Daemon.{suppression, depth_stream}`
      are `ConnectionScopedTask`; `Daemon::new` builds two
      `ConnectionScopedTask::new()`.
- [x] The four D-Bus method bodies rewritten to the `supersede`/`finish`/
      `disarm` table; method doc comments unchanged bar one dead
      `see DepthStreamState` reference dropped from `stop_depth_stream`.
- [x] `run_depth_stream` signature is `(zbus::Connection,
      watch::Receiver<HashMap<Input, u8>>, Input)`; no disconnect `select!`,
      no tail cleanup; `interval_at` + its comment kept.
- [x] New `connection_scoped_task::tests`: supersede-aborts-prior,
      stale-token-does-not-store, disconnect-runs-`on_clear`-iff-current
      (incl. the stale-arming `on_clear`-never-runs case), `pending()`-pump-
      ends-on-disconnect — all with fake channel-blocked pump futures / a bare
      p2p pair.
- [x] `dbus` integration tests: `2570` and `2954` kept intact (they already
      read as wiring smoke); no integration test deleted; the rest untouched.
- [x] `com.acheron.Daemon` interface unchanged (schema fixture test green);
      no `CONTEXT.md` change.
- [x] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, the
      full daemon suite (410) and the GUI suite (410) all green. `/code-review`
      Standards + Spec clean (findings are doc-density / stale-comment
      judgement calls; no correctness or spec-fidelity issue on either axis).

## Comments

**2026-09-04** — Filed from the second-pass architecture review
(`research/architecture-review-2026-09-03b.html`, candidate 2 of 7 — the
carried-forward first-pass #2). Design tree settled over four grilling
rounds; see "Decisions from the grilling". The first pass's "a third copy is
coming" argument was checked against `tartarus-dual-stage-keys` and does not
hold (no new signal, no new `GetState()` field) — the ticket rests on the
six existing in-place copies alone. Not yet implemented.

**2026-09-04** — Implemented. `daemon/src/dbus/connection_scoped_task.rs`
carries `ConnectionScopedTask` / `ArmToken` / `Inner` / `watch_disconnect`
and a 5-test `tests` module driving the race directly (fake channel-blocked
pump futures + bare `UnixStream::pair` p2p `zbus::Connection`s, no server
object). `dbus/mod.rs` loses both `{ epoch, watcher }` structs, the dead
`input` field and its writes, `run_depth_stream`'s disconnect `select!` +
tail cleanup, and the free `watch_disconnect`; the four methods are now
`supersede`/`finish`/`disarm` calls. Net −191/+466 lines across the two
files.

One deliberate deviation from the table: `set_output_suppressed(false)`
runs `disarm()` **before** the injector `.await`, not after. The spec's
prose calls the two order-independent; disarm-first reproduces the original
supersede-block-then-injector ordering exactly and avoids a concurrent
`(true)` call's freshly-stored watcher being aborted by this call's trailing
`disarm` (which would leave suppression stuck on with no disconnect guard).
Both review axes independently confirmed this is the correct
disambiguation.

**Deliberate "no" on `CONTEXT.md`** (spec asked this be recorded): the
scoped-task protocol is an internal mechanism of the IPC layer, not domain
vocabulary the GUI or spec docs consume. The module doc-comment carries the
full explanation; matches ticket 14's decision (Q10).
