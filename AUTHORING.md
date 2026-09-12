# Authoring log

The chronological build record for this provider. It records only work that happened; release results
will be appended after a release exists.

## 2026-09-10 — contract, and a contract change

1. Confirmed the target directory did not exist, created the repository with no remote, and left the
   dekopon checkout and the two sister provider checkouts (`dekopon-provider-gh`,
   `dekopon-provider-mediawiki`) untouched and read-only.
2. Read the SDK at the pinned commit rather than the published crate: `Provider`, `CommandRun`,
   `cli::run_command`, `export_provider_with_cli!`, and the `provider-cli` world that carries
   `run-command`. Mirrored `crates/dekopon-provider-sdk/wit/provider.wit` and `wit/http/http.wit`
   from that exact commit, byte for byte.
3. Scaffolded from `dekopon-provider-gh`: `build.sh` verbatim except the component and core names —
   the `dekopon-provider-repro-v1` metadata salt and all three remap prefixes are unchanged — plus its
   CI and release workflows, and `scripts/validate.sh` ported from `dekopon-provider-mediawiki`.
4. **The planned input contract was wrong, and the M0 spike is what proved it.** The plan specified
   `quality`, `size`, `background`, and `model` input fields with a size grammar and two values marked
   experimental. Nine live calls on 2026-09-10 showed the route accepts all of those fields and
   ignores every one of them: the service picks the quality and the size itself and always returns
   PNG. The size grammar, the enums, and the experimental flags were therefore never built. Inputs are
   `{prompt}` and `{prompt, images}`, the wire body is Codex's fixed body with the prompt substituted,
   and the schema descriptions tell a model to steer orientation through the prompt — which the spike
   also showed works, a prompt asking for a vertical 2:3 poster being the one call that came back
   1024×1536. The measured table is in the README.

## 2026-09-10 — implementation

- Strict native decoding (`deny_unknown_fields`) repeats every closed-schema bound, so a model that
  learned the platform API's parameters is told its field does not exist rather than believing it
  chose a size. Both validations are tested against the same inputs.
- `invoke_with(capability, input, FnMut(Request) -> Result<Response, HttpError>)` is the seam, as in
  the MediaWiki provider: every request-byte and response-projection test runs with no network and no
  host, and the seam is the one the component itself uses.
- The memory discipline is the interesting part, and it is measured rather than asserted. A counting
  `#[global_allocator]` in a `#[cfg(test)]` module reports two peaks per case — one for an allocator
  that extends a block in place, one for an allocator that must copy — because the only large
  allocation on the path that is *not* this component's is `serde_json`'s geometrically growing output
  buffer, and the honest answer for that depends on the allocator. The component's own cost is exactly
  one copy of the image: 10.7 MiB for an 11.18 MB response.

Observed friction and fixes:

- **`#[serde(borrow)]` on a `Cow<'a, str>` field does not borrow.** The plan named `Cow<'a, str>` or
  `&'a str` for the borrowing parse; only the second works. serde's `Cow` implementation always
  deserializes to `Cow::Owned`, so the first version of `response.rs` silently copied eleven megabytes
  and the `Cow::Borrowed` check it used as a guard rejected every well-formed response — which is how
  the bug was caught in under a minute instead of shipping as a quiet doubling. `b64_json` is now
  `&'a str`; the small metadata fields are owned `String`s on purpose, so a decorative `size`
  containing a JSON escape cannot fail an otherwise usable response.
- **`wit-bindgen` is pinned to `=0.46.0`, not the `=0.44.0` the plan and the sister providers name.**
  0.46.0 is what the SDK and the HTTP binding generate their own bindings with at the pinned commit,
  and two wit-bindgen runtimes in one component define `cabi_realloc` twice.
- The blob is moved out of the response buffer by trimming that buffer in place (`drain` then
  `truncate`), which needs the byte offset of a borrowed slice inside its owner. Pointer arithmetic in
  `usize` with `checked_sub`, a bounds check, and a slice comparison gets it with no `unsafe`; the
  comparison is also the proof the offset is right. A test asserts the returned `base64` has the same
  address the response body arrived at, which is a direct statement that nothing was copied.
- The success envelope is measured as the serialized skeleton plus the blob's own length rather than
  by serializing the real thing. That is exact, not an estimate, and only because the alphabet scan
  proved the base64 needs no JSON escaping — a test pins the arithmetic against a real serialization.
- `scripts/validate.sh`'s `unsafe` rejector had to learn to ignore comment lines: this crate's module
  documentation has to discuss unsafety, since the generated bindings contain `unsafe` by
  construction. The gate now drops comment lines before matching, exempts exactly `src/probe.rs`, and
  additionally asserts that `probe` is declared `#[cfg(test)]` so the exemption cannot quietly become
  a shipped one.
- A bare `image` renders the help page on stderr at status 2, not on stdout at 0. clap classifies a
  missing subcommand as a usage error whose text happens to be the help page, and the test says so
  rather than asserting the tidier behaviour.

## Validation record

```console
cargo +1.98.1 check --locked --all-targets        # MSRV
cargo fmt --all -- --check
cargo test --locked                              # 52 passed
cargo clippy --all-targets --locked -- -D warnings
cargo check --locked --target wasm32-unknown-unknown
cargo clippy --locked --target wasm32-unknown-unknown --lib -- -D warnings
./build.sh
./scripts/validate.sh
shellcheck build.sh scripts/validate.sh
actionlint
```

Observed component evidence:

- size `380545` bytes; SHA-256 `9b0434263c00e026ef56430bc2198b10f1dc05b35784f016640e70653f8f6703`;
- sole core import `dekopon:http/client@1.0.0` function `send`;
- component exports exactly `describe`, `invoke`, and `run-command`;
- no WASI import, no `resolve-command`, no banned `wasi` / `wasm-bindgen` / `js-sys` dependency, no
  handwritten `unsafe` outside the test-only allocator, and no embedded source, Cargo, or sysroot path;
- mirrored SDK and HTTP WIT matched the exact sources `Cargo.lock` resolved.

Peak live allocation, from the counting allocator:

| Measured | In place | Copying |
|---|---|---|
| the component alone, 11.18 MB response | 10.7 MiB | 10.7 MiB |
| a full generate, including the SDK's envelope | 32.0 MiB | 42.7 MiB |
| a full edit, 8 MiB input image + 11.18 MB response | 42.7 MiB | 53.3 MiB |

A second source copy, excluding `.git`, `target/`, and the generated artifacts, ran `./build.sh` with
its own ordinary default Cargo target. `cmp` matched both the Wasm and the checksum byte for byte at
the hash above. The temporary rebuild tree was then removed; `CARGO_TARGET_DIR` was never set, and
`cargo clean` was never run.

No broker smoke was run in this phase: HTTP execution requires an operator-configured
`dekopon-brokerd`, and the credential kind this provider needs (`chatgptSubscription`) lands in
dekopon 0.13.0. The local smoke recipe is in `examples/chat-image-studio/README.md`.

No GitHub repository, remote, pull request, release, tag, package, attestation, or OCI artifact had
been created by this build phase.
