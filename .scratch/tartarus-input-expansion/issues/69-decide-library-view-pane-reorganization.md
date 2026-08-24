Type: grilling
Status: resolved

## Question

Reorganize the Library view's panes: today's `library_view.py` is a flat two-column layout (browse list | editor) for both Macros and Steppers, reached as its own full-replace destination off the Grid/Library switcher ([ticket 48](./48-task-build-device-overview-nav-rail.md)). The idea on the table — the browse list moves into the Profile-sidebar slot, and steps/items get their own dedicated column — was raised while scoping [Scroll the Library editor's lists](./61-task-scroll-library-editor-lists.md) and deliberately deferred there: the minimal scrolling fix already solved the reported "window grows past the screen" complaint, so this is a materially bigger restructuring, not a follow-on polish pass.

Graduated now, ahead of the rest of this map's frontier, because it's judged to matter for the tool's overall usability and look-and-feel and likely to inform other UX decisions still to come — worth settling before building further on top of today's layout.

Note this would reopen [ticket 48](./48-task-build-device-overview-nav-rail.md)'s settled "Profile sidebar stays exactly as it is, in both destinations" decision. Ticket 61's own deferral called that acceptable to revisit — Library is already a Profile-agnostic screen, so folding its browse list into that slot doesn't collide with the sidebar's actual Profile-switching job — but it was never decided into being.

Settle at least:
- Does the browse list (Macro/Stepper names, "+ New") really move into the Profile-sidebar's slot, replacing/coexisting with Profile chrome when the Library destination is selected — or somewhere else entirely? If it moves there, what does a viewer see when Library is selected: Profile list replaced outright, tab-switched within the same slot, or something else?
- Does "steps/items get their own dedicated column" mean a genuine three-column layout (browse | steps-or-items | editor-detail), or is the current editor pane simply split in two?
- Does this change anything about the Macro-vs-Stepper tab switch within Library, or only the browse-list/editor relationship?
- Is this a "how should it look" question needing a prototype (per this map's own prototype-vs-grilling test, same as ticket 46 spawning ticket 47), or can the shape be settled directly in conversation?
- Any knock-on effect on ticket 48's nav-rail/sidebar contract, and whether it needs its own follow-up build ticket once the shape is settled.

## Comments

## Answer

Settled via a `/grilling` session, two rounds, no prototype needed — this is a rearrangement of already-designed widgets (the `sidebar` CSS shape, `ScrolledWindow` max-height, a tab-button row) reusing established visual language, not a new "how should it look" question the way ticket 46/47's nav-rail restructuring was.

The Library destination moves from today's two visual columns (Profile sidebar 150px, unchanged across destinations, plus Library's own separate 220px browse-list-and-editor pane) to a real three-column layout that **reuses** the Profile-sidebar's slot rather than adding a fourth:

1. **Column 1 — the former Profile-sidebar slot, now destination-dependent.** Grid destination: unchanged, shows the "Profiles" list exactly as today. Library destination: **full swap** — shows the Steppers/Macros tab row (moved here from its old place above the whole content area) atop the relevant browse list and "+ New" button, no separate "Profiles" heading. Pinned at a **fixed 220px in both destinations** (widened from today's 150px) so nothing visibly resizes when flipping Grid↔Library — same discipline as ticket 47/48's `set_hexpand(False)` fix, extended to the width itself. Profile switching is unreachable while Library is showing — accepted directly: Macros/Steppers are Profile-agnostic entities, and no ticket has surfaced a need to switch Profile mid-edit.
2. **Column 2 — new.** The selected Macro/Stepper's own name heading plus its steps/items list (unchanged `ScrolledWindow`/240px-cap treatment), now with the full vertical space column 1 used to claim.
3. **Column 3 — the remainder of today's `editor_box`.** "Changes save automatically" hint, toast/error label, the add-new-item controls, and (Steppers only) the Forward/Backward assignment row — everything that isn't the name or the list itself.

**Supersedes [ticket 48](./48-task-build-device-overview-nav-rail.md)'s settled "Profile sidebar stays exactly as it is, in both destinations"** — that held until this ticket, and no longer does for the Library destination specifically; Grid destination is unaffected.

Spawned [Build the Library view's three-column pane reorganization](./70-task-build-library-pane-reorganization.md) as a direct `task` ticket (no prototype), blocked by nothing.
