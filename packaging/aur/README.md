# Arch Linux (AUR)

`PKGBUILD` builds pulldeck from a release tag with the system Rust and Qt 6.

Build and install locally:

```bash
cd packaging/aur
makepkg -si
```

When a new version is released:

1. Set `pkgver` in `PKGBUILD` to the release (without the `v`) and `pkgrel` back to `1`.
2. Run `updpkgsums` to refresh `sha256sums`.
3. Check it builds in a clean chroot (`extra-x86_64-build` from `devtools`).
4. Publish: copy `PKGBUILD` to a clone of `ssh://aur@aur.archlinux.org/pulldeck.git`, run
   `makepkg --printsrcinfo > .SRCINFO`, commit both and push.

The crate needs the Rust version in `rust-version` (see the workspace `Cargo.toml`); if Arch's
`rust` is older, wait for it or build with rustup.
