# Releases

How to cut a release onto `main`. The README's "Building a release" section covers how the version string is derived; this is the procedure.

## Branch model

- **`dev`** holds everything, including the dev-only design record: `.scratch/`, `prototype/`, `CLAUDE.md`, `docs/agents/`.
- **`main`** is release-only. Each release is **one squashed commit** on top of the previous release, never a merge from `dev`. `main` has its own `.gitignore` with a main-only block that ignores the dev-only paths. **Never promote `.gitignore` from `dev`.**
- Every release commit on `main` gets an annotated tag `vX.Y.Z`, plus a GitHub release.

## Procedure

Start on a clean `dev` that has been pushed.

### 1. Test on `dev`

```sh
(cd daemon && cargo test)
gui/.venv/bin/pytest gui/tests
```

If `gui/.venv` is missing, set it up as described in `CONTRIBUTING.md`. System `python3` has no pytest.

### 2. Bump the version on `dev`

Replace the old version with the new one in all of these files:

- `daemon/Cargo.toml`: `version = "..."`
- `daemon/Cargo.lock`: the `acheron-daemon` package entry only
- `daemon/src/lib.rs`: doc comment
- `gui/acheron_gui/__init__.py`: `_BASE_VERSION`
- `gui/acheron_gui/daemon_stub.py`: `_daemon_version`
- `gui/tests/test_app.py`, `gui/tests/test_daemon_stub.py`, `gui/tests/test_version.py`
- `README.md`: "Building a release" section (two lines)

```sh
sed -i 's/1\.3\.2/1.3.3/g' README.md daemon/Cargo.toml daemon/src/lib.rs \
  gui/acheron_gui/__init__.py gui/acheron_gui/daemon_stub.py \
  gui/tests/test_app.py gui/tests/test_daemon_stub.py gui/tests/test_version.py
sed -i '/^name = "acheron-daemon"$/{n;s/^version = "1.3.2"$/version = "1.3.3"/}' daemon/Cargo.lock
```

Afterwards, grep for the old version, skipping `.scratch/` and `target/`, to confirm nothing is left. Re-run both test suites, then commit to `dev` as `Bump version to X.Y.Z for release`.

### 3. Build the release commit on `main`

```sh
git checkout main
git checkout dev -- $(git diff --name-only main dev -- . \
  ':!.scratch' ':!prototype' ':!docs/agents' ':!CLAUDE.md' ':!.gitignore')
```

If `dev` has **deleted** a file that `main` still has, `git checkout dev -- <path>` fails on it. Remove those with `git rm` instead.

Check the result. With the dev-only paths excluded, the diff below must show **only** `.gitignore`:

```sh
git diff --stat main dev -- . ':!.scratch' ':!prototype' ':!docs/agents' ':!CLAUDE.md'
```

Also check `docs/adr/` and `CONTEXT.md`. Both live on both branches, so new ADRs come across automatically.

Commit as `Release X.Y.Z: <one-line summary>`. The body is a short paragraph saying what the release promotes and which fixes it includes, ending with "Bumps the version to X.Y.Z in the daemon, GUI, and daemon stub." Run both test suites again on `main`.

### 4. Tag

```sh
git tag -a vX.Y.Z -m "Acheron X.Y.Z"
git checkout dev
```

**Switch back to `dev`** afterwards. Leaving the checkout on `main` hides `CLAUDE.md` and `docs/agents/` from the next session.

### 5. Push (ask first)

Pushing is outward-facing, so confirm with the user first.

```sh
git push origin dev main vX.Y.Z
```

### 6. GitHub release

```sh
gh release create vX.Y.Z --verify-tag --title "Acheron X.Y.Z — <feature>" --notes-file <notes.md>
```

Write the notes file in the scratchpad, not in the repo. Use these sections and skip any that are empty:

```markdown
## Highlights
- **<Feature>**: user-facing description (phrase it like the README, not like the tickets).

## Fixes
- **<Area>**: what was wrong, from the user's side.

## Upgrading from vX.Y.(Z-1)
Just `git pull` and re-run `./install.sh`. <Any config or requirement changes.>

**Full changelog:** https://github.com/Tortenkopf/Acheron/compare/vPREV...vX.Y.Z
```

To see what changed, read `git log --oneline <prev-release-commit-on-main>..dev` on `dev`, excluding the version bump. Also read the README and CONTEXT.md diffs between `main` and `dev`, since those already describe the feature in user terms.

Previous releases (`gh release list`) show the tone. Check that the new release is marked Latest.
