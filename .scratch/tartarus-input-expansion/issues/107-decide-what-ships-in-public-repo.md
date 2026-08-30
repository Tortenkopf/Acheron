Type: grilling
Status: claimed

## Question

Graduated from the map's **Not yet specified** — "What ships in the public repo" — the
last such item the user wants settled before v1.0 (the rest move to a post-1.0 effort).

Acheron's git repo (`github.com/Tortenkopf/Acheron`) is about to be made public. It was
built ticket-by-ticket with a heavy on-disk process record. Decide, for the v1.0 public
release, the disposition of each build-process artifact — keep in the tree as-is, keep
but trim/redact, move off `main` (history/branch only), or purge from history:

- `.scratch/` — the whole local issue tracker: 2 effort dirs, ~130 ticket files, research
  write-ups, raw `hidraw` capture assets (one 930 KB `.jsonl`), a 176 KB `map.md`.
  2.8 MB, 149 files. CONTRIBUTING.md currently points contributors at it as the design
  record.
- `prototype/` — 3 `.py` spikes on `main` (09, 12, 13); 8 more on unmerged local
  `prototype/*` branches, one of them (`prototype/30-chord-recording-ux`) pushed to
  `origin`. Shipped `daemon/`/`gui/` source carries ~15 inline comments citing
  `prototype/NN-…` paths, several of which resolve only on those unmerged branches.
- `docs/adr/` (4 ADRs), `CONTEXT.md` (the domain glossary) — genuine developer docs or
  process noise?
- `docs/agents/` (`domain.md`, `issue-tracker.md`) + `CLAUDE.md` — Claude Code tooling
  instructions; `issue-tracker.md` documents the `.scratch/` convention specifically.
- The **git commit history** — 131 commits, all authored `Charon`, subjects
  `Resolve ticket NN: …`, recent ones carrying `Co-Authored-By: Claude` /
  `Claude-Session:` trailers. Present regardless of the working tree unless history is
  rewritten. Depends on how the repo goes public (flip visibility on this repo vs. a
  fresh repo/history).
- The device **serial number** `PM24XXXXXXXXXXX` — appears ~10× in `.scratch/**/research/`
  files (and in history). A real hardware identifier of the user's unit.
- Commit **author identity** `Charon` vs. the real name on `LICENSE`/copyright
  (Justin Milatz) — deliberate themed pseudonym, or to be corrected?

Settle each, plus the mechanism and whether it's a ticket on this map (done before
archiving the effort) or its own small effort.

## Answer

Settled over a two-round grilling. The concern behind this item — that the heavy
on-disk process record would overwhelm a casual visitor who just wants to game — is
answered by making `main` a clean release artifact and moving the whole record to a
`dev` branch, not by destroying any of it.

### Decisions

| Point | Outcome |
|---|---|
| **Publish mechanism** | Flip visibility on the existing `Tortenkopf/Acheron` repo; full 131-commit history stays intact (an honest ticket-by-ticket AI-agent-built history is a feature, not something to hide; nobody browsing casually reads `git log`). |
| **Branch model** | **`dev` is the permanent working branch** — all ticket work and all `.scratch/`/`prototype/` churn happens there. **`main` is release-only**, mechanically rebuilt from `dev`'s non-process paths at each release (`git checkout dev -- daemon gui packaging docs/adr README.md CONTRIBUTING.md CONTEXT.md install.sh LICENSE layout.md .gitignore`), then tagged. `main` is *always* clean for visitors. |
| **`.scratch/`** | `dev` only. `git rm` from `main`, `.gitignore` it there. |
| **`prototype/`** | The `prototype/` dir (3 base spikes: 09/12/13) is `dev`-only; `git rm` + `.gitignore` on `main`. **The 8 `prototype/*` feature branches are kept and pushed as-is** — revised from "rescue the dirs then delete" once inspection showed they aren't `prototype/NN-slug/` dirs at all but single old commits carrying ad-hoc `gui/prototype_NN_*.py` files amid heavy stale divergence. Keeping the branches is zero-archaeology and keeps every `prototype/NN` code comment resolvable via `git show prototype/NN-slug:…`. |
| **`docs/adr/`, `CONTEXT.md`** | Stay on `main` — genuine, compact developer docs a non-agent contributor needs. |
| **`CLAUDE.md`, `docs/agents/`** | `dev` only. Pure agent plumbing; `CLAUDE.md`'s issue-tracker section is meaningless without `.scratch/`. A contributor running Claude Code works from `dev` anyway. |
| **Device serial** | The unit's real serial → `PM24XXXXXXXXXXX` (format-preserving, 15 chars) everywhere it appeared in `.scratch/**`, on `dev`. No history rewrite — it's a serial number, not a credential. |
| **Author identity** | Keep `Charon` — a deliberate on-theme pseudonym (the ferryman of the Acheron); `LICENSE`/copyright unambiguously name Justin Milatz. |
| **Inline code comments** citing `.scratch/…` / `prototype/…` (~15 across `daemon/`+`gui/`) | Left as-is — historical breadcrumbs; CONTRIBUTING notes those paths refer to the `dev` branch. |

### Execution (done this session, on resolution)

1. This ticket resolved; map updated; the **Tartarus input expansion** effort archived in `.scratch/README.md`.
2. `dev` branched from that commit (full record).
3. On `dev`: serial scrubbed; this Answer + the map corrected for the kept-branches revision.
4. On `main`: `.scratch/`, `prototype/`, `docs/agents/`, `CLAUDE.md` removed; `.gitignore` updated; CONTRIBUTING reworded (layout table + Design-record section point at `dev`).
5. `main`, `dev`, and all 8 `prototype/*` branches pushed. No branches deleted.

Left for the user: `gh repo edit Tortenkopf/Acheron --visibility public`, and `git tag v1.0.0`
on `main` when cutting the release. The other **Not yet specified** items go to a separate
post-1.0 effort (worked on `dev`).
