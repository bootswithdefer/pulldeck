# Flatpak

`io.github.bootswithdefer.pulldeck.yml` is a **starting point that has not been built**. Nothing
here has been run, and these are the known gaps:

- **Crate sources.** Flatpak builds offline, so every crate must be listed. Generate
  `cargo-sources.json` from `Cargo.lock` with
  [flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo)
  and put it next to the manifest.
- **Rust version.** The workspace needs the Rust in `rust-version` (`Cargo.toml`). The
  `rust-stable` SDK extension may be older or newer than that; check before building.
- **Qt.** `cxx-qt` finds Qt through `qmake`. The KDE SDK has it, but the Qt version
  (`runtime-version`) has to match what the app was written for (Qt 6).
- **Sandbox.** The app polls GitHub with a token from `GH_TOKEN`, `GITHUB_TOKEN` or `gh auth token`.
  Inside the sandbox there is no `gh` and no environment, so standalone polling needs a way to
  hand it a token (for example `flatpak override --env=GH_TOKEN=...`) until a sign-in flow
  that works without the CLI exists.

Build once the above is sorted:

```bash
flatpak-builder --user --install --force-clean build-dir io.github.bootswithdefer.pulldeck.yml
flatpak run io.github.bootswithdefer.pulldeck
```
