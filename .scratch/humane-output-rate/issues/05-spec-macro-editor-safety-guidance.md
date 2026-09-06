# Spec the user-facing output-safety guidance

Type: grilling
Status: resolved
Blocked by: 01, 06, 07
<!-- 02 (anti-cheat research) resolved 2026-09-06 — see docs/anti-cheat-input-heuristics.md
     (relocated from research/anticheat-input-timing-heuristics.md when 05 resolved) -->
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

Produce a gated `.scratch/humane-output-rate/spec-user-facing-output-safety-guidance.md` for the **user-facing output-safety
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

## Answer

Grilled + ratified with Charon in a `/grilling` + `/domain-modeling` session
(2026-09-06). Deliverable: gated [`spec-user-facing-output-safety-guidance.md`](../spec-user-facing-output-safety-guidance.md).

### Decisions

1. **Two GUI hints, not a toast.** Both the standing macro-editor disclaimer and the
   Analog-repeat notice are **GUI hints** — persistent `["dim"]` labels in the
   `_CONTROLLER_MACRO_HINT` style, shown while their condition holds. This supersedes
   ticket 01 Q3's "one-time toast": the "single-player only" warning is worth repeating
   every time Analog-repeat is the live Trigger-mode choice, and a persistent hint needs
   no seen-flag or dismiss control. **New CONTEXT.md `### Interface` terms** —
   **Toast label** (the one-shot ticket-60/68 pattern) vs **GUI hint** (persistent,
   condition-bound) — written this session to fix the vocabulary.

2. **Disclaimer** — one always-visible line at the top of the Macro editor (Macro tab
   only; not the Stepper tab, not `binding_editor.py`), leading with a `⚠️` emoji (a
   deliberate exception to the no-emoji norm). Exact copy in `spec-user-facing-output-safety-guidance.md` §2:
   *"⚠️ uinput input is always identifiable as synthetic; Acheron keeps its own Trigger
   modes within physically-plausible rates, but a Macro does exactly what you write. Use
   macros with caution!"*

3. **Tips** — a single `Gtk.Expander` ("About macro safety", collapsed by default) in
   the Macro editor, two themes (plausible-to-a-game / don't-lock-up-your-system) plus a
   recovery list (focus the window, tray pause, `systemctl --user stop`) and a link to
   the README. No separate help dialog. Compact copy in `spec-user-facing-output-safety-guidance.md` §3.

4. **README `## Output safety`** — a new top-level section between Usage and
   Troubleshooting (not a Usage subsection, not Troubleshooting), covering the ceiling
   principle for users, the two tip themes at length, recovery, a friendly restatement
   of the no-warranty term ("any trouble you get yourself into … is entirely your own
   responsibility"), and a link to the research doc. Quotes **no** threshold numbers —
   makes explicit that risk depends on the game. Full prose in `spec-user-facing-output-safety-guidance.md` §5.

5. **Research doc relocated this session** —
   `.scratch/humane-output-rate/research/anticheat-input-timing-heuristics.md` →
   `docs/anti-cheat-input-heuristics.md` (`git mv` on `dev`). A non-process path, so it
   reaches `main` and un-breaks ADR-0008's citation (which linked into `.scratch/`). Top
   matter reframed for a reader audience; body unchanged. ADR-0008, `map.md`, ticket 02
   links updated.

6. **Text and placement only** — no `config::validate` change, no `ConfigError`, no
   blocking Save widget, no new daemon message. Ticket 03 already ruled the within-run
   burst text-only; ticket 09 already makes Analog-repeat + Macro unsaveable.

### Handoff

`spec-user-facing-output-safety-guidance.md` is gated. Implementation — the two hint widgets, the expander, the README
section, and the feature-bullet pointers — is a fresh effort, per the map's Notes. The
research-doc move and the CONTEXT.md terms are already done; the implementation effort
only links to `docs/anti-cheat-input-heuristics.md` from the new README section.
