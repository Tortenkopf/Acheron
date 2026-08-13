Type: grilling
Status: resolved

## Question

When a Toggle-mode Binding is active — a Macro looping, or a Keypress held down — and the user then switches Profile, or the Layer changes (Mode key released/re-held), what should happen to that active toggle: release immediately, keep running until explicitly toggled off regardless of Profile/Layer, or something else? This is a real behavioral decision that shapes the Daemon's state model, not an implementation detail — get it wrong and either macros run away unbounded across contexts, or toggles silently drop in a way that surprises the user mid-use.

## Answer

Grilling session, 2026-08-12.

- **Layer change** (Mode key pressed/released): never touches an active toggle. Layers are momentary and can be entered/exited incidentally (grazing the Mode key while reaching for another key), so killing a running Macro loop or a held Keypress on every Layer flicker would be destructive and surprising mid-use — the exact failure mode this ticket warns against. A looping Macro or held Keypress keeps going through any number of Layer transitions.
- **Profile switch** (deliberate, manual): releases every active toggle immediately, as part of the switch. Switching Profile is the user's explicit "I'm doing something else now" signal (e.g. gaming → editing); a Macro or held key surviving into an unrelated Profile is the "runs away unbounded across contexts" failure this ticket calls out, and there'd be no clean way to stop it since the originating key may now mean something entirely different in the new Profile.
- **Stop mechanism, given a Binding is scoped per-Layer**: because Layer changes don't kill a running toggle, the same physical key can end up bound to something else in the current Layer while the toggle it started is still running elsewhere. Resolved as: **pressing the key that has an active toggle always stops that toggle first**, regardless of what Binding the current Layer nominally assigns to that key. Only once the toggle is stopped does the key resume evaluating the current Layer's own Binding. This gives one discoverable, always-available "off switch" — press what you pressed to start it — and rules out orphaned/runaway toggles.

Net model: an active toggle's identity is pinned to the **physical key** that started it, independent of the Binding lookup, and survives Layer changes but not Profile changes. This is Daemon state that sits alongside (but takes priority over) the normal Profile/Layer → Binding lookup.
