{
  # Reproducible dev shell with a pinned Qt 6, Rust and the project's tools:
  #   nix --extra-experimental-features 'nix-command flakes' develop
  # (or `nix develop` after enabling flakes in nix.conf; add `use flake` to .envrc for direnv).
  description = "pulldeck development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          # Keep in sync with workspace.package.rust-version / mise.toml.
          rust = pkgs.rust-bin.stable."1.99.1".default.override {
            extensions = [ "rust-src" "clippy" "rustfmt" ];
          };
          qt = pkgs.qt6;
        in
        {
          default = pkgs.mkShell {
            packages = [
              rust
              pkgs.pkg-config
              pkgs.cmake
              qt.qtbase
              qt.qtdeclarative # also provides qmllint / qmlformat
              qt.qtsvg
              pkgs.just
              pkgs.prek
              pkgs.cargo-deny
              pkgs.git-cliff
              pkgs.pinact
              pkgs.actionlint
              pkgs.typos
              pkgs.gh
              pkgs.jq
              pkgs.uv
            ];

            # cxx-qt-build locates Qt through qmake.
            QMAKE = "${qt.qtbase}/bin/qmake";
            # So the app can find its Qt plugins and QML modules when run from the shell.
            QT_PLUGIN_PATH = "${qt.qtbase}/${qt.qtbase.qtPluginPrefix}";
            QML2_IMPORT_PATH = "${qt.qtdeclarative}/${qt.qtbase.qtQmlPrefix}";
          };
        });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixpkgs-fmt);
    };
}
