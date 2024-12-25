#!/bin/bash
# update rust toolchain
rustup update
# update wasm toolchain
rustup target add wasm32-unknown-unknown
# update trunk
cargo install trunk