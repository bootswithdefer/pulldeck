# Contributing

## Setup

You need a recent stable Rust toolchain, Qt 6 (base + declarative) and an authenticated
[`gh`](https://cli.github.com/) CLI.

```bash
# Arch
sudo pacman -S qt6-base qt6-declarative qt6-svg
# Debian/Ubuntu
sudo apt install qt6-base-dev qt6-declarative-dev libgl1-mesa-dev \
  qml6-module-qtquick-controls qml6-module-qtquick-layouts qml6-module-qtquick-templates
```

## Workflow

```bash
just check   # fmt, clippy (warnings are errors), tests
just run     # launch the app
just list    # headless: print your PRs via the core crate
```

- Install the git hooks once per clone with `prek install` ([prek](https://prek.j178.dev),
  config in `prek.toml`). They check formatting, spelling, secrets and workflows on commit,
  require Conventional Commit messages, and run clippy, tests and `cargo deny` before push.
  `just pin` refreshes the pinned hook and action versions.
- Keep UI-independent logic in `pulldeck-core` and unit-test it there.
- Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:` …).
  With squash merges the **PR title** becomes the commit, so CI checks it.
- Do **not** edit `CHANGELOG.md` by hand. It is generated from commit messages.

## Releases

Releases are automated with [release-plz](https://release-plz.dev):

1. Merging to `main` opens or updates a *release PR* that bumps the version (SemVer, inferred
   from commit types: `feat` → minor, `fix` → patch, `!` or `BREAKING CHANGE` → major) and
   rewrites `CHANGELOG.md` in [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format
   (`feat` → Added, `fix` → Fixed, `perf`/`refactor` → Changed, …; `chore`, `ci`, `docs` and
   `test` are omitted).
2. Merging that PR tags `vX.Y.Z` and creates the GitHub release with the changelog section.
3. The same workflow then calls `release.yml`, which builds the Linux binary and attaches it
   to the release. To (re)build one by hand: `gh workflow run release.yml -f tag=vX.Y.Z`.

Optional: add a `RELEASE_PLZ_TOKEN` repository secret (a PAT with `contents` and
`pull-requests` write access). Pull requests opened with the default `GITHUB_TOKEN` don't
trigger CI, so without it the release PR runs no checks.

### Publishing to crates.io

CI does not publish yet (`publish = false` in `release-plz.toml`). After a release PR is
merged, publish by hand from the tagged commit:

```bash
cargo login            # once, with a crates.io token
just publish-dry-run   # check packaging
just publish           # dry run again, asks for confirmation, then publishes
```

`--no-verify` is required: `cxx-qt-build` writes a `.qmlls.ini` into the crate directory
during the build, and cargo's tarball verification rejects build scripts that modify the
source tree. The crate itself builds fine; CI covers that. Publishing is permanent, so use
the dry run first. `pulldeck-core` is published before `pulldeck` automatically.

To automate this, add a `CARGO_REGISTRY_TOKEN` secret, pass it to the `release` job in
`.github/workflows/release-plz.yml`, set `publish = true` in `release-plz.toml` and add
`publish_no_verify = true`.
