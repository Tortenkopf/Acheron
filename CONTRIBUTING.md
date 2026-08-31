<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# Contributing to Acheron

Acheron is a personal project released in the hope it's useful to other
Tartarus Pro owners on Linux. Bug reports, fixes, and hardware-compatibility
notes are welcome. Larger features are best raised as an issue first — the
scope is deliberately narrow (see the README's "What it is not").

## Repository layout

| Path | What |
|---|---|
| `daemon/` | the Rust Daemon — evdev capture, dispatch, `uinput` injection, D-Bus server |
| `daemon/src/capture/` | capture sources: `evdev_source`, analog `hidraw`, the `supervisor` that swaps between them, and a `fake` for tests |
| `gui/acheron_gui/` | the Python + GTK 4 GUI package |
| `gui/main.py` | run the GUI from a checkout (`python3 gui/main.py`) |
| `gui/tests/` | GUI test suite (real GTK widgets, no main loop) |
| `packaging/` | systemd unit, udev rule, `.desktop` entry, icons, launcher, `test_install.sh` |
| `install.sh` | the only install path |
| `CONTEXT.md` | the domain glossary — **authoritative vocabulary** |
| `docs/adr/` | architecture decision records |

Development happens on the **`dev`** branch; **`main`** is a clean, release-only
branch rebuilt from `dev` at each tagged release. Send PRs against `dev`.

## Design record

Acheron was built ticket by ticket, and the full record lives on the **`dev`**
branch (kept off `main` so a casual checkout stays lean):

- `.scratch/tartarus-keybinder/` — the original MVP: a `spec.md` plus its issues.
- `.scratch/tartarus-input-expansion/` — the road to v1.0 as a `map.md` (the
  index) with one file per decision under `issues/`.
- `prototype/` (3 base spikes) plus the `prototype/*` branches — throwaway
  UI/protocol spikes; `prototype/NN-…` paths in code comments refer to the
  matching branch (`git show prototype/NN-slug:<path>`).
- `CLAUDE.md`, `docs/agents/` — the agent workflow the project was built with.

If you want to know *why* something is the way it is, the relevant ticket's
"Answer" on `dev` almost certainly says. `CONTEXT.md` and `docs/adr/` (on both
branches) are the distilled version.

Use the vocabulary `CONTEXT.md` defines (Profile, Layer, Input, Binding,
Action, Chord, Stepper, Trigger mode, Daemon, GUI, …) and avoid the synonyms it
rules out. If a change contradicts an ADR, say so in the PR rather than
silently overriding it.

## Building

See the README's **Install** and **System requirements** sections. For
development you usually want:

```sh
# Daemon
cargo build --manifest-path daemon/Cargo.toml
cargo run   --manifest-path daemon/Cargo.toml      # runs against your real device

# GUI from a checkout
python3 gui/main.py
```

The GUI can run without the Daemon against a built-in stub — useful for
widget work — but binding changes then go nowhere.

## Tests

Everything below must be green before a PR.

### Daemon (Rust)

```sh
cargo test   --manifest-path daemon/Cargo.toml
cargo fmt    --manifest-path daemon/Cargo.toml --check
cargo clippy --manifest-path daemon/Cargo.toml --all-targets -- -D warnings
```

The Daemon's tests drive the real dispatch pipeline through a `fake`
`CaptureSource` and assert on injected `uinput` writes and emitted D-Bus
signals — never on private fields. New behavior needs a test at that seam.

### GUI (Python)

The GUI tests use real GTK 4 widgets, so they need PyGObject/GTK 4 available.
Create a venv that can see the system bindings, add `pytest`, and run:

```sh
python3 -m venv --system-site-packages gui/.venv
gui/.venv/bin/pip install pytest
gui/.venv/bin/pytest gui/tests
```

They construct the actual widget tree and emit signals synchronously (no main
loop); assert on rendered widget state and on the D-Bus calls made, matching
the existing style in `gui/tests/`.

### Packaging

```sh
bash packaging/test_install.sh
```

Checks `install.sh` idempotency and the unit/udev/desktop content with `cargo`,
`systemctl`, and `sudo` stubbed — it never touches your real system.

## Hardware verification

Acheron's whole point is driving real hardware, so anything that touches
capture, injection, timing, or the GUI's device-facing behavior is verified
against a **physically connected Tartarus Pro** before it's considered done —
the automated suites can't see a dead grid key or a mis-tuned repeat cadence.
`daemon/examples/analog_probe.rs` and `daemon/examples/device_info_probe.rs`
are throwaway tools for that. If you can't test on hardware, say so in the PR
and it can be verified before merge.

## Conventions

- **Licence headers.** Every source file starts with:

  ```
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright © <year> <your name>
  ```

  (`//` for Rust, `#` for Python/shell, after any shebang). Keep it on new
  files. By contributing you agree your changes are licensed GPL-3.0-or-later.

- Match the surrounding code — its naming, comment density, and idioms. The
  existing files are the style guide.

- **Adding a new mutating `Command`** (ticket 05). Every mutating `Command`
  flows wire variant → `Edit` variant → `edit::plan` arm → a mechanical
  translation line in `dispatch::handle_command` → an `Effect` variant *only
  if* there is a post-commit side effect. Concretely:

  1. Add the `Command` variant in `command.rs` (with its `reply` sender).
  2. Add the matching data-only `edit::Edit` variant — the same fields minus
     `reply`.
  3. Add an `edit::plan` arm. It mutates the `Config` clone, returns an early
     `Err(CommandError::…)` for every **operation precondition** (things only
     meaningful relative to the *requested operation*, not the resulting
     `Config`: `NotFound` / `AlreadyExists`, "can't delete the active
     Profile", "can't delete a still-referenced Macro/Stepper", a blank
     create/rename name — the slug, not the name, is the key, so a blank name
     is a bad request, not a corrupt `Config`), and pushes any `Effect`s. It
     never checks a **structural invariant of the resulting `Config`** — that
     is `config::validate`'s job (see below), run once at the end of `plan`.
  4. Add the one-line arm in `dispatch::handle_command`: translate the
     `Command` to its `Edit`, `edit::apply` it, `reply.send` **before**
     running effects, then `run_effects`.
  5. If the command has a runtime side effect that isn't a `Config` write
     (republishing actuation, recomputing axes, signalling the supervisor,
     stopping a Toggle, dropping a stepper cursor, emitting a signal…), add an
     `edit::Effect` variant and handle it in `dispatch::run_effects`.

  A **structural invariant of a stored `Config`** — anything that could be
  written to `config.toml` and reloaded — goes in `config::validate` and
  nowhere else. It is the single enforcement point both `config::parse`
  (startup) and `edit::plan` (every live D-Bus edit) run through, so a new
  arm never needs its own copy.

- Keep `install.sh` idempotent and safe to re-run.

## Licence

By submitting a contribution you certify you wrote it (or have the right to
submit it) and license it under the GPL-3.0-or-later, the same terms as the
rest of Acheron.
