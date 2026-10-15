# 09: Stop the small per-rebuild growth while the window is visible

Spec: [`../spec.md`](../spec.md). Filed from ticket 07's hardware verification.

**Found by:** ticket 07's second live session, with tickets 01–06 and 08 applied. While the main
window was visible, each Mode-key press and release (one `ActiveLayerChanged`, one full Device
Overview rebuild) grew the GUI's RSS by about **0.085 MB**. The growth was linear and didn't level
off:

- +13 MB over 160 Layer signals (13:38:53–13:39:06)
- +5.6 MB over 65 Layer signals (13:39:36–13:39:42)

The scripted run's 20 visible Profile switches showed the same rate, about +0.07 MB per
rebuild. That's roughly 500× less than the original 44 MB per rebuild, but 1,000 visible taps
still add about 85 MB. While the window is hidden, nothing grows: rebuilds are deferred
(ticket 06).

**What we know:** `test_rebuild_soak.py` (ticket 05) pins that no widget from an old build is
alive and that the live `Gtk.Widget` wrapper count stays flat across rebuilds. It runs
headless, though, where nothing is mapped or rendered. So the residue is something that exists
only for a realized, mapped window. Unconfirmed candidates:

- GSK render nodes or textures
- glyph or Pango layout caches keyed per widget
- CSS style nodes
- accessibility (AT-SPI) objects
- the C allocator not returning freed memory (fragmentation rather than a leak)

The spec's Out of Scope already names the structural alternative: update Device Overview in
place on a Layer change instead of rebuilding.

**What to build:** first find where the per-rebuild growth lives, measured against a real mapped
window, not headless. Then either:

- (a) fix it, so visible rebuilds hold RSS roughly flat over hundreds of Layer changes, or
- (b) if it's allocator fragmentation or a GTK-internal cache that plateaus, show that it
  plateaus and record that conclusion here, or
- (c) if neither is practical, propose the in-place Layer update (out of scope in the parent
  spec) as its own spec for the user to decide on.

**Blocked by:** None.

**Status:** ready-for-human (2026-10-15): the in-process rebuild path levels off, but the live growth isn't reproduced; one live re-run is needed (see Comments)

- [x] A repeatable measurement on a real mapped window: N Device Overview rebuilds with RSS
      sampled, driven without the physical Mode key (e.g. in-process rebuild calls, or
      `SwitchProfile` on a running Daemon, as ticket 07's scripted harness did).
- [ ] The growth is attributed to a cause, with evidence. Candidate tools: `tracemalloc` for
      the Python heap, `GOBJECT_DEBUG=instance-count` / `GTK_DEBUG=` for GObject types, and
      `malloc_trim(0)` to test the fragmentation theory.
- [ ] Outcome (a), (b) or (c) above, recorded in a `## Comments

- 2026-10-15: Investigated. **The rebuild path levels off on a mapped window, but that doesn't
  account for ticket 07's live figures.** So the attribution and outcome boxes stay open until
  one live re-run (below) decides between (b) and a new suspect.

  **Measurement:** `gui/tools/soak_visible_rebuild.py` runs the real `AcheronApplication` with
  its window presented and painting (Wayland). It drives one full Device Overview rebuild per
  `ActiveLayerChanged` callback (press/release alternating) and samples RSS. With `--live-daemon`
  it reads the real config and state over D-Bus from the running Daemon. Its output suppression
  and Toggle stops are no-ops, and the Layer callback is called in-process. Control flags:
  `--hidden`, `--idle`, `--trim` (`malloc_trim(0)` before each sample), `--tracemalloc` and
  `--pause-at`. `GSK_RENDERER` passes through. The figures below are from the sampled lines.
  "First ~150" starts at the first sample, after 20 warm-up rebuilds, except in the live-pattern
  row.

  | Run (real config over D-Bus unless noted) | First ~150 rebuilds | After that |
  |---|---|---|
  | `DaemonStub` config, 40 ms pace, 400 rebuilds | +1.6 MB | +0.2 MB over 350 |
  | 40 ms pace, 400 rebuilds | +3.3 MB | +0.2 MB over 350 |
  | 120 ms pace, 1,200 rebuilds | +13.9 MB | +0.4 MB over 1,050 |
  | 120 ms, `GSK_RENDERER=cairo` / `gl` / `vulkan` | +15.4 / +12.9 / +15.4 MB | +0.2 / +0.1 / +0.1 MB over 300 |
  | 120 ms, `--trim` | +4.2 MB | +0.2 MB over 300 |
  | 120 ms, `--tracemalloc` | Python heap 0.5 MB | 0.5 MB, unchanged |
  | 120 ms, `--idle` (visible, never rebuilt) | +0.6 MB | flat |
  | 120 ms, `--hidden` (deferred, ticket 06) | +0.2 MB | flat |
  | **Live pattern, ×3:** 85 ms, sampled from rebuild 1, 160 → 30 s pause → 65 | **+4.1 / +4.3 / +4.4 MB, all in the first 40** | **second burst: −0.5 / +0.1 / +0.1 MB** |

  **What this shows** (for rebuilds driven in-process on a mapped window):
  - **Nothing grows without bound.** Every run levels off within ~40–150 rebuilds, and 1,000+
    later rebuilds add under 0.5 MB. Pauses don't restart the growth.
  - **It isn't Python, or a particular renderer.** The traced Python heap stays flat, and the
    plateau is about the same under cairo, GL and Vulkan.
  - **It needs rebuilds and a mapped window.** An idle visible window and a hidden window both
    stay flat.
  - **Most of it is freed heap glibc keeps.** `malloc_trim(0)` cuts the 120 ms plateau from about
    14 MB to about 4 MB. The ~4 MB left is a guess, not a measurement: likely C-side caches filled
    on first realize (Pango/fontconfig, CSS). None of the following were run, so they aren't ruled
    out *as contents*:
    - `GOBJECT_DEBUG=instance-count` (needs a debug GLib)
    - CSS style nodes
    - AT-SPI objects
    
    They are only ruled out as an unbounded leak.
  - **The pace effect is unexplained.** The plateau height ranges from 2 to 15 MB depending on pace
    and run: slower pacing gives a higher plateau. That fits allocator high-water behaviour, but
    nothing measured confirms it.

  **What it doesn't explain:** live session 2 grew **+13 MB over its first 160 signals** (about
  81 ms per signal) and **+5.6 MB over the next 65** (after a pause). At that pace the harness
  grows only ~4 MB, and its second burst adds nothing, in all three runs. So the live growth
  includes something this harness doesn't exercise. The candidates, all untested:
  - **Real `ActiveLayerChanged` delivery over D-Bus.** A fake signal can't be injected: the
    `Gio.DBusProxy` only accepts signals from the Daemon's own name owner.
  - **Physical Mode-key input** while the GUI is focused.
  - **The real `TrayIcon`.** Its `update()` should be a no-op on a Layer change since ticket 08,
    but that wasn't measured here.
  
  Matching the live +13 MB to the 120 ms run's +13.9 MB (an earlier draft of this comment) was
  a pace mismatch and doesn't hold.

  **Next step (needs the user and the Tartarus):** run the installed or checkout GUI visible,
  sample its RSS once per second (`ps -o rss= -p <pid>`), and tap the Mode key in three bursts
  with ~30 s pauses: ~150, ~150, ~300 signals.
  - If the third burst adds ~0 MB, it's a plateau: record **(b)** here.
  - If it keeps adding ~0.085 MB per signal, the leak is on the live signal path, not the
    rebuild. Bisect it next: run the GUI with the real Daemon but disable the tray icon (one
    line), then compare.
