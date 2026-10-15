# Acheron — GUI memory leak across rebuilds

Status: ready-for-agent

Diagnosed 2026-10-14 against `dev` @ 1.3.4. The measurements below came from a soak script that
drives the real rebuild path (`build_status_wrapped_view` into one persistent box) against
`DaemonStub`'s config.

## Problem Statement

If the GUI stays open for a few hours, its memory use climbs without limit. Minimized to the tray
while a game runs, it can grow until the whole system stalls. The user did nothing in the GUI
beyond leaving it open. The growth is driven by Daemon signals that arrive while they play,
chiefly the Mode key: every press *and* every release of the Mode key emits
`ActiveLayerChanged`, and each one makes the GUI tear down and rebuild its whole Device Overview.

Measured cost: about **44 MB retained per rebuild, never freed**, so every Mode-key tap retains
about 88 MB. 300 rebuilds took the process from 106 MB to 13.2 GB RSS, and the number of live
widget wrappers grew linearly (590 → 167,990).

### Diagnosis

Three compounding causes:

1. **Every rebuild pre-builds 28 hidden Binding-editor top-level windows.** Each Input button on
   the Device Overview eagerly constructs a modal `Gtk.Window` (hide-on-close) that wraps a
   complete Binding editor, including the inline key/mouse-button picker. That is about 11,700
   widgets per rebuild, built before anything is clicked. GTK4 keeps every top-level window in a
   global list until `gtk_window_destroy()`. Nothing ever destroys these windows, and removing
   the button from the tree doesn't touch its window. The top-level count went 28 → 56 → 112
   across rebuilds. Each stale window has a C refcount of 2 (GTK's list plus its Python wrapper),
   so Python can never collect it, or anything its closures reach. The Lighting tab's colour
   picker button uses the same pre-built hide-on-close window pattern.
2. **Even destroyed, a Binding editor leaks through reference cycles that cross C-side
   ownership.** Python signal handlers on widgets and event controllers inside the editor capture
   their own ancestors. For example, the Depth bar's drag gesture handlers are bound to the bar
   itself, and the Save path's close handler captures the window that contains the Save button.
   GTK's parent→child ownership, and closures held by GObjects that no longer have a live Python
   wrapper, are invisible to Python's cycle collector, so it can't prove these cycles are
   garbage. Calling `destroy()` and `set_child(None)` on every stale window barely changed the
   growth rate. Building one editor standalone and dropping every reference to it still retained
   about 1.4 MB each time (about 21 wrappers survive: Boxes, Buttons, Labels, two DropDowns, one
   Depth bar), and the loss grew linearly.
3. **Rebuilds run regardless of window visibility.** The app-level rebuild runs on Layer change,
   Profile switch, device connect/disconnect, Capture-mode change, Daemon up/down, and committed
   edits, even while the window is hidden to the tray. Nobody can see the result, yet each one
   still pays (and leaks) the full cost.

The Device Overview's own main tree is *almost* freed correctly per rebuild. A handful of
wrappers survive per rebuild (two Popovers, two Entries, two Labels, the shared name-prompt
popover shape), so the same cycle pattern exists there at a small scale.

## Solution

Leaving the GUI open, visible or hidden in the tray, for any length of time and through any
number of Mode-key presses, Profile switches and edits, keeps its memory use flat. Concretely:

- A Binding editor window (and a Lighting colour picker window) exists only while the user has it
  open. Clicking an Input builds it, and closing it destroys it.
- A destroyed editor actually frees its memory: no editor-internal reference cycle outlives it.
- While the main window is hidden, Daemon signals only mark the view stale. The rebuild happens
  once, when the window is next shown.
- A soak regression test pins all of this, so a future change that reintroduces a pinned
  top-level window or an uncollectable cycle fails CI.

Behaviour the user can see stays the same, apart from editor opening becoming "built on click"
rather than "pre-built".

## User Stories

1. As a gamer, I want the GUI's memory use to stay flat while it sits in the tray during a long
   session, so that my system never stalls mid-game.
2. As a gamer who holds the Mode key constantly, I want Layer switches to cost the GUI nothing
   while it's hidden, so that tapping the Mode key thousands of times per session is free.
3. As a user, I want the GUI to show the correct, current Layer, Profile and device status the
   moment I reopen it from the tray, so that skipping rebuilds while hidden never shows me stale
   state.
4. As a user, I want clicking an Input on Device Overview to open its Binding editor exactly as it
   does today (same title, same content, same position near the main window), so that the fix is
   invisible to me.
5. As a user, I want closing a Binding editor (Save, Cancel, Escape, or the window's close
   button) to release it entirely, so that opening and closing editors many times doesn't grow
   memory.
6. As a user, I want Save and Apply in a Binding editor to still drive exactly one Device
   Overview refresh when the editor closes after a commit, so that the committed-edit refresh
   behaviour is unchanged.
7. As a user, I want a pure open-then-close of a Binding editor (nothing committed) to still skip
   the Device Overview refresh, so that browsing bindings stays cheap.
8. As a user, I want the editor for an Input to always reflect the latest config when I open it,
   so that an Actuation default changed from another key's editor shows up immediately.
9. As a user, I want the Actuation & release live depth bar to keep streaming while its editor is
   open and stop when it closes, so that the depth stream has the same lifecycle as now.
10. As a user, I want the Lighting tab's colour pickers to open and commit colours exactly as
    today, so that the lazy-build change is invisible there too.
11. As a user, I want "Select Chord members" mode to keep working on the same grid buttons, so
    that the override click path isn't broken by lazy editor construction.
12. As a user, I want a Daemon outage, device disconnect or Capture-mode change that happens while
    the window is hidden to be reflected correctly (status chip, dimmed overlay, mode badge) when
    I reopen it, so that deferral never hides a real problem.
13. As a user, I want the tray icon's menu (Profile list, status dot) to keep updating live while
    the window is hidden, so that the tray stays accurate even though the main view rebuild is
    deferred.
14. As a user, I want the Profile sidebar's rename/new-name prompts to free themselves on rebuild,
    so that the small residual per-rebuild leak in the main tree is gone too.
15. As a maintainer, I want a soak test that rebuilds Device Overview many times and fails if
    top-level windows or surviving widget wrappers grow, so that this class of leak can't silently
    come back.
16. As a maintainer, I want a soak test that opens and closes a Binding editor many times and
    fails if anything survives, so that editor-internal cycles are pinned too.
17. As a maintainer, I want the existing Device Overview / Binding editor tests to reach an
    editor's content by clicking its button, so that tests exercise the same path a user does.
18. As a maintainer, I want the hidden-window rebuild deferral to be testable with a fake window
    (the same duck-typed shape `_wire_focus_tracking` uses), so that it needs no real windowing
    system.
19. As a maintainer, I want the rule "a widget's own signal handler must not keep its ancestors
    alive" written down where the GUI's widget-building conventions live, so that new widgets
    don't reintroduce the cycle.
20. As a gamer, I want the GUI's CPU cost while hidden to drop too (no 28-editor construction per
    Mode-key tap), so that the background GUI doesn't steal frame time.

## Implementation Decisions

- **Lazy Binding-editor windows.** Building a Device Overview Input button no longer constructs
  its editor window. The button's click handler builds the window, with the Binding editor read
  from the config snapshot the button was built from, and presents it. The "Select Chord members"
  override is checked first, unchanged. The window is no longer hide-on-close; closing it destroys
  it. A rebuild while an editor is open must also destroy that editor (today a rebuild already
  replaces its content, and a full rebuild already closes popovers by design), so no top-level
  window can outlive the Device Overview tree that spawned it.
- **Commit-then-close flow preserved.** The "committed" flag still lives with the window
  (per-editing-session), and its close-request still drives exactly one `on_change()` when Save
  or Apply committed something, and none on a pure open/close. Save's own close path keeps calling
  the deferred rebuild directly, for the unrealized-window case it covers today.
- **The ticket-44 rationale moves.** The "built once, shown/hidden via present()/close()"
  reasoning (and its hide-on-close warning about re-presenting a destroyed window) is replaced:
  every open builds a fresh window, so no destroyed window is ever re-presented. Reopen is no
  longer pre-built-instant. That is an accepted tradeoff, since one editor is cheap to build and
  this removes about 11,700 widgets of eager construction per rebuild.
- **Same treatment for the Lighting colour picker.** It is built on click and destroyed on close.
  Cancel/Select keep their current commit semantics.
- **Test-facing hook.** The button no longer carries a pre-built `binding_editor_window` /
  `colour_picker_window`. Tests reach editor content by activating the button and reading the
  window it currently has open, which the button exposes (`None` when no editor is open). The
  widget-tree helpers tests use are updated accordingly.
- **Break editor-internal cycles.** No signal handler connected on a widget or event controller
  inside a built subtree may strongly capture that subtree's own ancestors (including itself via
  a bound method on a controller it owns). Accepted techniques: capture via a weak reference
  resolved at call time, read the target from the signal's emitter argument (`get_root()`,
  `get_ancestor()`), or disconnect/teardown on `unrealize`/destroy. Known instances: the Depth
  bar's drag-gesture handlers and its map/unmap handlers, the editor window's Save/close-request
  closures, the editor's DropDown `notify::selected` handlers, and the name-prompt popover. The
  soak test (below) is the arbiter: fix whatever it shows surviving, not just this list.
- **Defer rebuilds while hidden.** The app-level rebuild gains a visibility gate, a new small
  wiring function in the `_wire_*` style, taking the duck-typed window plus the real rebuild.
  While the window isn't visible, every rebuild trigger (Layer, Profile, status, Capture mode)
  still updates its state (`ui_state["selected_layer"]`, the status dict) but only marks the view
  dirty. When the window becomes visible (`notify::visible`, which also covers present-from-tray
  and second-launch activation), one rebuild runs if dirty. The tray icon's `update()` keeps
  running on every trigger regardless of visibility, so the tray stays live; it is cheap and isn't
  part of the leak.
- **No change to the Daemon or the D-Bus surface.** `ActiveLayerChanged` keeps firing on every
  Mode-key transition. The fix is entirely GUI-side.
- **No in-place Layer update.** Updating Device Overview in place on a Layer change, instead of a
  full rebuild, is not part of this spec (see Out of Scope). With editors lazy and cycles broken,
  a visible-window rebuild costs about 128 widgets and leaks nothing.

## Testing Decisions

- **What makes a good test here:** assert observable resource behaviour, not mechanism. Check
  the top-level window count (`Gtk.Window.get_toplevels()`), whether widgets from an old build are
  still alive (weak references to widgets walked from that build's tree, after `gc.collect()`),
  and the user-visible flows (click opens an editor with the right title/content, Save closes it
  and refreshes once). Don't assert which technique broke a given cycle.
- **Rebuild soak test (new seam, the one that matters).** Drive `build_status_wrapped_view` into
  one persistent box against `DaemonStub`, rebuilding N times, with the same `rebuild` closure
  shape the app uses (clear children, rebuild, pass `rebuild` as `on_change`), and pump the main
  context between rebuilds. Assert: top-level window count is 0 after any number of rebuilds when
  no editor is open; no widget from build 1 is alive after build N; the live widget-wrapper count
  doesn't grow between build K and build N. Cover both the Grid and Library destinations and the
  Lighting tab, since `ui_state["dest"]` selects which tree gets built.
- **Editor open/close soak test.** Through the existing Input-button seam: click an Input
  (including a grid key, so the Depth bar is built), close the editor, repeat N times. Assert no
  top-level window remains and nothing from the closed editors is alive. Repeat with a
  Save-commit close, and with a rebuild while an editor is open.
- **Existing tests migrate** from `btn.binding_editor_window` / `btn.colour_picker_window`
  (8 call sites, mostly through the widget-tree helpers) to click-then-read-open-window. Their
  assertions stay as they are.
- **Visibility gate** is tested with a fake window (`get_visible()` + `connect()`), exactly like
  the existing `_wire_focus_tracking` / `_wire_window_close_to_hide` tests: triggers while hidden
  call the rebuild zero times but still update state; becoming visible calls it once; becoming
  visible when nothing changed calls it zero times; triggers while visible rebuild immediately as
  today.
- **Prior art:** `gui/tests/test_app.py` (fake-window `_wire_*` tests, `_pump_idle_callbacks`),
  `gui/tests/test_device_overview.py` (real widgets against `DaemonStub`, `widget_tree` helpers),
  and `gui/tests/test_binding_editor.py` (the close-request/commit test).
- Soak N should be small enough to keep the suite fast (tens, not hundreds). Linear growth shows
  up immediately (about 558 wrappers per rebuild today), so a flat-count assertion at N ≈ 10–20 is
  decisive.

## Out of Scope

- Updating Device Overview in place on a Layer change instead of rebuilding. That would be a
  worthwhile follow-up for visible-window responsiveness, but it isn't needed to stop the leak.
- Any Daemon-side change (signal rate, coalescing, or suppressing `ActiveLayerChanged`).
- A general refactor of the rebuild-from-scratch architecture.
- The About dialog, licence window and Chord-binding dialog. They are already built on demand
  and destroyed by the default close behaviour (worth confirming with the editor soak test's
  pattern, but not expected to need changes).
- `DaemonStub`'s own `calls` log growth. That is test-only and never runs in production.

## Further Notes

- The diagnostic scripts (rebuild soak, standalone editor soak, top-level count, survivor split
  between main tree and editor windows) were throwaway and live only in the diagnosing session's
  scratchpad. The two soak tests above are their permanent form.
- Expected result after the fix: the 300-rebuild soak that previously reached 13.2 GB RSS stays
  near its starting footprint, and the top-level window count stays at 0 when no editor is open.
- Related prior decisions to respect: ticket 44 (top-level window instead of a Popover, which
  stays a real window, just built lazily), tartarus-dual-stage-keys ticket 10 (Apply commits
  without closing; one rebuild per committed editing session), ticket 27 (`on_saved` forcing a
  rebuild so freshly opened editors read fresh data, which lazy construction satisfies naturally),
  and ticket 36 (minimize-to-tray via hide, which is why the visibility gate keys off
  `notify::visible`).
