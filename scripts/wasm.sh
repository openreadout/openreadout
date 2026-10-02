#!/usr/bin/env bash
# Build and test the WebAssembly package (crates/openreadout-wasm).
#
#   scripts/wasm.sh build   # packaging/wasm/pkg/ (wasm-bindgen --target web), size-optimized
#   scripts/wasm.sh test    # wasm-bindgen-test in Node, then the npm wrapper's and the demo's tests
#   scripts/wasm.sh demo    # build, then copy the demo page (web/) with the module inlined
#                           # into book/book/demo/ (after `mdbook build book`) or $DEMO_OUT
#   scripts/wasm.sh sizes   # print the sizes of the built module
#
# Needs: rustup target add wasm32-unknown-unknown; wasm-bindgen-cli at the version of the
# `wasm-bindgen` crate in Cargo.lock (`cargo install wasm-bindgen-cli --version <v> --locked`);
# Node >= 18. wasm-opt (binaryen) is used when it is on PATH or in packaging/wasm/node_modules.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
pkg="$root/packaging/wasm/pkg"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
wasm="$target_dir/wasm32-unknown-unknown/wasm-release/openreadout_wasm.wasm"

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: $1 not found; $2" >&2
        exit 1
    }
}

check_bindgen_version() {
    need wasm-bindgen "cargo install wasm-bindgen-cli --locked --version \$(the wasm-bindgen version in Cargo.lock)"
    local want have
    want="$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/"/,"",$3); print $3; exit}' "$root/Cargo.lock")"
    have="$(wasm-bindgen --version | awk '{print $2}')"
    if [ "$want" != "$have" ]; then
        echo "error: wasm-bindgen-cli $have does not match the wasm-bindgen crate $want in Cargo.lock" >&2
        echo "hint: cargo install wasm-bindgen-cli --locked --version $want" >&2
        exit 1
    fi
}

find_wasm_opt() {
    if command -v wasm-opt >/dev/null 2>&1; then
        echo wasm-opt
    elif [ -x "$root/packaging/wasm/node_modules/.bin/wasm-opt" ]; then
        echo "$root/packaging/wasm/node_modules/.bin/wasm-opt"
    fi
}

build() {
    check_bindgen_version
    (cd "$root" && cargo build -p openreadout-wasm --target wasm32-unknown-unknown --profile wasm-release)
    rm -rf "$pkg"
    wasm-bindgen --target web --out-dir "$pkg" --out-name openreadout "$wasm"
    local opt
    opt="$(find_wasm_opt)"
    if [ -n "$opt" ]; then
        "$opt" -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
            --enable-mutable-globals "$pkg/openreadout_bg.wasm" -o "$pkg/openreadout_bg.opt.wasm"
        mv "$pkg/openreadout_bg.opt.wasm" "$pkg/openreadout_bg.wasm"
    else
        echo "note: wasm-opt not found; the module is not post-optimized (npm i in packaging/wasm)" >&2
    fi
    sizes
}

sizes() {
    local f="$pkg/openreadout_bg.wasm"
    [ -f "$f" ] || { echo "error: $f missing; run scripts/wasm.sh build" >&2; exit 1; }
    local raw gz
    raw=$(wc -c <"$f" | tr -d ' ')
    gz=$(gzip -9 -c "$f" | wc -c | tr -d ' ')
    echo "openreadout_bg.wasm: $raw bytes ($((raw / 1024)) KiB), gzip -9: $gz bytes ($((gz / 1024)) KiB)"
}

test_all() {
    check_bindgen_version
    need node "install Node >= 18"
    (cd "$root" && CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
        cargo test -p openreadout-wasm --target wasm32-unknown-unknown)
    [ -f "$pkg/openreadout_bg.wasm" ] || build
    node --test "$root/packaging/wasm/test/"*.test.mjs "$root/web/test/"*.test.mjs
}

demo() {
    [ -f "$pkg/openreadout_bg.wasm" ] || build
    local out="${DEMO_OUT:-$root/book/book/demo}"
    node "$root/web/build.mjs" "$out"
    echo "demo written to $out"
}

case "${1:-build}" in
build) build ;;
test) test_all ;;
demo) demo ;;
sizes) sizes ;;
*)
    echo "usage: scripts/wasm.sh build|test|demo|sizes" >&2
    exit 2
    ;;
esac
