Type: task
Blocked by: 55

## Question

Live-verify [ticket 55](./55-task-build-stepper-library-gui.md)'s Stepper library GUI against the real Daemon and Tartarus Pro — split off [ticket 41](./41-task-build-stepper-macro-library-ux.md), matching this map's discipline that a ticket resolves only once actually tested against the real, connected hardware, and matching the task/verify pairing precedent set by tickets 42/44 and 48/49.

Checklist:

- [ ] Create a new Stepper list, add/reorder/remove items using the real key/mouse-button picker.
- [ ] Assign the list's forward/backward Bindings to the scroll wheel's Up/Down Inputs (the primary intended use case), confirm each notch advances/retreats the cursor and fires the newly-selected item in one motion.
- [ ] Confirm wrap-around at both ends of the list.
- [ ] Confirm Hold-to-repeat fast-advance works via the existing repeat machinery, and that Toggle is correctly unavailable/rejected for a Stepper Binding.
- [ ] Assign a second Input pair to the same list; confirm it silently steals the pair from the first assignment and the GUI surfaces the "Moved off '<name>'" toast.
- [ ] Restart the Daemon; confirm the cursor resets to the list's first item (never persisted).
- [ ] Delete a Stepper list; confirm no delete gate blocks it even while assigned (ticket 03/31's settled no-gate behavior).

Fix any real bugs found live before considering this resolved, per this map's standing discipline.
