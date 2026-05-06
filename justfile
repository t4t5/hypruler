dev:
  cargo run

build:
  cargo build --release

bench:
  cargo bench --bench draw -- --sample-size 10 --measurement-time 1 --warm-up-time 1

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

  git add Cargo.toml Cargo.lock
  git commit -m "Version {{version}}" || echo "Nothing to commit"
  git tag -a "v{{version}}" -m "Version {{version}}"
  echo "Tagged v{{version}}"
