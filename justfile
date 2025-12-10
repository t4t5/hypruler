dev:
  cargo run

build:
  cargo build --release

start:
  ./target/release/hypruler

install:
  cargo install --path .

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

  git add Cargo.toml
  git commit -m "Version {{version}}" || echo "Nothing to commit"
  git tag -a "v{{version}}" -m "Version {{version}}"
  echo "Tagged v{{version}}"
