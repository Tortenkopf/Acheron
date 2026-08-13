Type: grilling

## Question

Decide how the Daemon is packaged and run as a systemd service: user unit vs system unit (weighing that `/dev/uinput` and the device nodes already work for this user via the `plugdev` group and an existing ACL, per the map's Notes, with no extra permission setup needed), unit file contents, install location/process, and whether/how it autostarts (login vs GUI launch) and restarts on failure.
