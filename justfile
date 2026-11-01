# List available recipes
default:
    @just --list

# ── run ──────────────────────────────────────────────────────────────

# Run the app
run:
    cargo run -p pulldeck

# Run against a scripted fake server, to try the collaboration UI without a real one
demo:
    PULLDECK_DEMO=1 cargo run -p pulldeck --features demo

# Print your open PRs without the UI
list:
    cargo run -q -p pulldeck-core --example list

# ── build & install ──────────────────────────────────────────────────

# Debug build of the whole workspace
build:
    cargo build --workspace

# Optimized build of the app
build-release:
    cargo build --release -p pulldeck

# Install the app from this checkout into ~/.cargo/bin
install:
    cargo install --path crates/pulldeck --locked

# Remove the installed app
uninstall:
    cargo uninstall pulldeck

# Add the launcher entry to ~/.local/share/applications
install-desktop:
    install -Dm644 packaging/pulldeck.desktop ~/.local/share/applications/pulldeck.desktop

# Remove build output
clean:
    cargo clean

# ── quality ──────────────────────────────────────────────────────────

# Fast checks: rustfmt, clippy, tests and the QML checks
check: fmt-check clippy test qml-fmt-check qml-lint

# Everything CI checks, including cargo-deny
ci: check deny

# Format the code
fmt:
    cargo fmt --all

# Fail if the code is not formatted
fmt-check:
    cargo fmt --all -- --check

# Lint with clippy; warnings are errors
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Apply clippy's automatic fixes, then format
fix:
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged
    cargo fmt --all

# Run all unit tests
test:
    cargo test --workspace --all-features

# Lint the QML; warnings are errors. Uses the app's generated types, so it builds first if needed
qml-lint:
    test -d target/cxxqt/qml_modules || cargo build -p pulldeck
    scripts/qt-tool qmllint --max-warnings 0 -I target/cxxqt/qml_modules crates/pulldeck/qml/*.qml

# Format the QML in place
qml-fmt:
    scripts/qt-tool qmlformat -i crates/pulldeck/qml/*.qml

# Fail if the QML is not formatted
qml-fmt-check:
    for f in crates/pulldeck/qml/*.qml; do scripts/qt-tool qmlformat "$f" | diff -u "$f" - || exit 1; done

# Licenses, advisories, bans and sources (needs cargo-deny)
deny:
    cargo deny check

# Build the API docs and open them
doc:
    cargo doc --workspace --no-deps --open

# ── dependencies & CI ────────────────────────────────────────────────

# Update Cargo.lock to the latest compatible dependency versions
update:
    cargo update

# Pin GitHub Actions and prek hooks to commit SHAs at their latest releases
pin:
    GITHUB_TOKEN="$(gh auth token)" pinact run --update
    prek autoupdate --freeze

# ── release ──────────────────────────────────────────────────────────

# Print the workspace version
version:
    @cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="pulldeck") | .version'

# Preview the changelog entries pending since the last release (needs git-cliff)
changelog:
    git cliff --config cliff.toml --unreleased

# --no-verify is required: cxx-qt-build writes .qmlls.ini into the crate dir, which cargo's verify step rejects.
# Check crates.io packaging without uploading
publish-dry-run:
    cargo publish --workspace --dry-run --no-verify

# Publish pulldeck-core then pulldeck to crates.io (permanent: versions can only be yanked)
[confirm("Publish to crates.io? This cannot be undone.")]
publish: publish-dry-run
    cargo publish --workspace --no-verify

# ── assets ───────────────────────────────────────────────────────────

# Regenerate images/logo.svg and images/logo.png (needs the Noto Sans font)
logo:
    ./images/build_logo.py
