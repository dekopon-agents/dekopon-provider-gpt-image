# chat-image-studio

An end-to-end walkthrough: a Slack DM asks for an image, a model writes one shell line, the broker
authorizes it and spends a ChatGPT subscription's image allowance, and the PNG comes back into the
thread as an attachment the model never saw.

Five files, and the split between them is the point:

| File | Holds | Reads |
|---|---|---|
| `broker.yaml` | the socket, the constraint sets, the credential binding, the audit log | the component, `policies.cedar`, `broker-credentials.yaml` |
| `policies.cedar` | who may act, through which gateway, as which agent | — |
| `broker-credentials.yaml` | the *path* to a ChatGPT credential file the broker owns | that file |
| `dekopon.yaml` | which agent answers and which capability words it may spell | — |
| `dekopond.yaml` | Slack tokens, a model endpoint, the route's two opt-ins | `dekopon.yaml` |

The gateway holds no image credential and no policy. The broker holds no Slack token and never reads
the catalog. Neither the model nor the shell session ever observes the ChatGPT access token: it is
injected inside the broker's native HTTP engine, for `chatgpt.com` only, after the guest's headers
have been validated.

> **Versions.** The broker credential kind `chatgptSubscription`, and the two route keys
> `providerAttachments` and `chatAssetInputs`, land in **dekopon 0.13.0** — the same release that
> publishes the `dekopon:provider@0.3.0` WIT this component is built against. On 0.12.0 this example's
> `broker-credentials.yaml` and `dekopond.yaml` refuse startup by naming the unknown fields, which is
> the intended behaviour: an unknown key is a typo until the release that defines it.

## Running it

```console
# 1. A ChatGPT credential family of its own, for the broker. Never share the gateway's file.
dekopon auth chatgpt login --auth-file ~/.config/dekopon/chatgpt-auth.gpt-image.json
chmod 0600 ~/.config/dekopon/chatgpt-auth.gpt-image.json

# 2. Point the broker at it.
cp broker-credentials.yaml.example broker-credentials.yaml
$EDITOR broker-credentials.yaml          # set authFile to the absolute path above
chmod 0600 broker-credentials.yaml broker.yaml policies.cedar dekopond.yaml

# 3. The component the broker loads.
(cd ../.. && ./build.sh)

# 4. Both halves.
dekopon-brokerd --config broker.yaml
DEKOPOND_SLACK_APP_TOKEN=xapp-… DEKOPOND_SLACK_BOT_TOKEN=xoxb-… dekopond --config dekopond.yaml
```

Then, in a DM with the app:

> draw me a tangerine on a cluttered desk, warm afternoon light, vertical poster

The model runs one line, and the image arrives as an upload:

```
image generate --prompt "a tangerine on a cluttered desk in warm afternoon light, tall vertical poster composition"
```

Attach a photograph and ask for a repaint, and it runs:

```
image edit --image chat-asset:1 --prompt "repaint as a loose watercolour sketch; keep the composition"
```

## What each opt-in buys

**`providerAttachments: {maxPerReply: 1}`** is what lets provider bytes reach a chat at all. The
component returns a reserved top-level `attachments: [{mediaType, base64}]` key; the gateway's broker
leg strips it, validates each entry (`image/png`, ≤ 8 MiB, PNG signature), routes the bytes to the
reply's image slot, and replaces the key with `attached: [{mediaType, bytes}]` so the model and the
shell see metadata only. Without the key on the route, the attachment is refused with fixed gateway
text and an audit event — the capability still runs and still costs quota, so a route that can call
this provider should have the opt-in.

A base64 blob is never printed into a transcript. The shell has no byte type and would clamp it to
~128 KiB of garbage in the model's context; that is the reason the convention exists at all.

**`chatAssetInputs: [gpt-image.edit]`** is the inbound half. On a listed capability the gateway walks
the proposal's input JSON and replaces any string exactly matching `chat-asset:<N>` with a
`data:<mime>;base64,<bytes>` URL, bounded at three expansions and 8.5 MiB decoded per invocation.
Unlisted, or over budget, and the proposal is refused before it is made — the component would
otherwise receive the literal marker, which it rejects by name:
`invalid-input: route does not allow chat asset inputs for gpt-image.edit`.

## What the ceilings are for

`broker.yaml` raises six `hostLimits` to 12 MiB-ish values and sets `serverLimits.maxFrameBytes` to
14 MiB. That is one number propagating: an 8 MiB PNG is ~10.7 MiB of base64, a result carrying it plus
its envelope needs ~11 MiB, the frame must hold the result plus 64 KiB, and the protocol's hard cap is
16 MiB. So **one result carries one image** — which is also why `maxPerReply` is 1 and why the agent's
instructions say one image per call.

`hostLimits.maxMemoryBytes` is deliberately *not* raised. It stays at the 64 MiB default that every
provider's store reserves against `maxTotalMemoryBytes`, and the component is measured against it: see
the memory table in the [top-level README](../../README.md#memory-which-is-the-real-constraint).

`maxInputBytes` is process-global with no per-capability knob, so raising it for this provider raises
it for every provider on the broker. That is an accepted cost here, bounded by the shell's own value
budget and by the frame.

## What to expect in the audit log

One `invocation` record per image, naming `gpt-image.generate` or `gpt-image.edit`, the principal, the
policy ids that permitted it, `credentialInjected: true`, and the symbolic credential name
`chatgpt-gpt-image` — never a token. One `HttpCallEvidence` entry for the single POST, with the
injected headers excluded from the accounted bytes. With telemetry on, a
`broker.credential.refresh` span appears the first time the broker rotates its own ChatGPT token,
`outcome=rotated`.

A 429 from the route shows up as `upstream-quota` with the refusal type and `x-codex-active-limit`, and
the component does not retry: the allowance is spent and trying again would only spend more.
