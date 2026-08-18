Type: grilling

## Question

Decide whether `GetState()`'s D-Bus surface should move off a positional tuple onto a keyed
dict (matching how `GetConfig()` already returns a dict), or stay positional with a
discipline to prevent the failure mode below from recurring.

Surfaced by [ticket 22](./22-task-build-analog-capture-source.md)'s code review, not acted on
there as a drive-by: the positional tuple already broke a real client once. When
[ticket 21](./21-task-apply-analog-data-model-to-code.md) added `capture_mode` as a fifth
tuple element, `gui/acheron_gui/app.py`'s `rebuild()` unpacked it positionally and raised an
uncaught `ValueError` on every refresh — caught only by that ticket's own code review, not by
the type system or a test that would fail the same way in CI. Every remaining
Binding-editor-facing ticket on this map (Chord, mouse-button output, Stepper, Profile Switch,
reusable Macro entities, plus [ticket 19](./19-prototype-trigger-point-ux-and-live-depth.md)'s
live-depth work) is a candidate to grow `GetState()` again the same way.

## Settle at least

- Keyed dict vs. positional tuple for `GetState()` — weigh against `GetConfig()`'s existing
  dict precedent and whatever `SetBinding`/the rest of the D-Bus surface already establishes
  as this codebase's convention.
- If staying positional: what discipline (a shared test, a doc comment, a changelog note)
  would have caught the ticket 21 regression before code review did, rather than after.
- Migration cost: does changing shape now mean touching every existing `GetState()` call site
  (`daemon_client.py`, `daemon_stub.py`, `app.py`, GUI tests, daemon-side D-Bus tests) in one
  ticket, or can it land incrementally.
- Whether this is worth doing before v1.0 given more growth is already expected, or an
  acceptable fast-follow given the required floor doesn't block on it.

## Answer

_(unresolved)_
