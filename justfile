default:
    @just --list

# Run the app
run:
    cargo run -p pulldeck

# Print your open PRs without the UI
list:
    cargo run -q -p pulldeck-core --example list

# Everything CI checks
check: fmt-check clippy test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

deny:
    cargo deny check
