Type: grilling
Status: resolved

## Question

Decide how Profiles (with their Layers, Bindings, Actions, and Trigger modes) are persisted to disk: file format (e.g. TOML vs JSON vs other), directory layout under something like `~/.config/`, and the read/write ownership model between the Daemon and the GUI — e.g. does the GUI write the file directly and the Daemon reload-on-change, or does the GUI only ever talk to the Daemon over D-Bus and the Daemon exclusively owns the file.

## Answer

Grilling session, 2026-08-12.

- **Ownership**: the Daemon owns the config file exclusively. The GUI never opens or writes it directly — every edit (add/remove a Profile, Layer, Binding, Trigger mode, Macro) goes over D-Bus to the Daemon, which is the sole reader/writer of the file. Chosen over "GUI writes directly, Daemon reload-on-change" (and its D-Bus-signal variant) because it keeps a single source of truth with no dual-writer races or reload-detection machinery to build, at the cost of a chattier D-Bus surface — the exact shape of that surface is deliberately left to the not-yet-specified "D-Bus interface surface" ticket, not decided here.
- **Format**: TOML — idiomatic in the Rust ecosystem (serde + `toml` crate), human-readable/editable with comments, and its table syntax maps cleanly onto nested Profile/Layer/Binding structure without JSON's no-comments/trailing-comma friction or YAML's indentation footguns.
- **Layout**: a single file, not one-file-per-Profile. Simplest atomic-write story (one file, one owner), and profile count for personal use is small enough to stay readable in one file.
- **Path**: `~/.config/acheron/config.toml`.
- **Versioning**: include a top-level `schema_version = 1` field from the start, so future format changes can detect and migrate old files instead of guessing from field absence.

**Naming decision surfaced during this session**: the tool itself is named **Acheron** (not just this map's "tartarus-keybinder" planning slug) — hence the `acheron` config directory above. This is bigger than this ticket; CONTEXT.md has been updated to record it.

Not decided here (left to later Daemon-implementation work, not a file-format/ownership question): startup behavior when the config file is missing, corrupt, or fails to parse.
