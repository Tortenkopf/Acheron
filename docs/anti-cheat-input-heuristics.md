# Anti-cheat input-timing & rate heuristics

Background for Acheron's [Output safety](../README.md#output-safety) guidance: what
publicly-documented signals let games and anti-cheat systems tell synthetic keyboard
and mouse input from a human, and where the line between "plausible" and "flagged"
actually sits. This is the grounding for the advice in the README and the macro
editor — it is not an evasion guide, and it quotes no "safe" threshold, because
there isn't one. It also underpins [ADR-0008](adr/0008-physical-plausibility-ceiling-for-synthetic-output.md),
which records the ceiling Acheron holds its own (non-macro) output to.

**Scope.** What publicly-documented signals let games / anti-cheat / bot-detection
systems tell synthetic keyboard & mouse input from a human, **restricted to timing
and rate**. Explicitly *out of scope*: process/module detection, driver signing,
memory scanning, kernel-module detection, injection-origin flags (covered only
briefly, as the "you are always visible anyway" baseline).

**Framing.** This is grounding for user-facing advice to macro authors who want
their macros to be *unobtrusive and plausible*, not for evasion. Acheron's `uinput`
output is always identifiable as synthetic and that is accepted (map, "Out of
scope"). Where a source describes an evasion trick, only the **detection signal it
implies** is recorded here, never the countermeasure.

**Confidence tags.** `[PRIMARY]` = the owner of the claim (kernel source, OS docs,
a patent, a peer-reviewed paper, first-party rules). `[OSS]` = a well-documented
open-source detector whose code/threshold is inspectable. `[SECONDARY]` =
vendor blog / forum / community write-up; directional only.

---

## 0. The baseline: origin is visible without any timing analysis

Before any timing heuristic runs, both major desktop OSes tag injected input, and
Linux exposes the virtual-device origin:

- **Windows.** Low-level hooks receive `LLMHF_INJECTED` (mouse) / `LLKHF_INJECTED`
  (keyboard, bit 4) in the hook struct's `flags`; `*_LOWER_IL_INJECTED` additionally
  marks injection from a lower-integrity process. These distinguish physical input
  from anything produced by `SendInput` / `mouse_event` / `keybd_event`.
  `[PRIMARY]` — Microsoft, `MSLLHOOKSTRUCT` / `KBDLLHOOKSTRUCT` reference.
  <https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msllhookstruct>
- **Linux.** A `uinput` device is a normal kernel `input_dev`; to *most* userspace
  it is indistinguishable from a physical device, but it **can** be told apart: the
  sysfs path is under `/sys/devices/virtual/input/inputN`, and the `phys`/`uniq`
  strings are whatever userspace set (commonly empty). "You can detect
  uinput-created virtual devices, but usually a process doesn't need to care so all
  the common userspace (libinput, Xorg) doesn't bother."
  `[PRIMARY]` — Peter Hutterer (X.org / libinput / kernel-input maintainer),
  "The difference between uinput and evdev".
  <http://who-t.blogspot.com/2016/05/the-difference-between-uinput-and-evdev.html>;
  libevdev uinput docs
  <https://www.freedesktop.org/software/libevdev/doc/latest/group__uinput.html>

Takeaway for Acheron: the interesting question is never "can they see it's
synthetic" (yes) but "does the *timing* of what we emit look like a plausible
physical device". The rest of this doc is that question.

---

## 1. Event-rate ceilings (actions / keystrokes / clicks per second)

### 1.1 Human clicking rates

| Regime | Rate | Source |
|---|---|---|
| Average sustained single-finger clicking | **4–7 CPS** (≈ 5–7 typical) | `[SECONDARY]` measurehuman.com "Average Click Speed"; Redragon CPS-test page |
| 1-second burst (peak) | most people **6–8 CPS**, trained ~10 | `[SECONDARY]` same |
| "Jitter" clicking (muscle-tremor technique) | **>12 CPS**, real-world ceiling ~17 CPS sustained | `[SECONDARY]` measurehuman / opauto-clicker "Human CPS Records" |
| "Butterfly" (two fingers, one button) | ~22 CPS burst | `[SECONDARY]` same |
| "Drag" clicking (friction, one swipe = many switch actuations) | **25–35+ CPS** | `[SECONDARY]` same |

These numbers are community/vendor-measured, not peer-reviewed — treat as
order-of-magnitude. The robust reading: **a sustained click stream above roughly
10–15 events/s from a single button is already outside ordinary human range**, and
above ~20/s only exotic hardware-abuse techniques (or automation) produce it.

### 1.2 Minecraft server-side click caps `[OSS]/[SECONDARY]`

Minecraft PvP anti-cheats are the most openly-discussed click-rate detectors
because clicks map to network packets (arm-swing / `use` / block-place packets),
one packet stream per tick (50 ms on 1.8):

- More than **1** `use`/place packet per tick ⇒ implied CPS > 20, flagged.
- **3** place packets in a tick ⇒ implied > 60 CPS, flagged; 2/tick tolerated.
- CPS is sampled for exactly 1 s on arm-swing; "high CPS in the range of **16 and
  above**" is a common first-pass threshold.
- `Geo25rey/CPS-Limiter` (Spigot plugin) simply *rate-limits* clicks to a
  configured CPS rather than banning. `[OSS]`
  <https://github.com/Geo25rey/CPS-Limiter>

Sources: Hypixel "AutoClicker Detection Tutorial (server side)"
<https://hypixel.net/threads/autoclicker-detection-tutorial-server-side.5483042/>;
various Spigot/BuiltByBit CPS-detector resources. **All `[SECONDARY]`** (forum
tutorials); numbers are consistent across them but not authoritative.

### 1.3 Reaction-time floor (rate ceiling on *stimulus-driven* actions)

A distinct ceiling: an action that *responds to something on screen* cannot
legitimately fire faster than human visual reaction.

- Large online dataset median simple reaction time ≈ **273 ms** (Human Benchmark).
- Controlled lab study, n = 1469, ages 18–65: mean simple RT **231 ms**
  (**213 ms** corrected for hardware latency). `[PRIMARY]`
  <https://www.ncbi.nlm.nih.gov/pmc/articles/PMC4374455/>
- Competitive gamers: ~200–250 ms; sub-200 ms is top-decile.
- Detection framing: interactions that fire "within a few tens of milliseconds of
  the element becoming actionable" are treated as synthetic (no human perception
  loop). `[SECONDARY]` crawlex "Detecting automation via timing"
  <https://blog.crawlex.net/blog/detecting-automation-via-timing/>

---

## 2. Inter-event regularity — constant intervals & zero jitter

This is the single most consistently-cited tell across every source class.

### 2.1 The core signal

- **"Identical inter-event gaps are the signature a fixed-delay typing loop leaves
  in the timestamp stream."** A stream like `64 64 64 64 64 ms` (or any single
  repeated value) is immediately separable from human typing, whose dwell and
  flight times vary by finger, key, and digraph. `[SECONDARY]` crawlex, above.
- Even a bot that targets a *human-plausible mean* (e.g. 300 ms between keystrokes)
  "still fails through excessive regularity, lacking natural variance and
  corrections." `[SECONDARY]` crawlex.
- The QUACK study (peer-reviewed, HID-injection detection) frames existing
  deployed defences as "simple heuristics based on typing speed or **timing
  regularity**", and confirms a purely-timing model with **no access to key
  content** separates human from machine: ROC-AUC **> 0.9 at 70 keystrokes**
  observed, saturating between 70 and 100. Naive fixed/PRNG timing generators are
  "trivially separable". `[PRIMARY]` Lotto, Marchiori, Conti, "QUACK! Making the
  (Rubber) Ducky Talk", arXiv:2604.15845.
  <https://arxiv.org/abs/2604.15845>

### 2.2 How much variance reads as human — concrete numbers

- **osu! `circleguard`** (`[OSS]`, inspectable thresholds): "unstable rate" (UR) is
  the standard deviation of hit-timing errors, ×10. Legit human play sits well
  above the tool's `UR_LIMIT = 50` (≈ 5 ms SD of tap timing); *below* ~50 UR is
  treated as "relax"-assisted (a bot handling the tapping).
  <https://circlecore.readthedocs.io/en/v4.5.1/appendix.html>,
  <https://circleguard.github.io/circlecore/using-circleguard.html>
- **osu! frametime** (`circleguard`, `[OSS]`): replay frame interval. Human median
  **15–16 ms** (game targets 16.67 ms / 60 Hz, with natural jitter). `FRAMETIME_LIMIT
  = 13 ms`: a *median* frametime below 13 ms indicates "timewarp" (client-speed
  automation). The docs stress the *distribution shape* matters, not just the mean —
  a human histogram has one broad peak near 16.67 ms; automation shows a narrow
  peak shifted low.
  <https://github.com/circleguard/circleguard/wiki/Frametime-Tutorial>
- **Valve / CS2** (`[PRIMARY]`, first-party rule + community-observed mechanism):
  In the 2024-08-19 update Valve began kicking for "input automation (via scripting
  or hardware) that circumvents core skills" — null-cancelling movement binds,
  jump binds, Snap Tap / SOCD keyboards. Community analysis of what trips it:
  "hundreds of direction changes with **exactly 0 ms of overlap and 0 ms of neutral
  state**" between opposite keys — i.e. *impossibly clean* A→D transitions with
  zero variance — read as a macro. Valve's own wording: "some hardware features
  have blurred the line between manual input and automation."
  <https://store.steampowered.com/news/> (8/19/2024 CS2 update);
  reporting: <https://www.gamingonlinux.com/2024/08/valve-bans-keyboard-automation-in-counter-strike-2-update/>,
  <https://esports.gg/news/counter-strike-2/valve-ban-snap-tap-and-other-movement-automations-in-cs2/>
- **Minecraft anti-cheats** (`[SECONDARY]`): flag when "the delay in ticks between
  clicks remains constant"; use **standard deviation** of the click-interval
  distribution (too low = automated), **skewness** (a randomised auto-clicker
  centred on 15 CPS with ±2 jitter produces a symmetric distribution around the
  mean; humans skew), and **kurtosis** ("tailedness"). A randomiser that draws
  uniformly around a set point is specifically called out as detectable *because*
  the distribution is too clean and too symmetric. Additional signals listed:
  "rounded CPS", "distinct delay counts" (few unique interval values), "phase
  locks". Hypixel tutorial, above; SpigotMC "AutoKiller" resource.

### 2.3 Quantised / round-number timestamps

- Synthetic intervals "computed to land on suspiciously round numbers (a 50 ms
  delay, a 100 ms delay)" survive timer-precision clamping *as* round numbers and
  remain a tell. `[SECONDARY]` crawlex.
- Browser context: genuine 60 Hz cursor motion clusters near **16.7 ms**
  (`requestAnimationFrame`-aligned); synthetic events "spaced exactly 10 ms apart,
  or all sharing a single timestamp" violate that platform rhythm. `[SECONDARY]`
  crawlex. (Mechanism-analogous to the osu! frametime check.)
- Events all carrying **one identical timestamp**, or timestamps at an exact fixed
  stride, is called out repeatedly.

### 2.4 Practical "how much jitter is enough" read

No primary source publishes a "safe" SD, and asking for one is the evasion
question. What the sources *do* support as *implausibly low*:

- Tap-timing SD ≈ **5 ms or below** over a long run (osu! UR 50). `[OSS]`
- **Zero** inter-event variance over hundreds of events (CS2 0 ms overlap). `[PRIMARY]`
- Interval distribution that is **symmetric and single-valued / few-valued**
  (Minecraft skew + distinct-delay-count checks). `[SECONDARY]`
- A single repeated interval value, or exact multiples of 10 ms. `[SECONDARY]`

Human physical input, by contrast, is described everywhere as carrying
"micro-shivers", "biological noise from physical finger movement", switch-travel
variation, fatigue drift, and hand-repositioning — a broad, skewed, non-stationary
distribution.

---

## 3. Key-hold-duration (dwell) distributions

- **Definitions** (universal in the keystroke-dynamics literature): *hold / dwell
  time* HT = key-down → key-up of the same key; *flight time* FT = key-up → next
  key-down. `[PRIMARY]` QUACK §2; keystroke-dynamics survey arXiv:2303.04605.
- **Typical human magnitudes**: keystroke-dynamics studies routinely **discard
  digraph intervals shorter than 30 ms or longer than 500 ms** as non-typing
  artefacts — i.e. real inter-key timing lives in ~30–500 ms. `[PRIMARY]` survey
  arXiv:2303.04605 (quoting its filtering rule).
- The canonical benchmark (Killourhy & Maxion, DSN 2009, CMU dataset: 51 subjects
  × 400 reps of `.tie5Roanl`) exists precisely because **each person's HT/FT
  vector is stable enough to identify them and variable enough between reps to be a
  distribution, not a constant** — top verifiers reach ~9.6% EER.
  `[PRIMARY]` <https://www.cs.cmu.edu/~keystroke/>
- **Synthetic tell**: injection frameworks emit a *fixed* or *near-fixed* hold
  time (often only a "press then release" with no modelled dwell at all), so the
  HT distribution collapses to a spike. QUACK: naive generators (fixed delay,
  PRNG) are "trivially separable" on timing features alone; a 70–100-keystroke
  window suffices. `[PRIMARY]`
- **osu! `osuReplayAnalyzer`** (`[OSS]`) lists "key press time" and "interval
  between key presses" among its macro-bot indicators, and "extremely fast
  singletaps" — i.e. an unrealistically *short* and *uniform* down→up time is a
  named signal. (Repo documents the categories; numeric thresholds are in source,
  not the README.) <https://github.com/firedigger/osuReplayAnalyzer>
- HID-injection timing-signature work (Holoska & Doucek, "Detecting HID Keystroke
  Injection … Using Timing Signatures", SSRN 6443044) reports a feature set of
  **cadence, jitter, tightness, robust percentiles, tail width, throughput**
  computed from KeyDown inter-key intervals, and claims it flagged **all 365
  clipboard-replay sessions** while clearing **all 20 human typing sessions**.
  `[PRIMARY]` (paper; abstract only was reachable — full text paywalled, so the
  exact ms bins are not verified here).
  <https://papers.ssrn.com/sol3/papers.cfm?abstract_id=6443044>

**Reads-as-implausible:** every emitted key having the *same* hold time; hold times
far below ~30 ms; a dwell distribution with near-zero spread.

---

## 4. Repeat cadence vs. the OS autorepeat envelope

### 4.1 What a real held key on Linux looks like — from kernel source `[PRIMARY]`

When a key is physically held, USB HID keyboards do **not** send repeats in
hardware (the HID boot protocol has no autorepeat); the **kernel input core
synthesises them in software**. `drivers/input/input.c`:

- At device registration, if the driver set no repeat values:
  ```c
  if (!dev->rep[REP_DELAY] && !dev->rep[REP_PERIOD])
      input_enable_softrepeat(dev, 250, 33);
  ```
  ⇒ **default initial delay `REP_DELAY = 250 ms`, default period `REP_PERIOD =
  33 ms` (≈ 30.3 Hz)**.
- On key-down, `input_start_autorepeat()` arms a timer at
  `jiffies + msecs_to_jiffies(dev->rep[REP_DELAY])`.
- `input_repeat_key()` (the timer callback) then, for as long as the key stays
  down, emits:
  ```c
  input_set_timestamp(dev, ktime_get());
  input_handle_event(dev, EV_KEY, dev->repeat_key, 2);   /* value 2 = autorepeat */
  input_handle_event(dev, EV_SYN, SYN_REPORT, 1);
  ```
  and re-arms itself at `jiffies + msecs_to_jiffies(dev->rep[REP_PERIOD])`.

So the canonical physical-hold event stream is:

```
EV_KEY code=KEY_X value=1   ; SYN_REPORT           (down)
   ... 250 ms ...
EV_KEY code=KEY_X value=2   ; SYN_REPORT           (first repeat)
   ... 33 ms ...
EV_KEY code=KEY_X value=2   ; SYN_REPORT           (steady repeats @ ~30 Hz)
   ... 33 ms ...            ...
EV_KEY code=KEY_X value=0   ; SYN_REPORT           (up)
```

Kernel source (current):
<https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/input/input.c>
(functions `input_enable_softrepeat`, `input_start_autorepeat`, `input_repeat_key`,
`input_register_device`).

- **Value semantics** are documented: down = 1, up = 0, **autorepeat = 2**.
  `[PRIMARY]` kernel `Documentation/input/event-codes.rst`
  <https://www.kernel.org/doc/html/latest/input/event-codes.html>
- The delay/period are **runtime-configurable** per device via `EVIOCSREP`
  (ioctl) or by writing `EV_REP`/`REP_DELAY`/`REP_PERIOD` events; X11
  (`xset r rate`), GNOME, KDE etc. commonly override the default to e.g.
  660/25 ms or 300/30 ms. Acheron already reads the live value via
  `evdev::Device::get_auto_repeat()` (`daemon/src/capture/analog.rs
  read_kernel_auto_repeat`), falling back to its own tuned constants.
  `[PRIMARY]` `Documentation/input/event-codes.rst`; util sources for `EVIOCSREP`.

### 4.2 The synthetic-autorepeat tells

No anti-cheat vendor publishes "we check autorepeat shape", but the signals fall
straight out of §2 + §4.1:

- **Wrong event `value`.** A real held key repeats with `value = 2`. A macro that
  loops full down(1)/up(0) pairs to simulate a hold produces a stream of
  `1,0,1,0,…` that a physical hold never produces — and each pair also implies a
  dwell time (§3) that a real repeat doesn't have.
- **Wrong envelope.** Real autorepeat = one initial gap (~250 ms, or whatever
  `REP_DELAY` is set to) **then** a *different*, shorter steady period (~33 ms).
  Synthetic repeat that starts immediately at the steady rate, or uses one uniform
  gap throughout (no distinct initial delay), doesn't match.
- **Too-fast steady rate.** Emitting faster than the live `REP_PERIOD` (default
  ~33 ms ⇒ ~30 Hz) is faster than the kernel itself would repeat that key — the
  exact bar this map sets.
- **Zero jitter on the period.** Kernel autorepeat is timer-driven and *itself*
  fairly regular, but it is subject to `jiffies` quantisation (`msecs_to_jiffies`,
  typically `CONFIG_HZ = 250` or `1000`) and scheduler latency, so real repeats
  show ms-scale scatter. A period that is *exactly* constant to the microsecond is
  a tell (§2.3).
- **Timestamp source.** Kernel autorepeat events are stamped with `ktime_get()` at
  emission (see code above) — monotonic, non-round. See §6 for the injected-event
  case.

### 4.3 Held mouse / gamepad buttons

The kernel does **not** autorepeat `BTN_*` codes — `input_repeat_key` only acts on
keys in `dev->key` that are repeat-eligible, and mouse-button / gamepad-button
codes are not enabled for softrepeat. A physically held mouse button therefore
produces **exactly one `value=1` then one `value=0`, no intermediate events**.
Any synthetic "repeat" on a held button has no physical analogue at all. `[PRIMARY]`
kernel `input.c` (`input_repeat_key` guards on `test_bit(dev->repeat_key,
dev->key)`); consistent with the map's "one Down/Up, no repeat" bar.

---

## 5. Macro signatures — identical sequences replayed

- **Byte-for-byte identical timing across runs.** BattlEye's own site describes
  "heuristic/generic detection routines" and behavioural analysis for "superhuman
  patterns" / "statistical anomalies"; community consensus on the specific macro
  vector is "the timing of keystrokes would always be the same" run to run.
  `[SECONDARY]` (BattlEye publishes no specifics)
  <https://www.battleye.com/>
- **Replay-similarity detection** is a first-class, numeric check in osu!
  `circleguard`: two replays whose cursor paths stay **within ~17 px** of each
  other are flagged as replay-stealing / the same automated source. The same idea
  generalises: two macro executions that are identical to the sample resolution
  are a signature. `[OSS]`
  <https://circleguard.github.io/circlecore/> ; ReplayGuard cites the same "within
  17 px" number. <https://replayguard.pages.dev/>
- **Non-stationarity is the human baseline.** QUACK's strongest synthetic
  generators are explicitly *non-stationary* histogram / GAN models; the paper's
  finding is that detectors generalise by *generator family*, and that a
  **stationary** distribution (same statistics forever) is itself the weak point —
  humans drift over a session (fatigue, attention, posture). A macro that emits the
  identical distribution on minute 1 and minute 90 is the tell. `[PRIMARY]`
  arXiv:2604.15845.
- **Minecraft**: "clicks at the same rate for extended periods without variation"
  and "flying-packet count between arm-swings identical and unchanged for extended
  periods" are named signatures. `[SECONDARY]`

**Reads-as-implausible:** the same sequence, at the same cadence, repeated
back-to-back many times with no drift; two runs that diff to zero; a session whose
timing statistics never move.

---

## 6. Documented handling of `uinput` / `evdev`-injected events

### 6.1 EV_SYN / SYN_REPORT grouping `[PRIMARY]`

- Injected events **must** be terminated by an `EV_SYN` / `SYN_REPORT` per logical
  hardware event: "After sending [events], the `EV_SYN` event type with
  `SYN_REPORT` code must be [sent]." Kernel `Documentation/input/uinput.rst`.
  <https://www.kernel.org/doc/html/latest/input/uinput.html>
- "Anything before a `SYN_REPORT` should be considered one logical hardware event."
  A reader groups all events up to a `SYN_REPORT` as simultaneous. Hutterer,
  who-t blog (above).
- `SYN_REPORT` "Used to synchronize and separate events into packets of input data
  changes occurring at the same moment in time." Kernel
  `Documentation/input/event-codes.rst`.
- Consequence for a detector: a well-formed injector frames one key transition +
  one `SYN_REPORT`, exactly like a physical device. Mis-framing (e.g. batching many
  key transitions into one SYN frame, or omitting SYN_REPORT) is malformed and
  conspicuous. `SYN_DROPPED` is *only* emitted by the kernel on evdev client buffer
  overrun — an injector should never produce it.

### 6.2 Timestamp source `[PRIMARY]` — important nuance

The kernel doc's example comment ("timestamp values below are ignored") is **stale
for current kernels**. Current `drivers/input/misc/uinput.c
uinput_inject_events()`:

```c
timestamp = ktime_set(ev.input_event_sec, ev.input_event_usec * NSEC_PER_USEC);
if (is_valid_timestamp(timestamp))
    input_set_timestamp(udev->dev, timestamp);
input_event(udev->dev, ev.type, ev.code, ev.value);
```

So a **userspace-supplied timestamp is honoured if `is_valid_timestamp()` passes**
— which requires it to be (1) positive, (2) not in the future, (3) not older than
an allowed bound. If the injector supplies `0`/garbage (or a stale/future value),
the input core falls back to a kernel monotonic stamp at handling time.

Detection implications:

- If an injector passes `time = {0,0}` on every event, downstream `evdev` readers
  see kernel-stamped times — fine, indistinguishable in *value*, but the injector
  has no control over them.
- If an injector *does* set timestamps, any of the §2.3 tells (round numbers,
  exact fixed stride, all-identical, future-dated, implausibly regular to the µs)
  are now in a field a detector can read directly.
- evdev delivers timestamps in `CLOCK_REALTIME` by default, switchable per-fd to
  `CLOCK_MONOTONIC` via `EVIOCSCLOCKID`. Kernel `input.c` / evdev docs.

### 6.3 Device-name / identity heuristics `[PRIMARY]/[SECONDARY]`

- `uinput` devices are created under `/sys/devices/virtual/input/` and carry
  whatever `id` (bustype/vendor/product/version via `UI_DEV_SETUP`), `name`,
  `phys`, `uniq` userspace chose. Physical devices have a real `phys` (e.g.
  `usb-0000:00:14.0-2/input0`) and often a `uniq`; a naive virtual device leaves
  these blank or sets an obviously-fake `name`. `[PRIMARY]` libevdev uinput docs;
  who-t blog.
- libinput / Xorg deliberately **do not** filter virtual devices; a bespoke
  anti-cheat could enumerate `/sys/devices/virtual/input/` or match on
  `name`/`phys`, but no public game anti-cheat documents doing so. `[PRIMARY]`
  who-t blog ("all the common userspace … doesn't bother").
- This is an *origin* signal, not a *timing* signal — noted only because ticket 02
  asks. It reinforces the map's premise: Acheron's device is nameable and
  detectable by design.

### 6.4 What is NOT publicly documented

- No public game anti-cheat (VAC, BattlEye, EAC, Vanguard) documents specific
  `evdev`/`uinput` timing heuristics, threshold numbers, or device-name blocklists.
  BattlEye and EAC publish only high-level "heuristic / behavioural" language.
  Linux support for those anti-cheats (where it exists, e.g. via Proton) runs the
  Windows anti-cheat, which sees the X11/Wayland/`SDL` input layer, not raw evdev.
  `[SECONDARY]` inference.

---

## 7. Consolidated signal table

| Signal | Human range / shape | Reads as synthetic | Confidence |
|---|---|---|---|
| Sustained clicks/s, one button | 4–7 CPS; ceiling ~17 (jitter), ~35 (drag-abuse) | steady >10–15/s; >20/s w/o technique | `[SECONDARY]` |
| Minecraft implied CPS | ≤ ~16 | ≥16 first-pass; >20 (≥2 packets/tick); >60 (3/tick) | `[SECONDARY]`/`[OSS]` |
| Stimulus-driven action latency | median ~230–273 ms; elite ~200 | < ~150 ms, or "tens of ms after actionable" | `[PRIMARY]` (RT) / `[SECONDARY]` (rule) |
| Inter-event interval distribution | broad, skewed, non-stationary; drifts over session | single repeated value; symmetric ±uniform; few distinct values; multiples of 10 ms | `[PRIMARY]`+`[SECONDARY]` |
| Tap-timing SD (osu! UR) | UR > 50 (≳ 5 ms SD) | UR < 50 | `[OSS]` |
| Frame/loop interval (osu!) | median 15–16 ms, one broad peak | median < 13 ms, narrow peak | `[OSS]` |
| Opposite-key transition (CS2) | non-zero, variable overlap/neutral | hundreds of transitions at exactly 0 ms | `[PRIMARY]` (rule) |
| Key hold / dwell time | ~30–500 ms range, per-key distribution | all keys identical HT; HT < ~30 ms; ~zero spread | `[PRIMARY]` |
| Held-key repeat (Linux) | down(1) → 250 ms → repeats(2) @ ~33 ms, ms-scale jitter, ktime stamps | 1/0 pairs not value-2; no initial delay; period < live REP_PERIOD; µs-exact period | `[PRIMARY]` (kernel) |
| Held button repeat | exactly one 1 then one 0 | any intermediate/repeat events | `[PRIMARY]` (kernel) |
| Run-to-run macro identity | drifts; two runs never identical | byte/timing-identical repeats; replays within ~17 px (osu!) | `[OSS]` / `[SECONDARY]` |
| SYN framing | one logical event per SYN_REPORT | mis-framed / missing SYN / spurious SYN_DROPPED | `[PRIMARY]` |
| Injected timestamps | kernel ktime, monotonic, non-round | round / fixed-stride / identical / future-dated userspace stamps | `[PRIMARY]` |

---

## 8. Implications for Acheron macro authors

Plain best-practice framing. None of this is about hiding that Acheron uses
`uinput` — it always does, and that is fine. It is about not making a macro's
*timing* look like something no physical device or hand could produce, which is
what draws scrutiny (and, in games with explicit input-automation rules like CS2,
what the rule is actually about).

1. **Respect the kernel's own repeat rate for held / repeating output.** The map's
   bar — never emit key repeats faster than the live kernel `REP_DELAY`/`REP_PERIOD`
   (default 250 ms then ~33 ms) — is exactly the rate a physically held key
   produces. A macro that hammers a key faster than that is emitting something no
   keyboard on that machine would.

2. **Don't simulate a "hold" with a tight down/up loop.** A real held key sends
   *repeat* events (value 2) after an initial delay, not a stream of full
   press/release pairs. If you want auto-fire, space the presses at a
   human-plausible cadence, not at machine speed.

3. **Put realistic gaps between steps.** For keyboard steps, inter-key gaps in the
   ~50–200 ms band look like typing; sub-30 ms gaps and sub-30 ms key holds look
   like nothing physical. For controller-button steps Acheron already requires
   ≥ ~35 ms between Down and Up (one poll frame).

4. **Avoid perfectly uniform timing.** The most-cited tell everywhere is *zero
   variance* — every gap identical, or every gap an exact multiple of 10 ms, or a
   distribution that's a single spike. If a macro repeats an action, small natural
   unevenness in the authored delays is more plausible than a metronome. (This is
   not an instruction to build a jitter engine — just: don't go out of your way to
   make every delay mathematically identical.)

5. **Don't loop a macro thousands of times unattended.** Run-to-run identity and a
   session whose timing never drifts are named macro signatures. A macro is best
   used as a shortcut for a sequence you'd otherwise type, not as an endurance bot.

6. **Mind stimulus-response actions.** A macro that reacts to something on screen
   faster than ~200 ms isn't plausible as a human reaction. Acheron can't see the
   screen, but an author chaining "see X → macro fires" workflows should know that
   sub-150 ms response is a classic flag.

7. **In games with explicit input-automation rules, one physical press = one game
   action.** CS2's 2024 rule (and Vanguard's heuristics) target *one input driving
   multiple actions* and *impossibly clean* opposite-direction transitions. A macro
   that turns one keypress into a burst of many actions, or that produces perfect
   0 ms A↔D switches, is squarely what those rules describe — regardless of how the
   events are injected.

8. **This is guidance, not a guarantee.** Acheron's output is identifiable as
   synthetic by origin; these practices only keep a macro's *timing* within the
   range of "plausibly a person with a keyboard", which is the difference between
   an unobtrusive convenience macro and one that looks like a bot.

---

## Sources

Primary / high-trust:

- Linux kernel, `drivers/input/input.c` (autorepeat: `input_enable_softrepeat(dev,
  250, 33)`, `input_start_autorepeat`, `input_repeat_key`) —
  <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/input/input.c>
- Linux kernel, `drivers/input/misc/uinput.c` (`uinput_inject_events`, timestamp
  validation) — same tree, `drivers/input/misc/uinput.c`
- Linux kernel docs, `Documentation/input/event-codes.rst` (EV_KEY values 0/1/2,
  EV_SYN/SYN_REPORT/SYN_DROPPED, EV_REP) —
  <https://www.kernel.org/doc/html/latest/input/event-codes.html>
- Linux kernel docs, `Documentation/input/uinput.rst` —
  <https://www.kernel.org/doc/html/latest/input/uinput.html>
- Peter Hutterer, "The difference between uinput and evdev" —
  <http://who-t.blogspot.com/2016/05/the-difference-between-uinput-and-evdev.html>
- libevdev uinput API docs —
  <https://www.freedesktop.org/software/libevdev/doc/latest/group__uinput.html>
- Microsoft, `MSLLHOOKSTRUCT` / low-level hook injected flags —
  <https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msllhookstruct>
- Lotto, Marchiori, Conti, "QUACK! Making the (Rubber) Ducky Talk: A Systematic
  Study of Keystroke Dynamics for HID Injection Detection", arXiv:2604.15845 —
  <https://arxiv.org/abs/2604.15845>
- Keystroke Dynamics survey, arXiv:2303.04605 —
  <https://arxiv.org/html/2303.04605v3>
- Killourhy & Maxion, CMU Keystroke Dynamics Benchmark (DSN 2009) —
  <https://www.cs.cmu.edu/~keystroke/>
- Holoska & Doucek, "Detecting HID Keystroke Injection … Using Timing Signatures",
  SSRN 6443044 (abstract only; full text paywalled) —
  <https://papers.ssrn.com/sol3/papers.cfm?abstract_id=6443044>
- "Factors influencing the latency of simple reaction time", PMC4374455 —
  <https://www.ncbi.nlm.nih.gov/pmc/articles/PMC4374455/>
- US Patent Application US20230241511A1 (Activision), "Submovement-based mouse
  input cheating detection" (mouse, not keyboard — submovement count/duration/
  velocity/accel/jerk vs "human bounds"; near-zero-duration single submovement and
  constant acceleration as synthetic tells; no published thresholds) —
  <https://patents.google.com/patent/US20230241511A1/en>
- Valve, Counter-Strike 2 update 2024-08-19 (input-automation kick rule) —
  <https://www.counter-strike.net/news>; 2025 VALORANT Champions Tour ruleset
  (permits SOCD/Snap Tap) via <https://noping.com/blog/are-socd-or-snaptap-banned-in-valorant>

Open-source detectors (inspectable thresholds):

- circleguard / circlecore (osu!): `FRAMETIME_LIMIT = 13 ms`, `UR_LIMIT = 50`,
  replay similarity ~17 px, aim-correction `max_angle 10°` / `min_distance 8 px` —
  <https://circleguard.github.io/circlecore/using-circleguard.html>,
  <https://circlecore.readthedocs.io/en/v4.5.1/appendix.html>,
  <https://github.com/circleguard/circleguard/wiki/Frametime-Tutorial>
- osuReplayAnalyzer — <https://github.com/firedigger/osuReplayAnalyzer>
- Geo25rey/CPS-Limiter (Minecraft) — <https://github.com/Geo25rey/CPS-Limiter>

Secondary (directional only):

- crawlex, "Detecting automation via timing" —
  <https://blog.crawlex.net/blog/detecting-automation-via-timing/>
- Hypixel, "AutoClicker Detection Tutorial (server side)" —
  <https://hypixel.net/threads/autoclicker-detection-tutorial-server-side.5483042/>
- measurehuman.com "Average Click Speed"; Redragon / opauto-clicker CPS pages
- Human Benchmark reaction-time dataset — <https://humanbenchmark.com/tests/reactiontime>
- BattlEye site — <https://www.battleye.com/>
- GamingOnLinux / Engadget / esports.gg reporting on the CS2 2024 automation ban
- Hutterer-independent Hak5 DuckyScript docs (injection-tool timing: `DELAY`
  minimum 20 ms, `DEFAULT_DELAY`, optional `$_JITTER_ENABLED` / `$_JITTER_MAX`) —
  <https://docs.hak5.org/hak5-usb-rubber-ducky/ducky-script-basics/delays/>,
  <https://docs.hak5.org/hak5-usb-rubber-ducky/advanced-features/jitter/index.html>
