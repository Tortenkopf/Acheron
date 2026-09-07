# Additive staging mode removed: a real keyboard can't hold two autorepeating keys

"Dual-stage keys" (ADR-0007) shipped with four Staging modes governing how a grid key's
primary and deep Actuation stages hand off as Depth crosses the deep band: Handoff,
No-Return, **Additive**, and Quick-Skip. Additive was the odd one out — instead of handing
the press from one stage to the other, it held *both* stages live at once: crossing into
the deep band added the deep stage's firing without releasing the primary.

We remove it. `StagingMode` becomes **Handoff / No-Return / Quick-Skip**
(`.scratch/humane-output-rate/` tickets 11 and 13).

## Why

**The output is physically impossible, not merely a regularity tell.** A real keyboard
autorepeats only the *most-recently-pressed* key — the kernel's `input_repeat_key` tracks a
single `repeat_key`, so you cannot hold two keys in autorepeat simultaneously. An Additive
key with both stages set to Hold-to-repeat emits **two concurrent `value=2` autorepeat
streams from one finger press**. This became concrete with the `value=2` kernel-shaped
repeat rebuild (ADR-0008; specced in `.scratch/humane-output-rate/spec-kernel-shaped-repeat.md`,
carried out in `.scratch/kernel-shaped-repeat-impl/`), which converted single-key
deep-stage Hold-to-repeat to genuine kernel autorepeat. `stage::Engine::deep_repeat` drives
the deep stream off the *primary's* `RepeatSchedule` pulses, so the two streams are also
phase-locked — a shape no physical two-finger press could produce either. That is squarely
an ADR-0008 ("physical-plausibility ceiling") violation, not a borderline call.

**No use case.** Nothing in the dual-stage spec, and nothing surfaced in review, needs
Additive's signature capability. Handoff and No-Return cover "hand off to a deeper action";
"fire several keys on the deep press" is a Macro bound to the deep stage.

**Always the shakiest mode.** The `tartarus-dual-stage-keys-impl` #03 post-ship corrections
clustered on the primary↔deep repeat interaction, with Additive entangled in each.

**Clean invariant after the cut.** With Additive gone, **no Staging mode ever emits two
concurrent autorepeat streams**: Handoff and No-Return release the primary in the deep band
(`primary_handed_off`), and Quick-Skip's `Skipped` phase suppresses it entirely.

## Migration: hard `ConfigError` at load, not a silent downgrade

A `config.toml` carrying `deep_stages.<input>.mode = "additive"` is **refused at load** with
`ConfigError::RemovedStagingModeAdditive`, which names Additive and points at rebinding the
deep stage or using a Macro. It is *not* silently downgraded to Handoff. Dual-stage keys is
a new niche feature with no known users of any mode, so a hard, explicit break is
acceptable and is the least surprising outcome. Detection is a raw-TOML scan
(`config::find_removed_additive_staging`) ahead of the typed deserialize, mirroring the
pre-existing legacy-inline-Macro guard — with `Additive` gone from the enum, serde would
otherwise reject `"additive"` as a generic "unknown variant". At the D-Bus `SetStagingMode`
boundary a client sending `"additive"` just gets the ordinary unknown-mode rejection.

## Consequences

- The pure `stage.rs` core loses its `additive()` transition table and one `advance()` arm;
  `deep_repeat` / `primary_handed_off` stay (Handoff / No-Return still need them).
- ADR-0007's four-mode parenthetical is corrected with a pointer here. ADR-0008 gains a
  one-line note that Additive is the first surface the ceiling *deleted* rather than
  reshaped. The `CONTEXT.md` **Additive** glossary entry is removed and the **Staging mode**
  list trimmed to three.
- Removing a shipped feature is a discrete, surprising, hard-to-reverse decision, so it
  earns its own ADR rather than an amendment to ADR-0007 (whose thesis is the narrow
  "depth interpretation runs in dispatch") or ADR-0008.

Cites ADR-0008 (the reason) and ADR-0007 (the four-mode context).
