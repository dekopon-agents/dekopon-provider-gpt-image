# dekopon-provider-gpt-image

The GPT Image provider for [Dekopon](https://github.com/dekopon-agents/dekopon), as a WebAssembly
component. Two capabilities: generate one image from a prompt, or remix one to three images with a
prompt. The bill goes to a ChatGPT subscription rather than a platform API key, because the route it
calls is the one Codex calls.

One invocation is one `POST`, and one image. There are no retries: the route spends the account's
image allowance, so a repeat is a second image and a second charge.

The component never sets `authorization` or `chatgpt-account-id`. The broker injects both at the
native HTTP boundary for destinations inside its binding, where no guest can observe them, and
rejects a guest that tries to set either rather than overwriting it.

## Capabilities

| Capability | Input | Effect | Risk |
|---|---|---|---|
| `gpt-image.generate` | `{prompt}` | external-write | Medium |
| `gpt-image.edit` | `{prompt, images}` | external-write | Medium |

`prompt` is 1–16 KiB of UTF-8, trimmed. `images` is one to three `data:image/(png|jpeg|webp);base64,…`
URLs, at most 8 MiB decoded each.

**There is no `quality`, `size`, `background`, `model`, or `outputFormat` field, because the route
has no such controls.** It accepts those request fields and ignores them: the service picks the
quality, the size, and the format itself, and always returns PNG. Orientation and aspect *are*
steerable — through the prompt. Ask for "a tall portrait poster", "a wide landscape banner",
"square", and the service obliges. Both input schemas are closed (`additionalProperties: false`) and
a model that types `--size 1024x1024` gets a usage error naming the flag rather than a silent no-op.

### Verified on 2026-09-10

Nine quota-costing calls with a freshly minted Codex token family, against
`https://chatgpt.com/backend-api/codex/images/{generations,edits}`. Every request body was the fixed
Codex body with one field varied.

| Request field sent | Value | HTTP | Echoed quality | Echoed size | Echoed background | PNG bytes | Seconds |
|---|---|---|---|---|---|---|---|
| `quality` | `high` | 200 | `medium` | `1370x1148` | `opaque` | 1.9–2.4 MB | 32–37 |
| `quality` | `xhigh` | 200 | `medium` | `1370x1148` | `opaque` | 1.9–2.4 MB | 32–37 |
| `quality` | `low` | 200 | `medium` | `1370x1148` | `opaque` | 1.9–2.4 MB | 32–37 |
| `size` | `1536x1024` | 200 | `medium` | `1370x1148` | `opaque` | 1.9–2.4 MB | 32–37 |
| `size` | `1024x1536` | 200 | `medium` | `1370x1148` | `opaque` | 1.9–2.4 MB | 32–37 |
| `background` | `transparent` | 200 | `medium` | `1370x1148` | `opaque` | 1.9–2.4 MB | 32–37 |
| `model` | `gpt-image-2.5-flare` | 200 | `medium` | `1370x1148` | `opaque` | 1.9–2.4 MB | 32–37 |
| `output_format` | `jpeg` | 200 | `medium` | `1324x1188` | `opaque` | — | — |
| *(none; the prompt asked for a "vertical 2:3 portrait poster")* | — | 200 | `low` | **`1024x1536`** | `opaque` | 2,174,054 | 23 |
| *(edit, one 2.4 MB PNG as `images[0].image_url`)* | — | 200 | `low` | `1370x1148` | `opaque` | 3,158,093 | 37 |

What that table says, in one sentence: **the service picks quality, size, and format; the request
fields are accepted and ignored; the prompt is the only thing that steers the result.** The
`background: transparent` call returned an RGB PNG with no alpha channel. The `output_format: jpeg`
call echoed `output_format: "png"`. Sizes observed across the nine calls were 1370×1148 (six times),
1369×1149, 1324×1188, and 1024×1536 — server-chosen per call, and none of them one of the documented
platform sizes except the prompt-steered portrait. Quality came back `medium` on most calls and
`low` on two.

Costs, for budgeting: a generate charged 26 input tokens and 429–829 output tokens (all image
tokens). One 2.4 MB reference image charged **~1,500 input tokens** (1,470 of them image tokens), so
an edit is an order of magnitude more expensive on input than a generate. Image generation burns a
plan's limits several times faster than text.

Still unverified, and deliberately not offered: `n` > 1, `mask`, `input_fidelity`, remote HTTP
reference URLs, and whether any `model` identifier other than `gpt-image-2` ever changes the result.
The component always sends `gpt-image-2`, which is what Codex sends.

## The `image` command word

```
image generate --prompt "a tangerine on a cluttered desk, warm afternoon light"
image edit --image chat-asset:1 --prompt "repaint as a loose watercolour sketch"
image generate --prompt -            # reads the value piped into the word
image --help                         # rendered by the guest, at exit 0
```

`--image` repeats, up to three. `--help`, `--version`, and every usage error are rendered inside the
component and authorize nothing. A well-formed argv becomes a *proposal*, which then travels the
identical path a direct `cap gpt-image.generate {…}` call takes: constraint-set lookup, Cedar,
credential injection. Naming a capability the caller was not granted is a denial, not an escalation.

The word is `image` rather than `gpt-image` because `dekopon-core` refuses a command word that parses
as a capability identifier.

## Result shape

```json
{
  "attachments": [{"mediaType": "image/png", "base64": "<b64_json, passed through untouched>"}],
  "image": {"generationId": "img_…", "bytes": 2174054, "quality": "low", "size": "1024x1536",
            "background": "opaque", "outputFormat": "png"},
  "model": "gpt-image-2",
  "usage": {"inputTokens": 26, "outputTokens": 772, "totalTokens": 798},
  "requestId": "<x-codex-imagegen-request-id>"
}
```

`attachments` is a gateway convention, not a broker feature: dekopond's broker leg strips the key,
validates each entry (`image/png`, ≤ 8 MiB, PNG signature), routes the bytes to the reply's image
slot, and replaces the key with `attached: [{mediaType, bytes}]` so the model and the shell see
metadata only — a base64 blob printed into a transcript would be clamped to ~128 KiB of garbage. The
route has to opt in with `providerAttachments: {maxPerReply: N}`.

`chat-asset:<N>` is the other convention, inbound: on a route that lists this capability in
`chatAssetInputs`, the gateway replaces the marker with a `data:` URL of the attachment's bytes
before proposing. An unexpanded marker reaching the component is not a malformed input, it is an
unconfigured route, and the error says so: `invalid-input: route does not allow chat asset inputs for
gpt-image.edit`.

Everything echoed from upstream — `quality`, `size`, `background`, `outputFormat`, `generationId`,
`model` — is kept only if it is a short identifier-shaped token. It is decorative data a model will
read, so anything surprising is dropped rather than forwarded into a prompt.

## Errors

No upstream body, message, or host detail is ever echoed. Seven codes:

| Code | When |
|---|---|
| `invalid-input` | the input fails the closed contract; an unexpanded `chat-asset:<N>`; a request too large for the authorized size |
| `upstream-unauthorized` | 401 or 403 — "the operator must re-login the gpt-image credential" |
| `upstream-quota` | 429, with the refusal type, `x-codex-active-limit`, and `retry-after` when present |
| `upstream-rejected` | any other 4xx, with the status |
| `upstream-failure` | 5xx, a timeout, a transport failure, or a broker denial |
| `response-invalid` | not JSON, no `data[0].b64_json`, not standard base64, not a PNG, or an `output_format` that disagrees with the bytes |
| `response-too-large` | the success envelope would exceed 12 MiB; refused before it is assembled |

## Credential

This provider needs the broker credential kind `chatgptSubscription` (dekopon 0.13.0): a
`{authFile, destinations}` entry naming a Dekopon ChatGPT credential file that the **broker** owns
and refreshes. It is a deliberately separate token family from the gateway's own model credential —
same ChatGPT account, independent refresh token — so neither can race the other's rotation and
either can be revoked alone:

```console
dekopond auth chatgpt login --auth-file ~/.config/dekopon/chatgpt-auth.gpt-image.json
```

Never point the broker and the gateway at one file. The access token the login mints was observed
valid for roughly ten days, so a refresh is rare but the broker still owns it.

## Constraint set and ceilings

The component grants nothing on its own. An operator points `dekopon-brokerd` at it and writes a
constraint set per capability; the broker refuses to start when one disagrees with the manifest.
These are this deployment's values — see [`examples/chat-image-studio/`](examples/chat-image-studio/README.md)
for the whole walkthrough:

```yaml
gpt-image.generate:            # and gpt-image.edit, identically
  provider: gpt-image
  effect: external-write
  risk: Medium
  credential: chatgpt-gpt-image
  constraints:
    timeoutMs: 240000
    maxOutputBytes: 12582912
    http:
      allowedHosts: [chatgpt.com]
      allowedMethods: [POST]
      maxRequests: 1
      maxRequestBytes: 12582912
      maxResponseBytes: 12582912
      allowPlaintextLoopback: false
```

```yaml
hostLimits:                    # process-global; every other ceiling a constraint set may only narrow
  maxInputBytes: 12582912
  maxOutputBytes: 12582912
  maxHttpRequestBytes: 12582912
  maxHttpResponseBytes: 12582912
  maxTimeoutMs: 300000
  maxTotalMemoryBytes: 268435456
serverLimits:
  maxFrameBytes: 14680064       # ≥ maxOutputBytes + 64 KiB, and under the 16 MiB protocol hard cap
```

`hostLimits.maxMemoryBytes` stays at its 64 MiB default: raising it would tax every provider's store
reservation on a shared Pi. The 12 MiB ceilings are sized for an 8 MiB PNG — 10.7 MiB of base64 plus
an envelope — and the largest image actually observed was 3.2 MB (4.2 MB of base64), so there is
roughly 3× headroom on the measured sizes. `timeoutMs: 240000` is sized for the observed 23–37 s plus
a wide margin; the route sets no timeout of its own and the tool's own documentation says to allow
several minutes.

## Memory, which is the real constraint

An 8 MiB PNG is ~10.7 MiB of base64, the guest store is 64 MiB, and the result has to carry the blob
back out. `src/response.rs` documents the discipline: move the body into a `String` without copying
it, parse it with the blob **borrowed** from that buffer, verify the PNG signature by decoding only
the first sixteen base64 characters, compute `bytes` from the base64 length, measure the envelope as
the serialized skeleton plus the blob's own length, and then trim that same buffer in place with
`drain` and `truncate` so the result owns the allocation the bytes arrived in. Nothing decodes the
image and nothing copies it.

`cargo test` measures this with a counting global allocator rather than asserting it, and reports two
numbers per case because a geometrically growing buffer has two honest answers — one for an allocator
that extends a block in place (what dlmalloc does for a chunk at the top of the heap, which the
newest large allocation is) and one for an allocator that must copy:

| Measured | In place | Copying |
|---|---|---|
| This component alone, 11.18 MB response | 10.7 MiB | 10.7 MiB |
| A full generate, including the SDK's envelope | 32.0 MiB | 42.7 MiB |
| A full edit, 8 MiB input image + 11.18 MB response | 42.7 MiB | 53.3 MiB |

The component's own cost is exactly one copy of the image. The rest is the SDK: `Provider::invoke`
takes and returns a `serde_json::Value`, so the input arrives already parsed into an owned tree while
the original JSON string stays alive, and the result is serialized out of a `Value` by a buffer that
doubles as it grows. A worst-case edit therefore sits at 53 MiB of the 64 MiB store in the pessimistic
model. It fits, and it is the reason `maxMemoryBytes` does not need raising — but the headroom is
thinner than the generate path's, and the lever that would widen it is an SDK `invoke` that borrows
its input and returns a pre-serialized result, not anything a provider can do for itself.

## Upstream facts

Researched against `openai/codex` at commit `ea53c8d`. `POST https://chatgpt.com/backend-api/codex/images/{generations,edits}`,
JSON in and JSON out, no SSE, no partial images, no background-job polling, no download-URL
follow-up. Codex sends `{prompt, model: "gpt-image-2", background, quality, size}` plus
`images: [{image_url: "data:image/png;base64,…"}]` for edits, and this component sends byte-identical
bodies. The response is `{created, data: [{b64_json, generation_id?}], background?, quality?, size?,
output_format?, usage?}` with unknown fields ignored, and `x-codex-imagegen-request-id` carries the
image-generation request id. A 429 may carry `error.type ∈ {usage_limit_reached, usage_not_included}`
and `x-codex-active-limit`, but not every 429 does.

The route is undocumented and Codex reaches it with OpenAI's first-party client id. No OpenAI page
permits or forbids a third-party client against it. Single-account use is the only use this is built
for; do not pool the credential.

## Building

```console
rustup toolchain install 1.98.1 --profile minimal
cargo install wasm-tools --version 1.259.0 --locked
./scripts/validate.sh
```

`scripts/validate.sh` is the whole gate — MSRV, fmt, clippy, tests, the wasm target, the WIT mirror
against the resolved sources, the ambient-dependency and `unsafe` rejectors, and then the component's
imports and exports — and it is what CI and the release workflow run too.

`build.sh` is a self-contained port of dekopon's `examples/providers/build-component.sh` and keeps
every mechanism that made the in-tree component reproducible: a `rustc` proxy that normalizes
`-Cmetadata` to the fixed `dekopon-provider-repro-v1` salt, `--remap-path-prefix` for the source
root, the Cargo home and the toolchain sysroot, `-Ccodegen-units=1`, and a final scan that fails the
build if any local path survives into the component. Given the same source and the same two pins it
lands on the same bytes on any machine, which CI asserts by building twice into independent trees and
comparing.

`cargo test` runs everything natively; nothing contacts chatgpt.com.

### The SDK pin

`dekopon-provider-sdk` and `dekopon-provider-http` are pinned to `= "0.13.0"` from crates.io — the
first published SDK carrying `dekopon:provider@0.3.0`, whose `provider-cli` world exports
`run-command`, the facade this component's world includes. An exact version, never a branch: a
`branch =` dependency resolves by fetching the ref, so deleting the branch upstream breaks every cold
build. The CI WIT-mirror step reads `Cargo.lock` and resolves a registry source to tag `v0.13.0`.

`wit-bindgen` is pinned to `=0.62.0` to match what the SDK generates its own bindings with; two
wit-bindgen runtimes in one component define `cabi_realloc` twice. That pin, `rust-toolchain.toml`,
and the `wasm-tools` version in `build.sh` and `scripts/validate.sh` move in lockstep with dekopon's
own — 1.98.1 and wasm-tools 1.259.0 at 0.13.0 — because wit-bindgen's generated code must agree with
the wasm-tools CLI.

## Releases

Each tag publishes `gpt-image-provider.wasm` two ways:

- a **release asset** with a `.sha256` and a provenance attestation, verifiable with
  `gh attestation verify gpt-image-provider.wasm --repo dekopon-agents/dekopon-provider-gpt-image`;
- an **OCI artifact** at `ghcr.io/dekopon-agents/provider-gpt-image`, pullable by tag or digest.

The release workflow rebuilds the component into an independent checkout and byte-compares before
publishing, so a tag that ships is a tag that reproduced. Attestation proves who built the artifact;
the rebuild proves what it was built from.

## License

MIT or Apache-2.0, at your option.
