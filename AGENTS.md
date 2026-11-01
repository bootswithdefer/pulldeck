# AGENTS.md

pulldeck: a Rust + Qt (cxx-qt 0.10, QML) app that tracks the user's GitHub PRs.

## Layout

- `crates/pulldeck-core` — UI-free: `auth` (env or `gh auth token`), `client` (GraphQL, paginated),
  `models`, and `store::diff`, which turns an old and new PR list into per-row `Change`s
  (Insert/Remove/Update). Parsing and the diff are unit-tested; the diff test is exhaustive over
  all small list transitions. `session` holds the client's replica of server state (PR list, inbox,
  resume point) as pure, synchronous code over domain types (`ServerEvent`, `ClientCommand`) that
  are independent of the wire format. `remote` is the connection driver (`RemoteSession`: sign-in,
  resume, reconnect with backoff, refusing actions while offline) behind `Transport`/`Connector`
  traits, tested with a fake transport on a paused clock. A real websocket transport converts
  between protocol messages and the domain types and validates commands before sending.
- `crates/pulldeck` — Qt app. `src/backend.rs` is `PrBackend`, a `QAbstractListModel` (roles
  `title`, `repo`, `number`, `url`, `author`, `isDraft`, `review`, `checks`, `updatedAt`;
  properties `status`, `user`, `loading`; invokables `refresh()`, `ago()`). A refresh applies the
  diff with `beginInsertRows`/`beginRemoveRows`/`dataChanged`, never a model reset. `qml/` holds
  the UI; `build.rs` registers the QML module `io.github.bootswithdefer.pulldeck`. New QML files
  must be added to `build.rs`. A new model role needs a constant, a `role_names` entry, a `data`
  arm and a `required property` in the delegate.

## Rules

- The pulldeck server is optional. The app always works standalone by polling GitHub itself; a
  server session adds live updates and collaboration. `PrBackend` keeps the server's pull requests
  and the polled ones as separate lists and shows `store::merge` of them (the server's copy wins),
  and only server-backed rows (`serverBacked`) offer server actions. Never let one source
  overwrite the other.
- No HTTP or subprocess on the Qt thread: spawn on the tokio runtime, return via `qt_thread().queue`.
- `just check` must pass: rustfmt, clippy (`-D warnings`), tests, `qmlformat` and `qmllint`
  (warnings are errors). `just ci` adds `cargo deny`.
- Run Qt tools through `scripts/qt-tool`: a bare `qmllint` can be Qt 5's, which silently checks nothing.
- Tokens never appear in logs, `Debug` output or error messages (`SessionToken` redacts itself and
  is not `Display`). The client persists at most the opaque server-issued session token, in the OS
  keyring, never in plain config; it never sees or stores a GitHub token. The server may persist
  GitHub tokens only encrypted at rest.
- Presence (telling others which PR you are viewing) is opt-in and off by default; the client
  simply never sends it unless the user turned it on.

## Commits & releases

- Conventional Commits are required; they drive version bumps and `CHANGELOG.md` (release-plz +
  `cliff.toml`). Never hand-edit `CHANGELOG.md` or bump versions manually.
