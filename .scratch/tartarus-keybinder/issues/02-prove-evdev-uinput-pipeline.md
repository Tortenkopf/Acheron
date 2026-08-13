Type: task
Status: resolved

## Question

Build a minimal spike that proves the core mechanism the Daemon depends on: grab one of the Tartarus Pro's evdev nodes exclusively, and on receiving a keypress, inject a *different* key via a `uinput` virtual device.

Confirm with the user that the injected key actually lands somewhere real (e.g. appears in a text field) — this also verifies that `uinput` write access works for this user without extra permission setup (unconfirmed per the map's Notes; only read access has been verified so far). Language for the spike doesn't need to be the final Rust Daemon — whatever proves the mechanism fastest is fine — but note in the answer whether anything about it should change the language/library choice already recorded in docs/adr/0003.

## Answer

Live-hardware session, 2026-08-12. Spike script (Python, `python-evdev`): [assets/prove-evdev-uinput-pipeline-spike.py](../assets/prove-evdev-uinput-pipeline-spike.py). It grabbed `if01` exclusively (`EVIOCGRAB`), watched for the grid key at row 2/col 2 (`KEY_Q`, per [Enumerate physical inputs](./01-enumerate-physical-inputs.md)), and on each press/release relayed a *different* key (`KEY_B`) through a virtual `uinput` device.

User pressed that physical key with focus in a real text field: **"b" appeared, never "q"** — confirming both that the exclusive grab suppresses the real event and that the injected event lands correctly. Log showed exactly one relayed press and one relayed release, matching the physical press.

**`uinput` write access**: works for this user (`justin`) without extra setup — `/dev/uinput` already carries an explicit ACL (`user:justin:rw-`, mode `660`), not just group ownership. So no udev rule or capability grant is needed for this specific machine/user; worth re-checking on a fresh install when the packaging/first-run ticket comes up, since this ACL's origin wasn't investigated (could be pre-existing system config rather than something the app installer would need to set up itself).

**Language/library choice (docs/adr/0003)**: no change warranted. The mechanism proved here — `EVIOCGRAB` on the evdev node, `write()`+`ioctl` to `/dev/uinput` — is a kernel-level interface, not a Python-specific one; Rust has equivalent crates (`evdev`, `uinput`) that call the same ioctls. This spike only proves the *permission model and mechanism* work on this system, which is language-agnostic. ADR-0003's Rust Daemon choice stands.
