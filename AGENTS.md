# AGENTS.md

pulldeck: a Rust + Qt (cxx-qt 0.10, QML) app that tracks the user's GitHub PRs.

## Layout

- `crates/pulldeck-core` — UI-free: `auth` (env or `gh auth token`), `client` (GraphQL, paginated),
  `models`, and `store::diff`, which turns an old and new PR list into per-row `Change`s
  (Insert/Remove/Update). Parsing and the diff are unit-tested; the diff test is exhaustive over
  all small list transitions.
- `crates/pulldeck` — Qt app. `src/backend.rs` is `PrBackend`, a `QAbstractListModel` (roles
  `title`, `repo`, `number`, `url`, `author`, `isDraft`, `review`, `checks`, `updatedAt`;
  properties `status`, `user`, `loading`; invokables `refresh()`, `ago()`). A refresh applies the
  diff with `beginInsertRows`/`beginRemoveRows`/`dataChanged`, never a model reset. `qml/` holds
  the UI; `build.rs` registers the QML module `io.github.bootswithdefer.pulldeck`. New QML files
  must be added to `build.rs`. A new model role needs a constant, a `role_names` entry, a `data`
  arm and a `required property` in the delegate.

## Rules

- No HTTP or subprocess on the Qt thread: spawn on the tokio runtime, return via `qt_thread().queue`.
- `just check` must pass: rustfmt, clippy (`-D warnings`), tests, `qmlformat` and `qmllint`
  (warnings are errors). `just ci` adds `cargo deny`.
- Run Qt tools through `scripts/qt-tool`: a bare `qmllint` can be Qt 5's, which silently checks nothing.
- Never log or persist tokens.

## Commits & releases

- Conventional Commits are required; they drive version bumps and `CHANGELOG.md` (release-plz +
  `cliff.toml`). Never hand-edit `CHANGELOG.md` or bump versions manually.
