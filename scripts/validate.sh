#!/usr/bin/env bash
# The shared shipping gate: local, CI, and release all run this one script.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$root"

package=dekopon-gpt-image-provider
component=gpt-image-provider.wasm
checksum=gpt-image-provider.wasm.sha256
core=target/wasm32-unknown-unknown/release/dekopon_gpt_image_provider.wasm
wasm_tools_version=1.259.0
msrv=1.98.1

command -v jq >/dev/null 2>&1 || {
  echo "error: jq is required" >&2
  exit 1
}
command -v wasm-tools >/dev/null 2>&1 || {
  echo "error: wasm-tools $wasm_tools_version is required" >&2
  exit 1
}
if [[ "$(wasm-tools --version)" != "wasm-tools $wasm_tools_version" ]]; then
  echo "error: expected wasm-tools $wasm_tools_version" >&2
  exit 1
fi
if ! rustup run "$msrv" rustc --version >/dev/null 2>&1; then
  echo "error: Rust $msrv is required for the declared MSRV check" >&2
  exit 1
fi

cargo "+$msrv" check --locked --all-targets --package "$package"
cargo fmt --all -- --check
cargo test --locked --package "$package"
cargo clippy --all-targets --locked --package "$package" -- -D warnings
cargo check --locked --package "$package" --target wasm32-unknown-unknown
cargo clippy --locked --package "$package" --target wasm32-unknown-unknown --lib -- -D warnings

# The mirrored WIT, against the exact sources Cargo resolved. CI fetches them from the pinned commit
# over the network; this compares the checkout Cargo already has, so the gate works offline too.
metadata=$(cargo metadata --locked --format-version 1)
sdk_manifest=$(jq -er '.packages[] | select(.name == "dekopon-provider-sdk") | .manifest_path' <<<"$metadata")
http_manifest=$(jq -er '.packages[] | select(.name == "dekopon-provider-http") | .manifest_path' <<<"$metadata")
cmp "$(dirname "$sdk_manifest")/wit/provider.wit" wit/deps/provider.wit
cmp "$(dirname "$http_manifest")/wit/deps/http.wit" wit/deps/http.wit
# The world this component declares is the CLI world, so the component must export `run-command`.
grep -Fq 'include dekopon:provider/provider-cli@0.3.0;' wit/provider.wit

mkdir -p target/validation
cargo tree --locked --target wasm32-unknown-unknown --edges normal,build \
  --prefix none --format '{p}' | sort -u >target/validation/deps.tree
if grep -Eqi '^(wasi([^[:alnum:]]|$)|wasm-bindgen([^[:alnum:]]|$)|js-sys([^[:alnum:]]|$))' target/validation/deps.tree; then
  echo "error: forbidden ambient dependency" >&2
  grep -Ein '^(wasi([^[:alnum:]]|$)|wasm-bindgen([^[:alnum:]]|$)|js-sys([^[:alnum:]]|$))' target/validation/deps.tree >&2
  exit 1
fi

# Handwritten `unsafe` is forbidden everywhere the component can reach. `src/probe.rs` is the one
# exemption: it is the measuring global allocator, it is declared `#[cfg(test)]`, and it is therefore
# not compiled into the component at all. The declaration is checked here so the exemption cannot
# quietly become a shipped one.
# Comment lines are dropped before the match: a file is allowed to *discuss* unsafety, and this
# crate's module documentation has to, since the generated bindings contain `unsafe` by construction.
found=$(grep -rn '\bunsafe\b' src --exclude=probe.rs | awk '{
  line = $0
  sub(/^[^:]+:[0-9]+:/, "", line)
  if (line !~ /^[[:space:]]*(\/\/|\*)/) { print }
}')
if [[ -n "$found" ]]; then
  echo "error: handwritten unsafe source is forbidden outside the test-only allocator" >&2
  echo "$found" >&2
  exit 1
fi
if ! grep -A1 '^#\[cfg(test)\]$' src/lib.rs | grep -Fxq 'mod probe;'; then
  echo "error: src/probe.rs must be declared as a #[cfg(test)] module" >&2
  exit 1
fi

./build.sh
test -s "$component"
test -s "$checksum"
test -s "$core"
wasm-tools validate "$core"
wasm-tools validate "$component"
wasm-tools metadata show "$core" >target/validation/core-metadata.txt
wasm-tools metadata show "$component" >target/validation/component-metadata.txt
grep -F 'wit-bindgen-rust' target/validation/component-metadata.txt >/dev/null

wasm-tools print "$core" | grep '(import ' >target/validation/core-imports.txt || true
test -s target/validation/core-imports.txt
if grep -Fv 'dekopon:http/client@1.0.0' target/validation/core-imports.txt; then
  echo "error: unexpected core import" >&2
  exit 1
fi
if [[ $(wc -l <target/validation/core-imports.txt | tr -d ' ') != 1 ]]; then
  echo "error: expected exactly one core import" >&2
  exit 1
fi

wasm-tools component wit "$component" >target/validation/component.wit
wasm-tools component wit -j "$component" >target/validation/component-wit.json
jq -e '
  (.worlds | length) == 1 and
  (.worlds[0].imports | length) == 1 and
  ((.worlds[0].exports | keys | sort) == ["describe", "invoke", "run-command"]) and
  (.interfaces | length) == 1 and
  (.interfaces[0].name == "client") and
  ((.interfaces[0].functions | keys) == ["send"]) and
  (.packages[.interfaces[0].package].name == "dekopon:http@1.0.0")
' target/validation/component-wit.json >/dev/null
if grep -Eq 'wasi:|resolve-command' target/validation/component.wit; then
  echo "error: component exposes an ambient import or an unexpected export" >&2
  exit 1
fi

expected=$(awk '{print $1}' "$checksum")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$component" | awk '{print $1}')
else
  actual=$(shasum -a 256 "$component" | awk '{print $1}')
fi
[[ "$actual" == "$expected" ]]
[[ $(awk 'NF {count++} END {print count+0}' "$checksum") == 1 ]]
[[ $(awk '{print $2}' "$checksum") == "$component" ]]

for forbidden in "$root" "${CARGO_HOME:-$HOME/.cargo}" "$(rustc --print sysroot)"; do
  if LC_ALL=C grep -aF -- "$forbidden" "$component" >/dev/null; then
    echo "error: component embeds local path $forbidden" >&2
    exit 1
  fi
done

printf 'all provider shipping gates passed; size=%s sha256=%s\n' \
  "$(wc -c <"$component" | tr -d ' ')" "$actual"
