# 01 — README `## Output safety` section

Status: done — 2026-09-07

**What to build:** A reader following the README gets a full "Output safety" section
explaining that Acheron's synthetic output is always identifiable as synthetic and never
paced faster than a physically held key, that Macros are the deliberate exception, how to
keep a macro plausible to a game (no threshold numbers — by design), how to avoid wedging
their own system and how to recover from a runaway, and a friendly no-warranty
restatement. Feature bullets earlier in the README point here.

**Blocked by:** None — can start immediately.

Source: `.scratch/humane-output-rate/spec-user-facing output-safety guidance.md` §5.

- [x] New top-level `## Output safety` section added **between `## Usage` and
      `## Troubleshooting`**, with the four parts from spec §5: the intro paragraph +
      "Macros are the exception", `### Keeping a macro plausible to a game` (6 bullets +
      the ban-risk line), `### Not locking up your own system` (3 bullets + the
      three-item recovery list), and `### Your responsibility`.
- [x] Copy matches spec §5 verbatim; no threshold numbers are quoted anywhere in it.
- [x] The in-section links resolve: `docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md`,
      `docs/anti-cheat-input-heuristics.md`, and the `#licence` anchor.
- [x] The **Trigger modes** feature bullet and the **Macro** mention in the **Actions**
      feature bullet each gain ` — see [Output safety](#output-safety)`; no other bullet
      changes.
- [x] The `#output-safety` anchor is what GitHub generates for this heading (so ticket 03's
      link button can target it).
