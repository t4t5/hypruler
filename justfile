# List all commands
default:
  @just --list

# Run the dev version (slow performance)
dev:
  cargo run

# Build the release version
build:
  cargo build --release

# Run the release version (fast performance)
start:
  ./target/release/hypruler

# Check for errors and warnings
check:
  cargo check --all
  cargo clippy --all

# Build a version with FPS monitoring
build-debug:
  cargo build --profile release-debug

# Run the debug version with FPS monitoring
start-debug:
  HYPRULER_DEBUG=1 ./target/release-debug/hypruler

# Install cargo dependencies
install:
  cargo install --path .

# Bump Cargo.toml version and commit
release version:
  #!/usr/bin/env bash
  set -euo pipefail

  current=$(grep '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')

  if [[ "{{version}}" == "$current" ]]; then
    echo "Version is already $current, no update needed"
  else
    sed -i 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    cargo check
    echo "Updated version: $current → {{version}}"
  fi

  git add Cargo.toml Cargo.lock
  git commit -m "Version {{version}}" || echo "Nothing to commit"
  echo "Push to main to trigger release of v{{version}}"
