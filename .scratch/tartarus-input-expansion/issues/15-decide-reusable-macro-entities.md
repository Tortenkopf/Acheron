Type: grilling

## Question

Design **reusable, named Macro entities**, replacing today's inline-only model where a Macro is a hand-specified Keypress sequence defined directly inside one Binding (`CONTEXT.md:37-38`). Surfaced by the Synapse catalog ([Catalog Synapse's remap/macro feature set](./07-task-catalog-synapse-feature-set.md)): Synapse maintains a separate macro library — a Macro is defined once, then assigned/reassigned to any physical key without rebuilding it — and [Lock the v1.0 feature list](./08-decide-v1-feature-list.md) locked this in scope for v1.0 as a genuine usability gap worth closing.

Settle at least:

- **Storage shape**: does a Macro become a named top-level entity in `config.toml` (e.g. a `[macros.<name>]` table), referenced by id/name from a Binding, alongside (not replacing) the option to still define one inline? Or does every Macro become a library entry, even single-use ones?
- **Identity and editing**: renaming a named Macro, and what happens to Bindings referencing it; deleting a Macro that's still referenced by one or more Bindings.
- **Scoping**: is a Macro library global (shared across all Profiles/Layers) or per-Profile? The Synapse behavior being copied is "swap a macro from one key to another" — decide whether that swap is meant to work across Profile boundaries too.
- **GUI**: a macro-library picker/manager (list existing macros, create/edit/delete, assign to a Binding) — likely a "how should it look/behave" question, consider `/prototype`.
- **Interaction with in-flight tickets**: confirm this doesn't collide with [Design Chord Bindings](./01-decide-chord-bindings.md) or [Design Profile Switch](./05-decide-profile-switch-action.md) — a named Macro should compose as a drop-in replacement for today's inline Macro wherever `Action::Macro` is used, without needing changes to either.

Once resolved, update CONTEXT.md's **Macro** entry to describe the named/reusable shape (currently describes only the inline form).

## Comments

Note added by [Design the Stepper list-stepping construct](./03-decide-stepper-list-stepping.md)'s resolution: Stepper landed on the same "named entity, defined once in a library, reassignable to a Binding" shape (global library, silent reassignment, no per-entry scoping). [Prototype the Stepper library and list-editing UX](./31-prototype-stepper-library-ux.md) is deliberately blocked on this ticket — when resolving here, check whether its GUI prototype should also cover Stepper's library picker, or whether they genuinely diverge enough (item-reordering, pair-assignment vs. single-Binding assignment) to stay separate.
