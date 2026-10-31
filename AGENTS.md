# AGENTS.md

pulldeck: a Rust + Qt (cxx-qt 0.10, QML) app that tracks the user's GitHub PRs.

## Layout

- `crates/pulldeck-core` — UI-free: `auth` (env or `gh auth token`), `client` (GraphQL, paginated),
  `models`. Parsing is unit-tested in `client.rs`.
- `crates/pulldeck` — Qt app. `src/backend.rs` is the `PrBackend` QObject (properties `prsJson`,
  `status`, `user`, `loading`; invokable `refresh()`); `qml/` holds the UI; `build.rs` registers
  the QML module `io.github.bootswithdefer.pulldeck`. New QML files must be added to `build.rs`.

## Rules

- No HTTP or subprocess on the Qt thread: spawn on the tokio runtime, return via `qt_thread().queue`.
- `just check` must pass (clippy runs with `-D warnings`).
- Never log or persist tokens.

## Commits & releases

- Conventional Commits are required; they drive version bumps and `CHANGELOG.md` (release-plz +
  `cliff.toml`). Never hand-edit `CHANGELOG.md` or bump versions manually.
