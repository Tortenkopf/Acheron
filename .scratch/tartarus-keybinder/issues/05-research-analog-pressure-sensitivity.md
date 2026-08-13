Type: research
Status: resolved

## Question

The Tartarus Pro's 20 grid keys are reported to support some kind of analog/pressure-sensitive input — Razer Synapse (Windows) can apparently detect how far down each key is pressed, not just binary up/down. Investigate how this actually works at the hardware/firmware/protocol level:

- Is it a real per-key analog signal (e.g. Hall-effect or optical analog switches reporting a continuous value), or is it inferred/simulated by Synapse from something like keystroke repeat frequency/timing rather than a true analog readout?
- What does the raw USB/HID protocol expose for this — is there a USB interface or HID report structure carrying analog data that OpenRazer isn't currently reading, separate from the three standard evdev-visible interfaces already identified (see map Notes)?
- Does OpenRazer's driver source, or its community/issue tracker, mention analog input for the Tartarus Pro (or sibling devices) at all?
- Bottom line: is this genuinely accessible from Linux userspace at all (even bypassing OpenRazer, e.g. via raw HID reports) — or is it Windows-driver/firmware-mode-specific and simply not present in the device's default Linux-visible protocol?

This almost certainly ends up out of scope for the MVP (see map Destination — Bindings currently model discrete presses only), but the map should record what was actually checked before ruling it out, since it's the kind of thing worth reopening later if the data turns out to already be sitting on the wire unused.

## Answer

It's real. The Tartarus Pro's 20 grid keys use genuine Razer Analog Optical switches (IR
light through the switch stem, read by a sensor) — Razer's own product and technology pages
confirm this specifically for the Tartarus Pro, distinct from Synapse inferring anything from
timing. The analog depth values (0–255 per key) travel over USB on a separate, undocumented
HID report (interface 1, endpoint `0x82`, report ID `0x06`) that is invisible on the three
evdev nodes already captured — it requires sending an undocumented mode-unlock command
(interface 2 / endpoint `0x83`) and reading raw `hidraw`, which a community project
(`open-tartarus-driver`) has already reverse-engineered. OpenRazer's own driver and PRs never
touch this data at all. So: real signal, *should* be reachable from Linux via raw HID — but only
via a nonstandard, undocumented vendor command, not via anything the daemon's evdev+uinput
design already touches. See [research/analog-pressure-sensitivity.md](../research/analog-pressure-sensitivity.md)
for full sourcing and the out-of-scope reasoning.

**Correction**: `open-tartarus-driver` is a Windows-only project (verified directly against the
repo after the user questioned it) — it does not run on or demonstrate this from Linux. The
protocol it documents is standard cross-platform HID, so Linux `hidraw` compatibility is a
reasonable inference, not a confirmed result. Full detail in the research file's "Correction"
note.
