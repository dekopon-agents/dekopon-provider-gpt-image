# dekopon-provider-gpt-image

GPT Image generation and editing for [Dekopon](https://github.com/dekopon-agents/dekopon), as a
WebAssembly component. Version **0.3.0** requires the **0.18.0** SDK/runtime asset contract.
Billing uses a ChatGPT subscription, not a platform API key: this calls the route Codex calls.

**One invocation is one POST, with no retry.** Repeating a call creates another image and spends
more quota. No live paid calls are needed by the tests.

## Commands and capabilities

```console
image generate --prompt "a tangerine on a cluttered desk, warm afternoon light"
image edit --image chat-asset:1 --prompt "repaint as a loose watercolour sketch"
image edit --image chat-asset:1 --image chat-asset:2 --prompt "combine the compositions"
image generate --prompt -
image --help
```

| Capability | Input | Effect | Risk |
|---|---|---|---|
| `gpt-image.generate` | `{prompt}` | external-write | Medium |
| `gpt-image.edit` | `{prompt, images}` | external-write | Medium |

`prompt` is 1–16 KiB of UTF-8 after trimming. `--prompt -` reads stdin. `images` contains **one to
five `chat-asset:<N>` references**, with PNG, JPEG or WebP content types. The broker enforces
8 MiB decoded per asset and 40 MiB decoded per invocation. Paths, remote URLs and data URLs are
refused, including by the pure command facade before proposing. Help and proposals never call
host imports; only authorized `invoke` resolves references.

The gateway passes referenced assets out of band; proposal JSON is not rewritten. The component
opens each reference and uses its validated content type in an `image_url` data-URL string on the
**upstream HTTP wire only**. Literal JSON segments surround base64 asset parts; the host encodes
and streams those parts at exact length. Input image bytes never enter guest memory.

There is no `quality`, `size`, `background`, `model`, `n`, or `outputFormat` input. The service
accepts but ignores the fixed Codex request settings. Steer subject, style, composition and
orientation through the prompt instead. Both schemas are closed and native validation enforces
the contract. The command word is `image` because capability-shaped words such as `gpt-image`
are refused by the shell.

## Output: attach is not send

The response handle is read once and parsed with `b64_json` **borrowed as `&str`**. The provider
checks standard base64, decoded size and the PNG signature, then allocates an `image/png` writer
with base64 storage, writes that borrowed slice and attaches it. It never decodes the full image
or copies base64 into result JSON. Only the first upstream image is used.

```json
{
  "image": {"generationId": "img_…", "bytes": 2174054, "quality": "low", "size": "1024x1536",
            "background": "opaque", "outputFormat": "png"},
  "model": "gpt-image-2",
  "usage": {"inputTokens": 26, "outputTokens": 772, "totalTokens": 798},
  "requestId": "<x-codex-imagegen-request-id>"
}
```

Optional metadata is omitted when absent or not a bounded identifier-shaped token. `image.bytes`
is the decoded size. No `attachments` byte envelope is returned. The gateway assigns the attached
output a reference and adds an `assetNote` naming its reference, content type and stored size.
Use the separate asset provider's send command, authorized by **`asset.send`**, to deliver it.
Generation/editing alone does **not** deliver anything. A failed invocation admits no asset effects.

Old `providerAttachments` and `chatAssetInputs` route configuration must be removed; they are not
compatibility switches. See [the example](examples/chat-image-studio/README.md).

## Errors

| Code | When |
|---|---|
| `invalid-input` | invalid fields, prompt, reference count/syntax/type, or HTTP request size |
| `upstream-unauthorized` | 401/403; operator must re-login the image credential |
| `upstream-quota` | 429, with validated quota/reset hints when present |
| `upstream-rejected` | other 4xx, with bounded upstream code/message |
| `upstream-failure` | other non-success status, transport failure or HTTP denial |
| `response-invalid` | invalid JSON/base64/PNG or contradictory output format |
| `response-too-large` | decoded output exceeds 8 MiB, or HTTP response exceeds host allowance |
| `asset-failure` | asset open/read/allocate/write/attach failed; stable host code only |

Non-401/403/429 refusals quote only the upstream error code/type and message, with control
characters removed and a 240-character bound. Bodies beyond 64 KiB are not parsed for that
quotation. The broker echo-scans streamed responses; host diagnostic text and credentials are never
forwarded by the component. An asset failure after the POST does not imply the paid action was
undone. Nothing retries automatically.

## Broker configuration

The component never sets `authorization` or `chatgpt-account-id`. The broker injects them for the
credential's bound destinations and refuses those headers from guests. Use a separate broker-owned
`chatgptSubscription` credential family, not the gateway model's auth file:

```console
dekopond auth chatgpt login --auth-file ~/.config/dekopon/chatgpt-auth.gpt-image.json
```

The broker owns rotation. Never point broker and gateway at the same credential file.

Both image capabilities need HTTP **and attach** grants:

```yaml
assets:
  rootPath: /var/lib/dekopon-assets  # broker-owned directory, mode 0700
  maxInFlightBytes: 67108864
hostLimits:
  maxHttpResponseBytes: 12582912
  maxTimeoutMs: 300000
constraintSets:
  gpt-image.generate:              # gpt-image.edit has the same shape
    provider: gpt-image
    effect: external-write
    risk: Medium
    credential: chatgpt-gpt-image
    constraints:
      timeoutMs: 240000
      http:
        allowedHosts: [chatgpt.com]
        allowedMethods: [POST]
        maxRequests: 1
        maxRequestBytes: 1048576
        maxResponseBytes: 12582912
        allowPlaintextLoopback: false
      asset: { attach: true }
```

No multi-megabyte JSON input/output/frame overrides are required. Request asset parts use asset
limits; HTTP `maxRequestBytes` bounds literal bytes and headers. **Streamed responses still obey
HTTP `maxResponseBytes`**, so retain the 12 MiB response grant and process ceiling for an 8 MiB PNG
in a base64 JSON response. The default 4 MiB HTTP response ceiling is insufficient. The broker must
have its assets directory configured; it owns spooling and in-flight disk accounting. The timeout
ceiling admits the four-minute capability timeout. Leave the per-store memory default unchanged.
Explicit delivery needs the asset provider, a matching `asset.send` constraint set and Cedar grant.

## Upstream facts and limits of testing

Endpoints are fixed at `https://chatgpt.com/backend-api/codex/images/{generations,edits}` with model
`gpt-image-2`. The fixed Codex shape is JSON in/out, no SSE, polling, partial images or follow-up
URL download. This undocumented first-party route was researched against `openai/codex` at
`ea53c8d`. Historical live observations on 2026-09-10: 23–37-second calls, server-chosen PNG
quality/size, and ~1,500 input tokens for one 2.4 MB reference image. These are observations, not
service guarantees. Single-account use only; do not pool credentials.

Native injected-transport tests pin exact one- and five-image HTTP bodies and fixed composed lengths,
one POST/no retry, output attachment sequencing, failure short-circuiting, bounded metadata and
borrowed payload addresses. A counting allocator covers an approximately 8 MiB output with five
input references and a metadata-only SDK envelope. These tests do not prove live upstream support,
real host streaming/Content-Length, cross-UID transfer, or chat delivery; those belong to host and
rollout integration. No asset/stream testkit adapter exists in SDK 0.18.0.

## Build and validate

Exact crates.io pins: `dekopon-provider-sdk =0.18.0`, `dekopon-provider-http =0.18.0`,
`wit-bindgen =0.62.0`. Toolchain: Rust 1.98.1, wasm-tools 1.259.0. WIT mirrors match the resolved
published crates; the world imports `dekopon:http/client@1.1.0` and `dekopon:asset/asset@0.1.0`.

```console
../provider-workflows/build.sh
cargo fmt --all -- --check
cargo deny --all-features check bans licenses sources advisories
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --locked --target wasm32-unknown-unknown --lib -- -D warnings
cargo test --locked
```

CI is the pinned shared `dekopon-agents/provider-workflows` workflow, check `ci / validate`:
mirrors, lint/policy checks, component inspection, native tests and independent reproducible builds.
The build script writes `gpt-image-provider.wasm` and its checksum; neither is committed.

## Releases

Tags publish the component, checksum, SBOM and provenance plus the OCI artifact
`ghcr.io/dekopon-agents/provider-gpt-image`. The shared workflow attests **component and SBOM files**,
not the OCI manifest. Verify the downloaded component:

```console
gh attestation verify gpt-image-provider.wasm --repo dekopon-agents/dekopon-provider-gpt-image
```

Bind its verified SHA-256 to the immutable OCI manifest's sole WASM layer; do not claim the
manifest itself is attested. Release builds independently reproduce before publication.

## License

MIT or Apache-2.0, at your option.
