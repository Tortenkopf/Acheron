Type: grilling

## Question

Design **Chord** Bindings — an Action triggered when two-or-more physical Inputs are pressed simultaneously, distinct from any Binding those same Inputs carry individually. Settle at least:

- **Config representation**: how a Chord's set of physical Inputs is stored (e.g. a Binding keyed by a set of `Input`s alongside the existing per-`Input` maps) and how it composes with `Layer`/`Profile` scoping (is a Chord Base/Held-scoped the same way an ordinary Binding is?).
- **Simultaneity detection**: what counts as "simultaneous" — a timing window between the first and last Input going down, or exact same evdev event batch? What happens on near-miss timing?
- **Precedence with single-Input Bindings**: if `grid_r1c1` and `grid_r1c2` are each individually bound *and* a Chord exists on both together, what fires when both go down — does the Chord suppress the individual firings entirely, fire after them, or something else? What about a partial press (one goes down, then the other) versus release order?
- **Trigger-mode applicability**: do Fire-once/Hold-to-repeat/Toggle all apply to a Chord's Action the same way they do to an ordinary Binding?
- **Size limits**: is a Chord capped at 2 Inputs, or open-ended?
- **GUI**: how a user defines/"records" a Chord — press-and-hold the physical controls live, or a multi-select picker? (Consider `/prototype` if this needs a look-and-feel answer, not just a data-model one.)

Once resolved, add the full **Chord** entry to CONTEXT.md (currently just a reserved name — see the map's Notes).

**Correction from [Lock the v1.0 feature list](./08-decide-v1-feature-list.md)**: Synapse's "four virtual thumbstick-diagonal bindings" (Up-Right/Up-Left/Down-Left/Down-Right, each firing when two adjacent thumbstick-direction switches are pressed together) is not a separate feature to build — the thumbstick's four cardinal directions are already ordinary bindable Inputs (`CONTEXT.md:22`), so a diagonal is just an ordinary Chord over two of them. No new engineering; when this ticket resolves, add the thumbstick diagonals as a worked example/test case (and confirm the GUI affordance for defining a Chord makes this discoverable) rather than scoping it separately. Bonus: this also means simultaneous Keypress/mouse-button output for a diagonal is already achievable today via a Macro with near-zero delay between steps, independent of Chord landing at all.
