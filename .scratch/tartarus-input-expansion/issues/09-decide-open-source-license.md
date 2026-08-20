Type: grilling
Status: resolved

## Question

Choose an open source license for Acheron's public release. Settle at least:

- Permissive (MIT/Apache-2.0) vs. copyleft (GPL family) — what does the user actually want for this project (attribution-only vs. derivative-works-stay-open), and does anything about the domain (a tool that talks to kernel input devices, no bundled third-party GPL code currently in the tree as far as known) push toward or away from either?
- If Apache-2.0 is a candidate: does its patent grant clause matter here, or is that irrelevant for a hobby hardware tool?
- Practical output: add the chosen license's text as `LICENSE` at the repo root, and confirm none of the project's existing dependencies (Rust crates, Python/GTK4 libraries) carry a license incompatible with the choice.

## Answer

**GPLv3-or-later.** The user's gut call was copyleft, not permissive — they want forks/derivative
works of Acheron to stay open source, not to permit closed-source or purely-attribution reuse.
Within the GPL family, went with the FSF-recommended "version 3, or (at your option) any later
version" phrasing rather than pinning to GPLv3-only, so the project stays automatically
compatible with future GPL versions and other GPLv3-or-later projects without needing a relicense
later. GPLv2-or-later was considered and dropped — no existing GPLv2 codebase to interoperate
with, and v3's clearer patent-defense and anti-tivoization language is strictly better for a tool
this shape with no offsetting cost.

Apache-2.0's patent grant clause was raised and set aside once copyleft was chosen — it's an
Apache-2.0-specific mechanism, moot under GPL, which has its own (broader) patent-retaliation
language built into v3 already.

**Dependency compatibility, checked against the real tree:** the Daemon's Rust crates
(`daemon/Cargo.toml`: `evdev`, `tokio`, `tokio-util`, `serde`, `toml`, `dirs`, `zbus`,
`futures-util`, `libc`) are all standard MIT/Apache-2.0-dual-licensed Rust-ecosystem crates — no
GPL incompatibility. The GUI is built on PyGObject/GTK4 (LGPL-2.1+), which is explicitly designed
to be linked into GPL applications (that's the point of choosing LGPL over GPL for a library) — no
incompatibility. Nothing in the current tree needed to change or be replaced.

**Practical output:** `LICENSE` added at the repo root — the canonical GPLv3 text fetched directly
from `https://www.gnu.org/licenses/gpl-3.0.txt` (not reproduced from memory, to avoid transcription
errors in a legal document). The "or later version" choice is a statement made in each source
file's copyright header (the standard GPL convention — the LICENSE file itself is version-3 text
regardless), not in the LICENSE file; per-file headers are deferred to whichever ticket writes the
release documentation, since it's a repo-wide mechanical pass, not part of this decision.
