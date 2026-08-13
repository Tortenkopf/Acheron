# Split-language stack: Rust Daemon, Python+GTK4 GUI

The Daemon and GUI run as separate processes (see CONTEXT.md). The Daemon is written in Rust; the GUI is written in Python using GTK4 (PyGObject). The Daemon sits on the latency-critical input path, where Rust avoids interpreter startup and GC jitter; the GUI is not latency-sensitive and benefits more from Python's development speed and PyGObject's maturity relative to `gtk4-rs`. Splitting along the process boundary lets each side use the tool suited to its job, rather than a shared-runtime constraint forcing a compromise on one side.
