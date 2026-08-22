Type: task

## Notice:
While this Ticket is not technically blocked by anything, it should still deferred towards the end
of 1.0 development, as we cannot say for certain what else will need to go into this documentation,
that might still surface while the full feature set is being implemented.

## Question

Write the release documentation a stranger needs to build, install, and use Acheron from a clean
git checkout: `README.md` (what it is, feature list, screenshots/description of the GUI),
install/build instructions matching `install.sh`'s real steps (including its privileged udev-rule
step, per [Wire live source-swap, udev rule, and install.sh](./23-task-wire-analog-supervisor-and-install.md)),
and a `CONTRIBUTING.md` if warranted. Both blocking prerequisites are now settled: the v1.0 feature
list ([Lock the v1.0 feature list](./08-decide-v1-feature-list.md)) and the license
([Choose an open source license](./09-decide-open-source-license.md) — GPLv3-or-later, `LICENSE`
already at the repo root).

Settle at least:

- **Structure and scope**: README sections (feature list, hardware requirement, build/install,
  usage basics, license) vs. a fuller docs split (separate INSTALL.md, USAGE.md); match the
  project's actual size rather than over-building.
- **Per-file copyright headers**: [ticket 09](./09-decide-open-source-license.md) deferred the
  "or (at your option) any later version" GPL header convention to this ticket as a repo-wide
  mechanical pass — decide the exact header text and where it lands (every source file? just
  new/touched ones going forward?).
- **How much process detail to carry into the public repo** — `.scratch/`, `prototype/`,
  `docs/adr/`, `CONTEXT.md` are all currently assumed to ship as-is, but the user flagged a
  concern this might overwhelm someone who just wants to game. This ticket's README should at
  least *not make that worse* (e.g. don't lead with process); whether those directories ship at
  all is tracked separately in the map's Not yet specified and isn't this ticket's call to make.
- Live-check: does `install.sh` actually work end-to-end on a clean checkout, following only the
  written instructions?

## Answer
