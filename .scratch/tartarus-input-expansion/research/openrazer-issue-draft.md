# Draft: OpenRazer issue — Tartarus Pro set_device_mode reset may be a transaction_id artifact

Not sent yet. Drafted from findings in
[linux-analog-grid-key-protocol.md](linux-analog-grid-key-protocol.md) §2, §5. Holding off on
filing: enabling analog for the grid buttons isn't worth anything to OpenRazer on its own
without a companion app that does something with the values — which, as far as we can tell,
is currently just us. Revisit if that changes (e.g. someone picks #1868 back up, or another
Tartarus Pro owner reports the same reset).

## Existing OpenRazer issues/PRs referencing the Tartarus Pro

- Issue [#1039](https://github.com/openrazer/openrazer/issues/1039) (61 comments) — general Tartarus Pro support request/tracking
- Issue [#1177](https://github.com/openrazer/openrazer/issues/1177)
- PR [#2336](https://github.com/openrazer/openrazer/pull/2336) (17 comments)
- PR [#2622](https://github.com/openrazer/openrazer/pull/2622)
- PR [#2710](https://github.com/openrazer/openrazer/pull/2710) — the merged support PR with the `DRIVER_MODE = False` / probe carve-out this issue would respond to
- PR [#1868](https://github.com/openrazer/openrazer/pull/1868) (unmerged) — Linux analog driver-mode support for the Huntsman Mini Analog, useful prior art for anyone picking this back up

None of #1039/#1177/#2336/#2622 mention a reset/reconnect/disconnect/loop/crash — #2710 is the
sole public source of the reset report.

## Draft issue text

```markdown
Title: Tartarus Pro set_device_mode reset (from #2710) may be a transaction_id artifact, not the command itself

## Summary

#2710 disables `DRIVER_MODE` for the Tartarus Pro and skips `razer_set_device_mode()` in the
kernel probe, citing a firmware reset triggered by the set-device-mode command. We independently
tested the same command on our own Tartarus Pro (firmware v1.2) from userspace, using a
different `transaction_id`, and saw no reset across multiple unlock/re-lock cycles. Posting this
in case it's useful if analog/driver-mode support for this device is ever revisited (e.g. #1868).

## What we tested

Sent the standard 91-byte `razer_report` feature report directly via `hidraw`/`HIDIOCSFEATURE`
to Interface 2 (`0003:1532:0244.000D`), bypassing OpenRazer entirely (module still loaded and
bound to all three interfaces at the time — no unbind needed, per `HID_CONNECT_HIDRAW`/raw_event
pass-through behavior).

- `command_class = 0x00`, `command_id = 0x04` (set device mode), same as
  `razer_chroma_standard_set_device_mode()`
- Mode `0x03` (unlock) and mode `0x00` (re-lock), two full cycles each
- **`transaction_id = 0x01`** — differs from both paths in the current driver:
  - kernel `razer_set_device_mode()`: `0x1F`
  - sysfs `device_mode` write attribute: `0xFF`

| Source | `transaction_id` | Outcome on our unit (fw v1.2) |
|---|---|---|
| Kernel `razer_set_device_mode()` | `0x1F` | Reported reset (probe crash, #2710) |
| sysfs `device_mode` write attr | `0xFF` | Reported reset (daemon detection loop, #2710) |
| Our test | `0x01` | No reset, either direction, across 2 cycles |

`0x01` is what Synapse itself sends on Windows (per `ultramonaka/open-tartarus-driver`'s capture:
[`main.rs:660`](https://github.com/ultramonaka/open-tartarus-driver/blob/HEAD/tartarus_driver/src/main.rs#L660)),
and that project also reports never reproducing a reset across extensive testing with the same
`transaction_id`. Notably, that project's own `lighting.rs` documents `0x1F` and `0x01` as two
*separately* confirmed transaction ids for different command classes on this device — so it's
plausible the kernel/sysfs paths are using a lighting-class id for a device-mode-class command.

## Verification

Read back `device_mode` after each send: `00 00` after re-lock, and the analog report stream
(Report ID `0x06`, Interface 1) started/stopped correctly after unlock/re-lock respectively —
so the command is doing what's intended, not merely failing silently.

## What this does and doesn't show

This is one unit, one firmware revision (v1.2), a handful of cycles, and it only exercises the
unlock/re-lock round trip — we have not tested the kernel-side driver-mode data path (`ABS_*`
event emission, as in #1868) end-to-end. It's not a claim that #2710's carve-out is wrong, just
a second, contradicting data point on top of #2710's single-contributor report — worth someone
re-testing with `transaction_id = 0x01` before treating "this command resets the Tartarus Pro"
as settled, especially if analog support for this device gets picked back up.

Happy to share our probe code/exact byte sequence if useful.
```
