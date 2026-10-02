# openreadout-wasm

OpenReadout's readers compiled to WebAssembly with `wasm-bindgen`, so a web page or a Node program can read instrument files it already holds. The crate has no network code. It is not published to crates.io; the npm package is built from it in `packaging/wasm`. Build and test with `scripts/wasm.sh build` and `scripts/wasm.sh test`. Guide: <https://openreadout.github.io/openreadout/guides/wasm.html>. Licensed MIT OR Apache-2.0.
