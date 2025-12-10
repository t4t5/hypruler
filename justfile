dev:
  cargo run

build:
  cargo build --release

start:
  ./target/release/pixelsnap

install:
  cargo install --path .
