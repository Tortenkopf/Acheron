<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# Acheron

**An open keybinding tool for the Razer Tartarus Pro.**

---

> **Disclaimer:** This is an independent, community-developed project, not
> affiliated with, endorsed by, or supported by Razer Inc. "Razer" and
> "Tartarus" are trademarks of Razer Inc. Provided as-is, with no warranty
> (see the [Licence section](#licence)).

---

Acheron remaps the Tartarus Pro's keys and builds macros, layers, and profiles
for it on Linux and enables the use of the Tartarus Pro's analog keys.
It talks to the device directly through the Linux kernel (`evdev` in, `uinput` out).

---

> The Acheron (/ˈækərən/) is a river in the Epirus region of northwest Greece.
> …
> Ancient Greek mythology saw the Acheron, sometimes known as the "river of
> woe", as one of the five rivers of the Greek underworld.
> …
> The Suda describes the river as "a place of healing, not a place of punishment,
> cleansing and purging the sins of humans".
>
> — [Wikipedia](https://en.wikipedia.org/wiki/Acheron)

---

## What it does

Acheron is two cooperating pieces:

- a **Daemon** (Rust) — a background `systemd --user` service that captures the
  device and injects the remapped output. It runs whether or not the GUI is
  open, and owns the single config file.
- a **GUI** (Python + GTK 4) — edits profiles, bindings, macros and steppers,
  and shows live daemon/device status. It configures the Daemon over D-Bus and
  never touches the config file itself.

### Features

- **Profiles** — named, complete binding sets you switch between manually.
  The active Profile drives the Tartarus Pro's Status LEDs as indicators.
- **Layers** — hold the Mode key for a second full set of bindings per Profile
  (Razer calls this "Hypershift").
- **Bindings** from any Input — the 20 grid keys, the Mode key, the four
  thumbstick directions, and the scroll wheel's up/down/click.
- **Actions**: a **Keypress** (any key or modifier combination), a **Macro**,
  a **Stepper** step, a **Profile Switch**, or a **Controller button**.
- **Output picker** covering the whole keyboard — letters, numbers, function
  keys through F24, the numpad, navigation/lock keys, and multimedia/consumer
  keys — plus the five mouse buttons (left / middle / right / back / forward)
  and a virtual gamepad's buttons.
- **Chords** — bind a *set* of Inputs pressed together to their own Action
  (this is also how the thumbstick's four diagonals work).
- **Trigger modes**: Fire-once, Hold-to-repeat, Toggle, and **Analog-repeat**
  ("Simulated Analog Key-Interlacing" — re-fire rate rises with how hard a grid
  key is pressed, for keyboard-driven driving sims).
- **Macro library** and **Stepper library** — named, reusable sequences and
  ordered lists (a Stepper walks a cursor through a list, firing each item as
  you step; ideal on the scroll wheel for weapon wheels or hotkey pages).
- **Analog grid keys** — on hardware that supports it, per-key actuation and
  release points with a live depth readout, and continuous **axis output**
  (a grid key drives a gamepad trigger or stick half by pressure).
- **Mouse-button hold** — Hold-to-repeat on a mouse button is a real sustained
  press, so click-and-drag works.
- **System tray icon** — active profile/layer at a glance, quick profile
  switching, and pause/resume of the Daemon.
- **Plain TOML config** at `~/.config/acheron/config.toml` — hand-editable and
  easy to back up.

An unbound Input passes its normal keycode through unchanged, so you only
configure the controls you care about.

### What it is not

- Not a lighting/RGB tool — OpenRazer already covers that for this device.
- Not an automatic per-application profile switcher — profile switching is
  always manual.
- Not a general remapper — it models *this* device specifically. It is written
  to make adapting it for other Tartarus variants possible later, but only the
  Tartarus Pro is supported and tested today.

## Hardware

**Razer Tartarus Pro** (USB `1532:0244`) only. This is the one device the
author owns and tests against. Other Tartarus models (V2, Chroma) are not
supported.

Analog features (per-key actuation points, analog axis output, Analog-repeat)
use the Tartarus Pro's pressure-sensitive optical switches. If the Daemon can't
reach the analog interface it falls back automatically to digital capture — every
key still works, just without pressure.

## System requirements

- **Linux** with **systemd** (the per-user instance) and a **D-Bus session bus**.
- Primary target, fully tested: **Ubuntu + GNOME Shell + Wayland**. KDE Plasma
  and XFCE are expected to work but are not regularly tested. On **GNOME** the
  tray icon needs the *AppIndicator and KStatusNotifierItem Support* extension
  (`gnome-shell-extension-appindicator`) — GNOME has no built-in tray.
- Membership of the **`plugdev`** group (most desktop distros already add your
  login user). This is what lets the Daemon reach the analog interface without
  root.
- To **build the Daemon**: a **Rust toolchain, 1.85 or newer** (the crate uses
  edition 2024), with `cargo`. [rustup](https://rustup.rs/) is the easy path;
  a distro `rustc`/`cargo` that new works too.
- To **run the GUI**: **Python 3.9+**, **PyGObject** with **GTK 4**, and
  **dasbus**.

On Ubuntu:

```sh
sudo apt install build-essential python3-gi gir1.2-gtk-4.0 python3-dasbus
# plus a Rust toolchain — rustup, or:  sudo apt install cargo
```

## Install

Acheron installs from a git checkout with one script. There is no distro
package.

```sh
git clone <repo-url> acheron
cd acheron
./install.sh
```

`install.sh` is idempotent — re-run it after every `git pull` to rebuild and
reinstall. It:

1. builds the release Daemon and installs it to `~/.local/bin/acheron-daemon`;
2. installs and enables the `systemd --user` unit, so the Daemon starts now and
   at every login;
3. installs a **udev rule** to `/etc/udev/rules.d/` — **this step asks for
   `sudo`**. It grants the `plugdev` group access to the Tartarus Pro's
   `hidraw` interfaces, which analog capture needs. If you decline or it fails,
   the install continues and the Daemon still runs (digital capture only); the
   script prints the manual commands to finish it later.
4. installs the GUI package, the `acheron-gui` launcher, a desktop entry, and
   icons under `~/.local`.

Make sure **`~/.local/bin` is on your `PATH`** (standard on modern desktop
distros) — the app-grid entry runs `acheron-gui` from there.

After installing, log out and back in once so the udev rule and the group
membership take full effect (or unplug/replug the device).

### Building a release

Both components self-label their version from git. A plain `main` checkout
reports `1.1.0-dev+<short-hash>`; a checkout sitting exactly on the `v1.1.0`
tag (or a tarball with no `.git`) reports the bare `1.1.0`. **Tag the release
commit before building** the artifacts you hand to users. The canonical
version numbers live in `daemon/Cargo.toml` and `gui/acheron_gui/__init__.py`
(`_BASE_VERSION`); a release bumps both. `daemon/build.rs` honours an explicit
`ACHERON_VERSION` environment variable if a packager needs to pin the string.

### Installed files

| Path                                                | What                                                    |
| --------------------------------------------------- | ------------------------------------------------------- |
| `~/.local/bin/acheron-daemon`                     | Daemon binary                                           |
| `~/.local/bin/acheron-gui`                        | GUI launcher script                                     |
| `~/.config/systemd/user/acheron-daemon.service`   | systemd --user unit                                     |
| `/etc/udev/rules.d/60-acheron-tartarus-pro.rules` | analog-access udev rule (root-owned)                    |
| `~/.local/lib/acheron/acheron_gui/`               | installed GUI package (incl. a bundled`LICENSE`)      |
| `~/.local/share/applications/acheron.desktop`     | desktop entry                                           |
| `~/.local/share/icons/hicolor/*/apps/acheron.png` | app icons                                               |
| `~/.local/share/acheron/tray-icons/*.svg`         | tray status icons                                       |
| `~/.config/acheron/config.toml`                   | your configuration (created by the Daemon on first run) |

### Uninstalling

`install.sh` has no uninstall mode. To remove Acheron:

```sh
systemctl --user disable --now acheron-daemon
rm ~/.local/bin/acheron-daemon ~/.local/bin/acheron-gui
rm ~/.config/systemd/user/acheron-daemon.service
rm -rf ~/.local/lib/acheron
rm ~/.local/share/applications/acheron.desktop
rm ~/.local/share/icons/hicolor/*/apps/acheron.png
rm -rf ~/.local/share/acheron
sudo rm /etc/udev/rules.d/60-acheron-tartarus-pro.rules
# optional — your bindings:
rm -rf ~/.config/acheron
```

## Usage

The Daemon runs from login. Launch the GUI from your app grid ("Acheron") or
run `acheron-gui`.

- **Device Overview** mirrors the physical pad. Click any control to open its
  binding editor. The **Base / Held** tabs are the Mode-key layer; **Profiles**
  are the left sidebar. Unbound controls show "passthrough".
- The binding editor picks the **Action** and **Trigger mode**, and opens the
  output picker for keys, mouse buttons, or gamepad buttons. Grid keys also get
  an **Actuation & release** section with a live depth bar when analog capture
  is active.
- **Chords**: turn on "Select Chord members", click two or more controls on the
  grid, then "Binding →" to give the set an Action.
- **Library** (the Grid / Library switch) holds your named **Macros** and
  **Steppers**. Edits there save automatically. Assign them to a control from
  the binding editor.
- The **tray icon** shows the active profile and layer, switches profiles, and
  pauses/resumes the Daemon. Closing the main window hides it to the tray; use
  the tray's **Quit** to actually exit the GUI (the Daemon keeps running).
- **About Acheron** (header-bar menu, top right) shows the version, the
  connected device's firmware and serial number, acknowledgements, and the
  licence.

Editing is blocked with an on-screen reason whenever the Daemon isn't running
or the device isn't connected, so a change never looks applied when it can't be.

### Configuration file

The Daemon owns `~/.config/acheron/config.toml` exclusively and rewrites it on
every change made through the GUI. You can hand-edit it, but **stop the Daemon
first** (`systemctl --user stop acheron-daemon`) or your edit will be
overwritten. If the file is unparseable the Daemon refuses to start rather than
discard it — check `systemctl --user status acheron-daemon` for the error.

### After a rebuild

`install.sh` replaces the binary but does not restart a running Daemon. Pick up
a new build with:

```sh
systemctl --user restart acheron-daemon
```

## Troubleshooting

- **Bindings do nothing.** Check `systemctl --user status acheron-daemon` and
  that the device shows connected in the GUI or the tray.
- **No tray icon on GNOME.** Install and enable the *AppIndicator and
  KStatusNotifierItem Support* GNOME extension.
- **`acheron-gui: command not found`** from the app grid — `~/.local/bin` is
  not on your `PATH`.
- **Analog features unavailable / "digital capture".** The udev rule isn't in
  effect yet, or you're not in `plugdev`. Re-run the `sudo` commands
  `install.sh` printed, confirm `groups` lists `plugdev`, then log out and back
  in.
- **Daemon won't start after a hand-edit.** The error is in
  `systemctl --user status acheron-daemon` / `journalctl --user -u acheron-daemon`.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the layout, how to run the test
suites, and the domain vocabulary. In short: `cargo test` in `daemon/`,
`pytest` in `gui/`, and `CONTEXT.md` / `docs/adr/` for the design.

## Licence

Acheron is free software under the **GNU General Public License, version 3 or
(at your option) any later version**. See [LICENSE](LICENSE).

Copyright © 2026 Justin Milatz.

This program comes with ABSOLUTELY NO WARRANTY. You are free to change and
redistribute it under the terms of the GPL.

## Acknowledgements

- [**open-tartarus-driver**](https://github.com/ultramonaka/open-tartarus-driver)
  by ultramonaka — the analog-mode protocol reference.
- [**Matt Pocock's skills**](https://github.com/mattpocock/skills) — the agent
  workflow used to build this.
