#!/usr/bin/env python3
"""Spike: prove evdev-capture -> uinput-inject pipeline.

Grabs the Tartarus Pro's if01 node exclusively. On the grid key that
normally emits KEY_Q (row 2, col 2 - confirmed in ticket 01), injects
KEY_B via a virtual uinput device instead. If the pipeline works, and
uinput write access is really available without extra setup, pressing
that physical key should type "b" into a focused text field - never "q",
since if01 is grabbed and the real KEY_Q event never reaches the desktop.
"""
import evdev
from evdev import ecodes, UInput
import select
import signal
import sys

SRC = "/dev/input/by-id/usb-Razer_Razer_Tartarus_Pro-if01-event-kbd"

dev = evdev.InputDevice(SRC)
dev.grab()

ui = UInput({ecodes.EV_KEY: [ecodes.KEY_B]}, name="tartarus-spike-uinput")

running = True


def stop(signum, frame):
    global running
    running = False


signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)

print("spike running: grid key at row2/col2 (normally KEY_Q) -> injects KEY_B", file=sys.stderr)

try:
    while running:
        r, _, _ = select.select([dev], [], [], 0.5)
        if not r:
            continue
        for event in dev.read():
            if event.type == ecodes.EV_KEY and event.code == ecodes.KEY_Q:
                ui.write(ecodes.EV_KEY, ecodes.KEY_B, event.value)
                ui.syn()
                print(f"relayed KEY_Q value={event.value} -> injected KEY_B", file=sys.stderr)
finally:
    dev.ungrab()
    dev.close()
    ui.close()
    print("stopped, ungrabbed, uinput device closed", file=sys.stderr)
