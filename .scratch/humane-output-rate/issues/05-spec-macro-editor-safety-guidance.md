# Spec the user-facing output-safety guidance

Type: grilling
Status: open
Blocked by: 01, 06, 07
<!-- 02 (anti-cheat research) resolved 2026-09-06 — see research/anticheat-input-timing-heuristics.md -->
<!-- Broadened by ticket 01 (2026-09-06): also holds the Analog-repeat selection toast, and
     waits on the fix (06) + the value=2 spec (07) so the tips describe post-change reality.
     Sequencing per Charon: audit → required changes → then the disclaimer/tip text. -->
<!-- 03 (self-DoS macro guardrail) resolved 2026-09-06: the guardrail is TEXT ONLY — no
     editor warning widget, no config::validate check. The within-a-single-run keystroke
     burst and a Macro fired once are deliberately unrestricted; the tips must describe that
     honestly (this is the footgun the built-in modes protect you from and a hand-written
     Macro does not). The Analog-repeat + Macro combo is now rejected at the config layer
     (ticket 09), so the tips need not warn about it — it is structurally impossible. Drop
     the "whether the editor shows any of ticket 03's warning inline" open item: it doesn't. -->
Parent: [Humane output rate](../map.md)

## Question

Produce a gated `.scratch/humane-output-rate/spec.md` for the **user-facing output-safety
guidance** — the macro-editor disclaimer + best-practice tips **and** the Analog-repeat
selection toast — ready to hand to a fresh implementation effort. It must cover:

### Analog-repeat selection toast (ticket 01, Q3)

A one-time toast/notice shown when Analog-repeat is chosen as a Binding's Trigger mode
(trigger-mode selector in the binding editor, *not* the macro editor): Analog-repeat can
produce inhumanly fast keypresses as Depth approaches full travel and the output ramps into
kernel autorepeat; use it only in single-player / known-safe games, never for competitive
multiplayer. Exact wording, trigger (first selection only? every selection? a "don't show
again"?), and widget (toast vs inline hint by the selector — follow `_CONTROLLER_MACRO_HINT`)
in the spec. Note that ticket 07 makes the top of the ramp *look like* genuine autorepeat —
the toast is about the real rate, not the shape.

### Disclaimer copy

Short, always-visible line in the macro editor (`gui/acheron_gui/library_view.py`, Macro
tab) — the honest framing: `uinput`-generated input is always identifiable as synthetic;
Acheron keeps its non-macro output within physically-plausible rates, but a Macro does
exactly what you write, so writing one is on you. Exact wording in the spec.

### Best-practice tips (the "Learn more" / expander content + README section)

Two themes, grounded in ticket 01 (why non-macro modes are safe) and ticket 02 (anti-cheat
timing heuristics):

1. **Plausible-to-a-game macros** — realistic delays between steps, avoid perfectly
   regular timing, don't exceed human rates, be aware repeated identical sequences are a
   tell, prefer the built-in Trigger modes (which stay within the bar) over a macro when a
   macro isn't needed.
2. **Don't lock down / impede your own system** — the concrete footguns:
   - zero / tiny delays in a Macro under Toggle or Hold-to-repeat (ticket 26 story;
     whatever guardrail ticket 03 added, and what it does *not* catch)
   - holding a modifier (Ctrl/Alt/Super) or a key `KeyDown` with no matching `KeyUp`
   - a Macro that never returns / very long with no delays
   - Macros that spam Profile/Layer switches or fight the focused app
   - recovery: how to stop a runaway (GUI focus stops all Toggles; kill the daemon;
     `systemctl --user stop`), and that Output suppression / `StopAllToggles` exist

### Placement / behaviour decisions the spec must lock

- disclaimer line vs. expander vs. dialog — and whether the tips are a GtkExpander in the
  editor, a separate help dialog, or both
- the README home for the long-form version (new section? under an existing one?)
- whether the editor shows any of ticket 03's warning inline, and its copy
- follow the existing `_CONTROLLER_MACRO_HINT` pattern for hint styling/placement

Invoke `/grilling`. The spec is the deliverable — implementation (the GtkExpander, the
README edit, any warning widget) is a fresh effort, per the map's Notes.

> **Blocked by 06 and 07** (wired 2026-09-06) — ticket 01's audit graduated exactly one
> inline fix (06, the pace-loop clamp) and one behaviour-change spec (07, `value=2`). The
> user's sequencing is audit → required changes → *then* the disclaimer/tip text, so the
> tips can describe what the built-in modes actually do post-change.
