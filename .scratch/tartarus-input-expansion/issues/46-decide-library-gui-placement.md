Type: grilling
Status: resolved

## Question

Where does the Stepper/Macro library GUI (settled in [Prototype the Stepper library and list-editing UX](./31-prototype-stepper-library-ux.md) as a two-panel, tab-switched screen) live within the real app's window chrome — as opposed to how it's *reached*, which was never decided, only the screen's own internal content?

Surfaced re-reading [Build the Stepper/Macro library UX for real](./41-task-build-stepper-macro-library-ux.md): its scope note claims "no open design questions remain on... the GUI," but ticket 31's prototype only ever ran as a standalone script (`python3 gui/prototype_31_stepper_macro_library_ux.py`) reacting to the two-panel screen in isolation — never mounted inside the real `device_overview.py` window, so nothing decided where it opens from or what it replaces/overlays.

Since a Macro/Stepper is now a global, Binding-independent library entity (ticket 15/03) rather than something owned by one Binding, the binding-picker popup/modal (`make_input_button`'s per-key `Gtk.Window`, per ticket 44) is the wrong home for it — that surface is Profile+Layer+Input-scoped, one level below where a reusable library entity conceptually sits. The existing left Profile sidebar (`build_profile_sidebar`, `device_overview.py`, 150px, permanent) is the closer precedent: also global/cross-Profile, also a flat list with create/rename/delete chrome.

Settle at least:
- Does the library replace the Profile sidebar's content via a tab switch (user's proposal — sidebar swaps between "Profiles" and "Library"), or live as a separate top-level surface (its own window, a main-window view swap, a menu/toolbar-launched dialog)?
- If it's a sidebar-tab swap: does switching away from "Profiles" hide Device Overview's Profile-dependent content (Action Table, grid) entirely, or does the library open as an overlay/second pane while Device Overview stays visible underneath?
- How is the library entry point discovered — a persistent tab always visible, or only reachable once relevant (e.g. from the Action dropdown's "Manage Macros…")?
- Does this change how a Binding *assigns* a library entry (today's `render_action_editor` Action dropdown picking Keypress/Macro/Stepper/Controller Button) — e.g. does picking "Macro"/"Stepper" there need a way to jump into the library, or do the two stay fully decoupled (assign existing entries only from the Binding editor; author/manage only from the library)?

## Comments

## Answer

Settled via a `/grilling` session, two questions, one round:

- **Placement family — a nav-rail, not a same-box content swap, a widening sidebar, or a separate window.** The left sidebar becomes a narrow permanent rail switching between top-level views; selecting a non-Profile view replaces the main content area rather than trying to squeeze the new content into the existing 150px `build_profile_sidebar` box (ruled out empirically: ticket 31's variant B prototype ran at 760×620, nowhere close to fitting in 150px).
- **Scope correction, surfaced by the user mid-round**: this isn't a Library-only question. The existing 280px Action Table sidebar (`table_sidebar` in `device_overview.py`) is already felt to be too narrow for its own contents, so the nav-rail is the natural place to fix that too — the restructuring covers three top-level surfaces (Profiles+Grid, Action Table, Library), not two.
- **The concrete rail shape does not resolve in conversation.** Icon-vs-label rail, which views (if any) coexist rather than fully replace each other, exact widths post-restructuring — genuine "how should it look" questions per this map's own prototype-vs-grilling test. Spun out to [Prototype the Device Overview nav-rail restructuring](./47-prototype-device-overview-nav-rail.md) rather than guessed at here.
- **Assignment↔authoring coupling — fully decoupled.** `binding_editor.py`'s Action dropdown, once Macro/Stepper become library-only (ticket 15/03), offers a plain dropdown of existing library entries only — no "create new" shortcut inline. Authoring only happens from the Library view's own create/rename/delete chrome (ticket 31). Holds regardless of which rail shape ticket 47 lands on.

[Build the Stepper/Macro library UX for real](./41-task-build-stepper-macro-library-ux.md)'s `Blocked by` moved from this ticket to [ticket 47](./47-prototype-device-overview-nav-rail.md) — the concrete placement it needs isn't settled until that prototype resolves.
