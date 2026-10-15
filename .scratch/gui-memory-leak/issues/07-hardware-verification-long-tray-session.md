# 07: Verify on hardware — long session in the tray

Spec: [`../spec.md`](../spec.md). The live check behind the user's original report: memory grew
over hours with the GUI in the tray while gaming.

**What to build:** Confirm on the real Tartarus Pro, Daemon and installed GUI that memory use
stays flat in the scenario that used to stall the system.

**Blocked by:** 02, 03, 04, 05, 06.

**Status:** done (2026-10-15)

- [x] Note the GUI process's RSS. Hide the GUI to the tray, tap the Mode key heavily (hundreds of
      presses) and switch Profiles a few times. RSS stays roughly flat (it was about 88 MB per tap
      before).
- [x] Reopen from the tray: Device Overview shows the current Layer, Profile and device status at
      once.
- [x] With the window visible, tap the Mode key repeatedly: the grid follows the Layer and RSS
      stays roughly flat.
- [x] Open and close a grid key's Binding editor many times, including Save and Apply commits,
      and some Lighting colour pickers: RSS stays roughly flat, and the editors behave as before.
- [x] Record the before/after RSS figures in a `## Comments` section on this ticket.

## Comments

- 2026-10-15: Done, on the real Tartarus Pro with Daemon 1.3.4 (no Daemon changes since). "Before"
  is the installed pre-fix GUI (`~/.local/lib/acheron`); "after" is `dev`, run from the checkout.
  The installed copy still needs `install.sh` to pick the fix up.

  **Scripted run** (the real `AcheronApplication` in-process, hidden through its own close→tray
  path, with rebuilds driven by `SwitchProfile` on the real Daemon):

  | | Before | After |
  |---|---|---|
  | At launch | 260 MB, 57 top-level windows | 161 MB, 1 |
  | Hidden, Profile switches | +263 MB over 6 (≈44 MB and 28 windows each) | +0.2 MB over 20 |
  | Visible, Profile switches | +256 MB over 6 | +1.4 MB over 20 |
  | Editor open/close (grid key 1) | 421 windows by the end | rises ~50 MB over the first ~40 cycles, then flat at 224 MB through 120 |

  **Live session 1** (the user tapping the Mode key, window hidden): about 960
  `ActiveLayerChanged` signals left RSS at 177→214 MB. The memory leak is fixed. But the tray
  menu's revision had reached 962: the tray sent `LayoutUpdated`/`NewIcon`/`NewTitle`/`NewStatus`
  on every Layer edge. The tray menu stopped opening, and then the whole desktop stopped taking
  clicks until the GUI was killed. This existed before the leak fix and was masked by it. Fixed
  by **ticket 08**.

  **Live session 2** (with ticket 08; all four checks done by the user, nothing misbehaved):
  - Hidden: 269 Layer signals, RSS 175.1→177.6 MB. Flat.
  - Profile switches from the tray work. Reopening from the tray shows the current state.
  - The tray revision reached only 13 over 524 Layer signals and 11 Profile signals.
    gnome-shell peaked at 41% CPU, and the desktop stayed responsive throughout.
  - Visible Mode-key taps: about **+0.085 MB per Layer signal**, linear: +13 MB over 160 signals,
    +5.6 MB over 65. That's about 500× less than the old 44 MB, but not zero. The headless soak
    tests show no surviving widgets or wrappers, so the residue is outside what they measure,
    probably renderer or GSK resources of a mapped window. A candidate follow-up if visible
    Mode-key use matters.
  - Editors (including Save/Apply) and Lighting colour pickers: +35 MB during the burst, flat
    afterwards. This matches the scripted run's warm-up plateau.
