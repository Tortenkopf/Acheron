Type: research

## Question

Sharpen [existing research](../tartarus-keybinder/research/analog-pressure-sensitivity.md) into a concrete Linux `hidraw` implementation plan for the Tartarus Pro's per-grid-key analog depth signal, as the first step of an open-ended attempt to close a genuine Linux gap (no existing Linux tool exposes this — see [Lock the v1.0 feature list](./08-decide-v1-feature-list.md)).

That prior research (done for the archived MVP map, where analog was correctly ruled out of scope for a discrete-remap-only effort) already establishes the bottom line: the signal is real hardware-level analog sensing (Razer Analog Optical Switches), sent as an undocumented HID Feature Report, reverse-engineered in detail by **`ultramonaka/open-tartarus-driver`** — but that project is Windows-only, and its documented protocol (Report ID `0x06` on Interface 1, the unlock command on Interface 2, byte layout, CRC) has never been verified against Linux `hidraw` or against this repo's own hardware.

Settle at least:

- **Read `ultramonaka/open-tartarus-driver`'s actual source** (not just its `research.md` prose, which the prior research file already summarized) — extract the exact byte sequences, HID report structures, and Windows API calls (`HidD_SetFeature` or equivalent) it uses for the unlock command and the analog read, straight from the Rust source.
- **Translate to Linux equivalents**: what `hidraw` ioctl (`HIDIOCSFEATURE` to send the unlock, plain `read()` on the Interface-1 hidraw node to receive Report `0x06`) and what device-node discovery (matching VID `0x1532`/PID `0x0244`, the correct `hidraw` interface among possibly several) this requires. Note any Linux-specific wrinkle Windows's HID stack wouldn't have (permissions on `/dev/hidraw*`, `udev` rules, kernel driver claiming the interface first).
- **Flag known risk**: the prior research notes a reported firmware reset/reconnect loop on some units after sending the unlock command (cross-referenced against an OpenRazer issue) — surface anything more specific found in `open-tartarus-driver`'s issue tracker or commit history about which firmware revisions are affected, so the prototype (blocked by this ticket) can approach it cautiously.
- Produce a concrete, implementation-ready translation (exact ioctl calls, byte buffers, report IDs) that [the follow-up prototype](./13-task-standalone-analog-capture-prototype.md) can build directly against, rather than re-deriving from Windows source at prototype time.

Does not touch Acheron's codebase — this is protocol investigation only, output is notes (append to or supersede the existing research file, or a new one in this map's own `research/` location — writer's judgment).
