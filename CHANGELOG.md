# Changelog

## [0.5.0] - 2026-10-04

### Changed

- Pin the published Dekopon SDK and testkit to 0.33.0 and build against HTTP client 1.1.0; keep asset-backed image streaming and the existing authorization boundaries unchanged.

## [0.4.0] - 2026-10-03

### Changed

- Migrate image generation and editing to the streams-only typed provider SDK 0.31.0; piped prompts are read only during authorized invocation, while broker-owned asset and quota boundaries remain unchanged.
- Preserve chat-asset reference schema constraints and CLI rendering guarantees under typed image commands.
- Pin the published core SDK and testkit exactly to crates.io 0.31.0.
