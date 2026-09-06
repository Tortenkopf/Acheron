# Anti-cheat input-detection heuristics for the macro tips

Type: research
Status: resolved
Blocked by: —
Parent: [Humane output rate](../map.md)

## Question

What publicly-documented signals do games and anti-cheat systems use to distinguish
synthetic / automated input from a human at a keyboard, **restricted to timing and rate**
(not process/driver detection, not memory scanning)? Feed the macro-editor best-practice
tips (ticket 05).

Surface, with primary/high-trust sources where they exist:

- **Event-rate ceilings** — actions per second, keystrokes per second, clicks per second
  that trip a flag. Any published thresholds.
- **Inter-event regularity** — perfectly constant intervals, zero jitter, quantised
  timestamps as a tell. How much variance reads as "human".
- **Key-hold-duration distributions** — unrealistically short or uniform down→up times.
- **Repeat cadence** — synthetic autorepeat that doesn't match the OS autorepeat envelope
  (initial delay then steady period).
- **Macro signatures** — identical sequences replayed byte-for-byte, back-to-back, with
  identical timing each run.
- **Known handling of `uinput` / `evdev` injected events** specifically, if documented
  (device name heuristics, `EV_SYN` grouping, timestamp source).

Deliverable: a Markdown file under `.scratch/humane-output-rate/research/` on a
`research/humane-output-rate-anticheat` branch, with a short "implications for Acheron macro
authors" section. This is grounding for advice text — not a spec, and not a claim that
Acheron should evade detection (it can't; uinput origin is always visible).

## Answer

Findings written to `research/anticheat-input-timing-heuristics.md`
(570 lines, every claim tagged `[PRIMARY]` / `[OSS]` / `[SECONDARY]`).
**Relocated 2026-09-06 (ticket 05)** to
[`docs/anti-cheat-input-heuristics.md`](../../../docs/anti-cheat-input-heuristics.md) —
a non-process path, so it reaches `main`, is citable from ADR-0008, and is linked from
the README's new Output safety section. Top matter lightly reframed for a reader
audience; body unchanged. The throwaway
worktree branch was removed after the file was copied into the `dev` checkout; `.scratch/`
is gitignored so the tracker is local-only anyway.

Key facts for tickets 01, 03, 04, 05:

- **Linux held-key autorepeat** (`[PRIMARY]`, kernel `input.c`): USB keyboards do not
  repeat in hardware — the input core does it in software, `input_enable_softrepeat(dev,
  250, 33)`: **250 ms initial delay, then ~33 ms period (~30 Hz)**, each repeat an
  `EV_KEY value=2` with its own `SYN_REPORT`, timestamped `ktime_get()`. Runtime-tunable
  via `EVIOCSREP`. Confirms the ticket-01 bar exactly. **Mouse/gamepad `BTN_*` codes are
  never autorepeated** — one `1`, one `0`. Confirms `spawn_held` is the correct model.
- **`uinput` timestamps** (`[PRIMARY]`, current `uinput.c`): the "timestamps ignored" doc
  comment is stale — a valid userspace timestamp is now honoured, so round / fixed-stride /
  identical stamps are a directly-readable tell. (Acheron does not set them; relevant to
  the tips, and to whether the audit should care.)
- **Regularity is the universal tell**, not raw rate. osu! `circleguard` (`[OSS]`):
  ~5 ms SD of tap timing = bot-assisted; human loop-interval median 15–16 ms. CS2 (Valve,
  `[PRIMARY]` rule text): "exactly 0 ms overlap and 0 ms neutral" between opposite inputs =
  macro. Minecraft anti-cheats (`[SECONDARY]`): low interval SD, symmetric ±uniform jitter,
  few distinct delay values, rounded CPS.
- **Rate ceilings** (`[SECONDARY]`/`[OSS]`): human sustained clicking 4–7 CPS, jitter
  ceiling ~17; Minecraft flags ≥16 CPS. Stimulus-driven human reaction-time median
  ~230–273 ms → sub-150 ms responses implausible.
- **Key hold/dwell**: keystroke-dynamics work discards intervals outside ~30–500 ms as
  non-typing; naive fixed/PRNG timing is "trivially separable" from human (QUACK,
  `[PRIMARY]`, ROC-AUC > 0.9 within 70 keystrokes).
- **Macro signatures**: run-to-run byte + timing identity, and timing stats that never
  drift across a session (humans drift with fatigue).
- **No mainstream game anti-cheat (VAC/BattlEye/EAC/Vanguard) publishes evdev/uinput
  timing thresholds or device-name blocklists** — that part of the file is built from
  kernel/OS primary sources + OSS detectors only.

The file closes with an 8-point non-evasion "Implications for Acheron macro authors"
section (respect the live kernel repeat rate; don't fake a hold with a tight down/up loop;
~50–200 ms inter-key gaps; vary timing; don't loop macros unattended; one physical press =
one game action).
