Type: grilling

## Question

Choose an open source license for Acheron's public release. Settle at least:

- Permissive (MIT/Apache-2.0) vs. copyleft (GPL family) — what does the user actually want for this project (attribution-only vs. derivative-works-stay-open), and does anything about the domain (a tool that talks to kernel input devices, no bundled third-party GPL code currently in the tree as far as known) push toward or away from either?
- If Apache-2.0 is a candidate: does its patent grant clause matter here, or is that irrelevant for a hobby hardware tool?
- Practical output: add the chosen license's text as `LICENSE` at the repo root, and confirm none of the project's existing dependencies (Rust crates, Python/GTK4 libraries) carry a license incompatible with the choice.

## Answer

