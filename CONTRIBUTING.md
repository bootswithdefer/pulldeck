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
3. The tag triggers `release.yml`, which attaches a Linux binary.

One-time setup: add a `RELEASE_PLZ_TOKEN` repository secret (a PAT with `contents` and
`pull-requests` write access). Without it the tag is still created, but `release.yml` will
not start.
