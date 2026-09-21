# Chat image studio — asset-handle example

This example configures GPT Image 0.3.0 on Dekopon 0.18.0. A mapped Slack sender can generate a
PNG or remix one to five conversation assets. Image bytes are never in proposal/result JSON.

**This example grants generation/editing and attachment, not delivery.** To send the generated
file to Slack, install the independently released `dekopon-provider-asset`, add its `asset.send`
constraint set using that release's exact effect/risk, grant it to the same principal in Cedar,
and add the capability/provider to the agent catalog. Follow that provider's command help to send
the returned reference. Do not assume image generation sends automatically.

## Files

- `broker.yaml`: provider registration, broker-owned assets directory, constraints and identity map.
- `broker-credentials.yaml.example`: copy to `broker-credentials.yaml`, point it at a separate
  broker-owned ChatGPT subscription auth file, and chmod 0600.
- `policies.cedar`: only the mapped `artist` through `dekopond-gateway` and this agent may invoke
  generation/editing. There is deliberately no implicit send grant.
- `dekopon.yaml`: agent instructions and catalog; no authority is granted here.
- `dekopond.yaml`: Slack/model connection and route. Set the documented environment-variable names,
  not secret values in these files.

Replace example absolute paths, UID, workspace/user mapping and model endpoint before starting.
Use a broker-owned mode-0700 socket directory, configuration files mode 0600, and a broker-owned
mode-0700 asset root at `assets.rootPath`. Startup clears stale asset-root entries. Build the
component using `../provider-workflows/build.sh` (with provider-workflows cloned beside the
provider), or its absolute path, **from the provider root**, to produce `gpt-image-provider.wasm`.

Create the independent broker credential family:

```console
dekopond auth chatgpt login --auth-file ~/.config/dekopon/chatgpt-auth.gpt-image.json
```

Never reuse the gateway model's auth file; the broker owns refresh of its credential family.
Slack delivery needs `files:write`; input fetching needs `files:read`. Discord delivery needs
Attach Files. No live deployment or credential login is performed by provider tests.

## Handle flow

```text
image edit --image chat-asset:1 --prompt "repaint as a watercolour"
  → pure proposal, unchanged reference
  → broker authorization (HTTP + asset.attach)
  → open handle → streamed POST → response handle
  → borrowed base64 → PNG writer → attach
  → metadata + gateway assetNote with a new chat-asset reference
  → separate authorized asset.send → reply delivery
```

The last step requires the extra configuration above. If it is absent, the example agent reports
that delivery is not configured instead of claiming success.

Remove old `providerAttachments` and `chatAssetInputs` keys; neither is used in 0.18.0. The gateway
resolves exact reference leaves automatically. A reclaimed/missing asset is refused, not fetched
again. Only PNG/JPEG/WebP inputs are accepted by this provider. Output is PNG. Attach/send are
separate, and no error retries a paid POST.

## Bounds

The asset host enforces 8 MiB decoded per asset, five inputs/outputs and 40 MiB decoded per
invocation. `maxInFlightBytes: 67108864` is disk-spool accounting, not a JSON-frame allowance.
No large `maxInputBytes`, `maxOutputBytes`, `maxHttpRequestBytes` or `maxFrameBytes` overrides remain.
The request literal/header budget is 1 MiB; asset parts are host-streamed separately.

**Keep the 12 MiB HTTP response ceiling and grant.** SDK/runtime 0.18.0 still charges streamed
response bytes against `http.maxResponseBytes`, including the base64 JSON returned by upstream.
Its default process ceiling is only 4 MiB. `timeoutMs: 240000` and the process timeout ceiling
allow slow generations. The default per-store memory limit is unchanged.

Native tests prove request composition and output handling without network access. They do not
prove this example against a live Slack workspace, actual credentials, or a deployed broker.
