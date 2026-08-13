# D-Bus for Daemon↔GUI IPC

The Daemon and GUI need to communicate — the GUI pushes configuration changes and reads current state; the Daemon reports which Profile and Layer are active. We chose D-Bus over a custom Unix-socket protocol: this system already runs a working session D-Bus bus (proven functional by OpenRazer itself), both Rust (`zbus`) and Python (`dbus-python`) have mature bindings, and D-Bus gives introspection/debugging tooling (`dbus-send`, `d-feet`) for free instead of requiring a hand-invented and hand-documented wire protocol.
