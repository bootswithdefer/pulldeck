{
  # Reproducible build and dev shell with a pinned Qt 6, Rust and the project's tools:
  #   nix develop        # shell with Qt, Rust 1.99.1 and just/prek/cargo-deny/...
  #   nix build          # builds the app into ./result/bin/pulldeck
  #   nix run            # builds and runs it
  # Flakes need `experimental-features = nix-command flakes` in nix.conf (or pass
  # `--extra-experimental-features 'nix-command flakes'`). Add `use flake` to .envrc for direnv.
  description = "pulldeck: a GitHub pull request tracker (Rust + Qt)";

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

      pkgsFor = system: import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };

      # Nix puts every Qt module in its own store path, but cxx-qt-build asks `qmake` for a
      # single library directory and looks for every module there. So: merge the modules we
      # use into one prefix and hand cxx-qt a qmake that reports that prefix.
      qtFor = pkgs:
        let
          qt = pkgs.qt6;
          merged = pkgs.symlinkJoin {
            name = "qt6-merged";
            paths = with qt; [
              qtbase
              qtbase.dev
              qtdeclarative
              qtdeclarative.dev
              qtsvg
              qtsvg.dev
            ];
          };
          # `qmake -query` is what cxx-qt-build reads; everything else passes straight through.
          qmake = pkgs.writeShellScriptBin "qmake" ''
            if [ "''${1:-}" = "-query" ]; then
              ${qt.qtbase}/bin/qmake "$@" | ${pkgs.gnused}/bin/sed \
                -e 's|${qt.qtbase}|${merged}|g' \
                -e 's|${qt.qtbase.dev}|${merged}|g'
            else
              exec ${qt.qtbase}/bin/qmake "$@"
            fi
          '';
        in
        {
          inherit qt merged qmake;
          pluginPath = "${merged}/${qt.qtbase.qtPluginPrefix}";
          qmlPath = "${merged}/${qt.qtbase.qtQmlPrefix}";
        };

      # Keep in sync with workspace.package.rust-version / mise.toml.
      rustFor = pkgs: pkgs.rust-bin.stable."1.99.1".default.override {
        extensions = [ "rust-src" "clippy" "rustfmt" ];
      };
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = pkgsFor system;
          q = qtFor pkgs;
          rust = pkgs.rust-bin.stable."1.99.1".minimal;
          rustPlatform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
        in
        {
          default = rustPlatform.buildRustPackage {
            pname = "pulldeck";
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "pulldeck" ];
            doCheck = false; # `just ci` runs the tests; this derivation only builds.

            nativeBuildInputs = [ pkgs.pkg-config pkgs.cmake q.qt.wrapQtAppsHook ];
            buildInputs = [ q.merged ];
            # Qt's setup hook sets QMAKE to qtbase's own; override it once the hooks have run.
            preBuild = ''
              export QMAKE=${q.qmake}/bin/qmake
            '';

            meta = {
              description = "Track your GitHub pull requests from a native desktop window";
              homepage = "https://github.com/bootswithdefer/pulldeck";
              license = pkgs.lib.licenses.mit;
              mainProgram = "pulldeck";
            };
          };
        });

      devShells = forAllSystems (system:
        let
          pkgs = pkgsFor system;
          q = qtFor pkgs;
        in
        {
          default = pkgs.mkShell {
            packages = [
              (rustFor pkgs)
              pkgs.pkg-config
              pkgs.cmake
              q.qmake
              q.merged
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

            # So the app finds its Qt plugins and QML modules when run from the shell.
            QT_PLUGIN_PATH = q.pluginPath;
            QML2_IMPORT_PATH = q.qmlPath;

            # cxx-qt-build locates Qt through qmake (see qtFor above). Qt's setup hook sets
            # QMAKE to qtbase's own, so override it after the hooks have run.
            shellHook = ''
              export QMAKE=${q.qmake}/bin/qmake
            '';
          };
        });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/pulldeck";
        };
      });

      formatter = forAllSystems (system: (pkgsFor system).nixpkgs-fmt);
    };
}
