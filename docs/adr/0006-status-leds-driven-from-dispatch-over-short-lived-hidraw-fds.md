# Status LEDs are driven from the dispatch task over short-lived Interface-2 hidraw fds

The three side Status LEDs are set with a single Razer extended-matrix static-effect feature
report (`command_class 0x0F`, `command_id 0x02`, LED id `0x0B`) on the Tartarus Pro's
Interface-2 control node. We drive them from a dedicated non-fatal `led` task fed by the
dispatch task (the sole `Config` owner and Profile-switch decider), writing on a
freshly-opened, immediately-closed hidraw fd — **not** the analog capture layer's long-lived
handle (nothing holds one in Digital capture mode) — and **without** entering driver mode
(the `0x0F/0x02` frame works regardless, and the normal→driver transition resets this device
— see `.scratch/tartarus-status-leds/` tickets 01/02).

Considered and rejected:

- **Sharing the analog capture layer's Interface-2 handle.** Couples LED writes to the
  Capture mode: in Digital mode nothing holds a control handle, so the writer would have
  nothing to share half the time and would have to thread an `Arc<Mutex<File>>` across the
  supervisor↔dispatch boundary and handle "capture just swapped modes and dropped the handle"
  races. A short-lived fd, opened per write like `relock()` / `read_device_info()`, sidesteps
  all of it — the LED write is an occasional one-shot, not a hot path.
- **Letting the supervisor write on connect.** It would need Profile state it does not own.
  Dispatch owns `Config`, so dispatch decides what the LEDs show; the supervisor stays
  LED-agnostic.

The fd *lifetime* is a consequence of today's occasional one-shot writes (Profile switch,
device connect, clean-exit clear). A future host-streamed-animation feature would revisit the
lifetime — a persistent fd plus a render loop in the `led` task — without disturbing the
transport, the ownership, or the no-driver-mode choice.

All lighting frames — now and future — route through the one `led` task; the device has a
single control channel and frames must not interleave, so a second parallel writer opening
Interface 2 independently is disallowed.

This refines ADR-0002 ("OpenRazer remains available if lighting integration is ever wanted
later"): lighting integration is now wanted, and it is done directly over hidraw on the same
transport Acheron already uses for analog capture, not via OpenRazer.
