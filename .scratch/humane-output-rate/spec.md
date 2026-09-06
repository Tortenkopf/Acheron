# Spec: user-facing output-safety guidance

Status: gated — ready for a fresh implementation effort
Source: [Humane output rate](map.md) ticket [05](issues/05-spec-macro-editor-safety-guidance.md)
(grilled + ratified with Charon, 2026-09-06)

Depends on the facts in tickets [01](issues/01-audit-holding-repeating-output-paths.md)
(the audit), [03](issues/03-self-dos-macro-guardrail.md) (the Macro guardrail decision),
[06](issues/06-clamp-repeat-pace-loop-deadlines.md) (the pace-loop clamp, landed) and
[07](issues/07-spec-kernel-shaped-repeat.md) (`spec-kernel-shaped-repeat.md`, gated). The
guidance below describes the **post-change** reality: built-in Trigger modes stay within
the [Physical-plausibility ceiling](../../CONTEXT.md) and a held single key presents as
genuine kernel autorepeat; only a Macro can exceed the ceiling.

---

## 1. Goal

Three user-facing surfaces, all **text and placement only** — no config validation, no
new runtime behaviour, no editor warning widget that blocks a save:

1. A standing **GUI hint** in the Macro editor: `uinput` output is always identifiable as
   synthetic, Acheron paces its own Trigger modes but a Macro does exactly what you write.
2. An **"About macro safety"** `Gtk.Expander` in the Macro editor holding the compact
   best-practice tips.
3. A **GUI hint** below the Trigger-mode selector whenever **Analog-repeat** is chosen:
   it can drive a key faster than a hand could near full travel; single-player / known-safe
   games only.

Plus the long-form version as a new **`## Output safety`** section in `README.md`, and the
anti-cheat research relocated to `docs/` as its citable backing (done — §6).

**Non-goals.** Not a config-layer restriction (ticket 03 decided the within-run burst and
the once-fired Macro stay unrestricted; ticket 09 already rejects Analog-repeat + Macro at
the config layer, so no tip needs to warn about it). Not a jitter engine. Not a claim that
any macro is "safe" — the guidance quotes no threshold numbers, by design (ticket 02).

---

## 2. Macro-editor disclaimer — the standing GUI hint

**What**: a single always-visible line at the top of the Macro editor, above the
`Gtk.Expander` from §3 and above the existing "Changes save automatically." hint.

**Widget**: a **GUI hint** (see `CONTEXT.md` → Interface) — `Gtk.Label`, `xalign=0`,
`wrap=True`, `css_classes=["dim"]`, in the same style as `_CONTROLLER_MACRO_HINT`
(`gui/acheron_gui/library_view.py:118`). Its condition is simply "the Macro editor is
shown", so it is always present on the Macro tab. **Not** shown on the Stepper tab, and
**not** added to `binding_editor.py` (the disclaimer belongs where a Macro is *written*,
not where it is *assigned*).

**Exact copy** (leads with the `⚠️` emoji — a deliberate exception to the GUI's
no-emoji norm, matched by the Analog-repeat hint in §4):

```
⚠️ uinput input is always identifiable as synthetic; Acheron keeps its own Trigger
modes within physically-plausible rates, but a Macro does exactly what you write.
Use macros with caution!
```

Render as one wrapped line (no hard breaks); the three sentences above are the content,
not a layout.

---

## 3. "About macro safety" — the Macro-editor expander

**What**: a `Gtk.Expander`, **collapsed by default**, directly under the §2 disclaimer
line in the Macro editor's column. Label: **`About macro safety`**.

**Placement note for the implementer**: the Macro and Stepper editors are built to
identical measurements so nothing shifts when the user flips tabs
(`library_view.py:126-146`). The expander is Macro-only, so the Stepper editor needs an
inert reserve of the same height (mirror the `_header_middle_reserve()` pattern already
used for the Stepper/Macro middle slot), **or** place the expander below the
vexpanding step list where a height difference doesn't shift anything above it. The
implementer picks; the spec's requirement is only that flipping tabs stays visually
stable.

**Body** — render each theme as a bold sub-label followed by a bulleted `Gtk.Label`
list (dim), then a link row. Exact content:

> **Staying plausible to a game**
>
> - Acheron's built-in Trigger modes stay within the rate a physically held key
>   produces. A Macro does exactly what you write — it's the one feature that can
>   exceed that.
> - Prefer a built-in Trigger mode when you don't need a sequence.
> - Leave realistic gaps between steps — tens of ms and up. Sub-30 ms gaps and holds
>   resemble nothing physical.
> - Don't fake a held key with a fast down/up loop; don't make every delay identical;
>   don't loop an identical sequence unattended for hours. Regularity, not raw speed,
>   is the usual tell.
> - Depending on the game, running a macro at all can risk an automation ban. That's
>   your call.
>
> **Not locking up your own system**
>
> - A zero/tiny-delay Macro under Toggle or Hold-to-repeat: Acheron floors how often a
>   Macro *re-fires*, not the cadence *within* one run.
> - Every KeyDown needs a KeyUp; don't leave a modifier held.
> - Avoid a Macro that never returns or spams Profile/Layer switches.
> - **To stop a runaway:** focus the Acheron window (stops every Toggle), pause the
>   Daemon from the tray, or `systemctl --user stop acheron-daemon`.

**Link row** (below the two themes): a `Gtk.LinkButton` labelled
`Full guide: Output safety` pointing at the project README's `#output-safety` anchor.
If a stable public URL for the README is not known at build time, fall back to a
`Gtk.Label` reading `Full guide: the "Output safety" section of the README`.

---

## 4. Analog-repeat selection hint

**What**: a **GUI hint** shown directly below the Trigger-mode dropdown
(`gui/acheron_gui/binding_editor.py`, `build_binding_editor_fields`) **whenever the
selected Trigger mode is `analog_repeat`**, in every editor that offers the selector
(the individual binding editor; the deep-stage editor). Removed the moment the
selection changes away from Analog-repeat.

**Widget**: same `["dim"]` `Gtk.Label` style as `_CONTROLLER_MACRO_HINT`. This is a
**GUI hint**, not a **Toast label** — it tracks the current selection rather than firing
once. (This deliberately supersedes ticket 01 Q3's "one-time toast" phrasing: the
"single-player only" warning is worth showing every time Analog-repeat is the live
choice, and a persistent hint needs no `ui_state` seen-flag or dismiss control.)

**Wiring note**: `binding_editor.py` already has the single-handler-on-`trigger_dd`
pattern for exactly this (`_trigger_handler`, added for ticket 42's Trigger-mode-dependent
key warning) — reuse it so the hint's show/hide doesn't pile up one stale listener per
`render_action_editor()` rebuild.

**Exact copy** (leads with `⚠️`, matching §2):

```
⚠️ Analog-repeat can drive a key far faster than a hand could as it nears full
travel. Use it only in single-player or otherwise known-safe games — never in
competitive multiplayer, where it may be flagged as automation.
```

Note for the implementer: ticket 07's `value=2` rebuild makes the *top of the ramp*
(hold-solid) present as genuine kernel autorepeat. The hint is about the real **rate**
the ramp can reach, not the event **shape** — the copy above is written to stay correct
after ticket 07 lands.

---

## 5. README `## Output safety` section

**Placement**: a new top-level section in `README.md`, **between `## Usage` and
`## Troubleshooting`**.

**Exact content**:

```markdown
## Output safety

Acheron turns your key presses into synthetic input events. That output is always
identifiable as synthetic — on Linux a `uinput` device can be told apart from real
hardware — and Acheron doesn't try to hide it. What it *does* guarantee is that its
own Trigger modes never emit events faster than the Linux input stack would for a
physically held key: the kernel's own autorepeat delay and period for a held key, and
exactly one press/release for a held mouse or gamepad button. Held or repeated single
keys are emitted as genuine kernel autorepeat, not a stream of press/release pairs.
(Rationale: [ADR-0008](docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md);
the input-timing heuristics behind it are collected in
[docs/anti-cheat-input-heuristics.md](docs/anti-cheat-input-heuristics.md).)

**Macros are the exception.** A Macro does exactly what you write, at the cadence you
write it — Acheron does not pace the keystrokes inside a single run. That's
deliberate: a macro is a shortcut for a sequence you could type yourself. It also
means a badly-written macro is the one way to make Acheron produce output no hand
could.

### Keeping a macro plausible to a game

Some games, and most competitive multiplayer anti-cheat, look for input no person
could produce. **There are no safe numbers to quote** — it depends entirely on the
game, and any threshold here would be guesswork — but the shapes that draw scrutiny
are well understood:

- **Prefer a built-in Trigger mode** when you don't actually need a sequence.
  Hold-to-repeat, Toggle and Analog-repeat all stay within the kernel's rate; a macro
  loop doesn't.
- **Leave realistic gaps between steps** — tens of milliseconds and up. Gaps or key
  holds under ~30 ms resemble nothing a physical keyboard produces.
- **Don't fake a held key with a fast down/up loop.** A real held key sends repeat
  events after an initial delay; it doesn't hammer press/release.
- **Don't make every delay identical.** Perfectly uniform timing — a metronome, or
  every gap an exact multiple of 10 ms — is the single most cited automation tell.
  (Not a call to build a jitter engine; just don't go out of your way to make the
  timing mathematically perfect.)
- **Don't loop a macro unattended for hours.** An identical sequence repeated
  forever, with timing that never drifts, is a classic macro signature.
- **One physical press should map to one in-game action.** Turning a single keypress
  into a burst of many actions is exactly what rules like Counter-Strike 2's
  input-automation ban describe, however the events are produced.

Depending on the game, running a macro at all can carry a real risk of a ban. That
risk is yours to weigh.

### Not locking up your own system

A macro runs on *your* machine first. A few ways to wedge it, and how to recover:

- **Zero or tiny delays under Toggle or Hold-to-repeat.** Acheron floors how often a
  macro *re-fires* — it can't loop faster than the kernel's autorepeat period — but it
  does not floor the cadence of keystrokes *within* one run. A long macro of no-delay
  steps still fires them all in a sub-millisecond burst every loop.
- **Unbalanced keys.** Every `KeyDown` step needs a matching `KeyUp`. A held modifier
  (Ctrl, Alt, Super) with no release will fight every other app until you clear it.
- **A macro that never returns**, or one that rapidly switches Profile or Layer, or
  otherwise fights whatever window has focus.

To stop a runaway:

- **Focus the Acheron window** — it stops every running Toggle immediately.
- **Pause the Daemon** from the tray icon (or `systemctl --user stop acheron-daemon`).
- A connected client can also ask the Daemon to withhold all output without stopping
  anything internally.

### Your responsibility

Acheron is free software provided with no warranty of any kind (see
[Licence](#licence)). In plainer terms: any trouble you get yourself into using
Acheron — and the Macro feature especially — is entirely your own responsibility.
```

**Feature-bullet pointer**: append ` — see [Output safety](#output-safety)` to the
**Trigger modes** bullet (currently `README.md:66-68`) and to the **Macro** mention in
the **Actions** bullet (`README.md:58-59`). No other bullet changes.

---

## 6. Anti-cheat research relocation — DONE in this session

`.scratch/humane-output-rate/research/anticheat-input-timing-heuristics.md` →
**`docs/anti-cheat-input-heuristics.md`** (`git mv`, on `dev`). A non-process path, so it
reaches `main`, and:

- **ADR-0008** line 4 now links `../anti-cheat-input-heuristics.md` (was a `.scratch/`
  path that would have shipped broken to `main`).
- **`map.md`** and ticket 02's Answer updated to the new path.
- Top matter (4 lines) reframed from "Research for … ticket 02" to a reader-facing
  intro that points back at the README's Output safety section and ADR-0008. Body,
  confidence tags, and Sources unchanged.

The implementation effort does **not** need to move anything — only link to
`docs/anti-cheat-input-heuristics.md` from the new README section (already in the §5
copy).

---

## 7. CONTEXT.md — DONE in this session

New `### Interface` subsection with **Toast label** and **GUI hint**, distinguishing the
one-shot styled notice (ticket 60 / ticket 68 pattern) from the persistent condition-bound
advisory line (`_CONTROLLER_MACRO_HINT` pattern). The §2 disclaimer and the §4
Analog-repeat notice are both **GUI hints**.

---

## 8. Placement / behaviour summary

| Surface | Widget | Where | Shown when | Dismissable |
|---|---|---|---|---|
| Macro disclaimer (§2) | GUI hint (`["dim"]` label, `⚠️`) | top of Macro editor | always, on the Macro tab | no |
| "About macro safety" (§3) | `Gtk.Expander`, collapsed | under the disclaimer | always, on the Macro tab | collapses |
| Analog-repeat notice (§4) | GUI hint (`["dim"]` label, `⚠️`) | below Trigger-mode dropdown | Trigger mode == `analog_repeat` | no (tracks selection) |
| Output safety (§5) | README section | between Usage & Troubleshooting | — | — |

No `config::validate` change. No `ConfigError`. No blocking Save. No `ui_state`
persistence. No new daemon message.

---

## 9. Out of scope for the implementation effort

- Any config-layer check on Macro step timing — ticket 03 ruled this out; the within-run
  burst and the once-fired Macro are accepted, text-only.
- A tip about Analog-repeat + Macro — ticket 09 makes that combination unsaveable, so
  there is nothing to warn about.
- Changing the `⚠️` to an icon asset, or theming the hint differently from
  `_CONTROLLER_MACRO_HINT` — keep the existing dim-label style.
- Any change to the `value=2` behaviour or the pace-loop clamps — those are tickets 07
  (gated spec) and 06 (landed).
- A separate help dialog, a Help menu, or an in-GUI copy of the full research doc — the
  expander is compact, the README is the long form, `docs/anti-cheat-input-heuristics.md`
  is the backing.

---

## 10. Map / cross-ticket effects (apply when ticket 05 resolves)

- **Map** → Decisions-so-far gains this ticket's gist; both **Not yet specified** items
  stay (the guidance UI is now specced but not built — it remains a fresh effort; the
  `value=2` implementation is unchanged). Update the `map.md` research link to
  `docs/anti-cheat-input-heuristics.md`.
- **`.scratch/README.md`** → ticket 05 `[**resolved** 2026-09-06]`; the effort's Fog line
  "implement the guidance UI" now reads "spec gated" like the `value=2` line.
- **No ticket rewiring** — 05 had no downstream tickets; 08 and 09 are independent.
- The effort's remaining open tickets after this: **08** (test-only, frontier) and **09**
  (config restriction, frontier). Once both resolve, the map's destination is reached:
  audit ratified, surfaces fixed (06), ADR + term written (04), and both specs
  (`spec.md`, `spec-kernel-shaped-repeat.md`) gated.
