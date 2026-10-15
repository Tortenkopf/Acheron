# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""RSS soak for Device Overview rebuilds on a real, mapped window
(gui-memory-leak ticket 09).

`gui/tests/test_rebuild_soak.py` runs headless, where nothing is realized or
rendered. This harness runs the real `AcheronApplication` against a
`DaemonStub`, presents its window, and drives Layer changes through the
stub's Mode-key seam (each one an `ActiveLayerChanged` and one full rebuild),
letting the frame clock paint between them. It samples RSS every `--every`
rebuilds and prints one line per sample.

Usage:  python3 gui/tools/soak_visible_rebuild.py [--rebuilds N] [--every K]
            [--interval MS] [--trim] [--tracemalloc] [--hidden]
            [--live-daemon] [--idle]

  --trim         call glibc `malloc_trim(0)` before each sample, to separate
                 freed-but-unreturned heap (fragmentation) from live memory
  --tracemalloc  also report the Python heap's traced size per sample
  --hidden       keep the window hidden (control: rebuilds are deferred)
  --live-daemon  read config and state from the running Daemon over D-Bus
                 (read-only; Layer changes are still faked in-process)
  --idle         run the same clock with no Layer changes at all (control:
                 a visible window that is never rebuilt)

Run it under a real session (Wayland or X11). Environment knobs such as
`GSK_RENDERER=cairo` or `GOBJECT_DEBUG=instance-count` pass straight through.
"""

from __future__ import annotations

import argparse
import ctypes
import gc
import sys
import time
import tracemalloc
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gio, GLib  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import acheron_gui.app as _app_mod  # noqa: E402
from acheron_gui.app import AcheronApplication  # noqa: E402
from acheron_gui.daemon_client import DBusDaemonClient  # noqa: E402
from acheron_gui.daemon_stub import DaemonStub  # noqa: E402


class _NoTray:
    def __init__(self, *_a, **_kw) -> None:
        pass

    def update(self, *_a, **_kw) -> None:
        pass


_app_mod.TrayIcon = _NoTray


class _FakeSystemd:
    def ensure_daemon_started(self) -> None:
        pass

    def stop_daemon(self) -> None:
        pass

    def start_daemon(self) -> None:
        pass


class _ReadOnlyDaemon:
    """The real `DBusDaemonClient`, for its real config and its real
    GLib.Variant unpacking, minus anything that changes the running Daemon:
    focus-driven output suppression and Toggle stops become no-ops. Layer
    changes are faked locally (the live Layer needs the physical Mode key),
    so every subscribed Layer callback is held here and driven by
    `simulate_mode_key_press`/`_release`, the same seam `DaemonStub` has."""

    def __init__(self) -> None:
        self._real = DBusDaemonClient()
        self._layer_callbacks: list = []

    def __getattr__(self, name):
        return getattr(self._real, name)

    def set_output_suppressed(self, _suppressed: bool) -> None:
        pass

    def stop_all_toggles(self) -> None:
        pass

    def subscribe_layer_changed(self, callback) -> None:
        self._layer_callbacks.append(callback)

    def simulate_mode_key_press(self) -> None:
        for callback in self._layer_callbacks:
            callback("held")

    def simulate_mode_key_release(self) -> None:
        for callback in self._layer_callbacks:
            callback("base")


_libc = ctypes.CDLL("libc.so.6")


def _rss_mb() -> float:
    with open("/proc/self/statm") as fh:
        pages = int(fh.read().split()[1])
    return pages * 4096 / 2**20


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rebuilds", type=int, default=400)
    parser.add_argument("--every", type=int, default=50)
    parser.add_argument("--interval", type=int, default=40, help="ms between Layer changes")
    parser.add_argument("--warmup", type=int, default=20, help="rebuilds before the first sample")
    parser.add_argument("--trim", action="store_true")
    parser.add_argument("--tracemalloc", action="store_true")
    parser.add_argument("--hidden", action="store_true")
    parser.add_argument("--live-daemon", action="store_true")
    parser.add_argument("--idle", action="store_true")
    args = parser.parse_args()

    if args.tracemalloc:
        tracemalloc.start()

    stub = _ReadOnlyDaemon() if args.live_daemon else DaemonStub()
    app = AcheronApplication(client=stub, systemd_client=_FakeSystemd())
    app.set_application_id("com.acheron.gui.soak09")
    app.set_flags(Gio.ApplicationFlags.NON_UNIQUE)

    done = {"n": 0}
    t0 = time.monotonic()
    baseline: dict = {}

    def sample() -> None:
        gc.collect()
        if args.trim:
            _libc.malloc_trim(0)
        rss = _rss_mb()
        line = f"rebuild {done['n']:5d}  rss {rss:8.1f} MB"
        if baseline:
            delta = rss - baseline["rss"]
            per = delta / max(1, done["n"] - baseline["n"])
            line += f"  Δ {delta:+7.1f} MB  ({per * 1024:+6.1f} KB/rebuild)"
        else:
            baseline.update(rss=rss, n=done["n"])
        if args.tracemalloc:
            line += f"  py-heap {tracemalloc.get_traced_memory()[0] / 2**20:7.1f} MB"
        print(line, flush=True)

    def step():
        win = app.get_active_window()
        if args.hidden and win.get_visible():
            win.set_visible(False)
        if args.idle:
            pass
        elif done["n"] % 2 == 0:
            stub.simulate_mode_key_press()
        else:
            stub.simulate_mode_key_release()
        done["n"] += 1
        if done["n"] == args.warmup or (done["n"] > args.warmup and (done["n"] - args.warmup) % args.every == 0):
            sample()
        if done["n"] >= args.rebuilds + args.warmup:
            print(f"done in {time.monotonic() - t0:.1f}s", flush=True)
            app.quit()
            return GLib.SOURCE_REMOVE
        return GLib.SOURCE_CONTINUE

    def start():
        # Once, after the window has had time to map and paint its first frame.
        GLib.timeout_add(args.interval, step)
        return GLib.SOURCE_REMOVE

    app.connect("activate", lambda _a: GLib.timeout_add(1500, start))
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
