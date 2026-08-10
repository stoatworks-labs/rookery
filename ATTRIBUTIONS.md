# Attributions

rookery is MIT licensed. It builds on the following.

## Within this fleet

- **[WebLinked](https://github.com/stoatworks-labs/weblinked)** — the thing
  rookery drives. Its OSC verb set and `/api/state` shape are the protocol
  this project speaks; `docs/protocol.md` records what was read from its
  source and what was measured against a running instance.
- **[flock](https://github.com/stoatworks-labs/flock)** — the fleet control
  panel this is modelled on. `crates/diag` is vendored from it unchanged, and
  `crates/core/src/crypto.rs` and the subnet-probe rules in
  `crates/discovery` are adapted from it.

## Rust crates

Direct dependencies, all MIT or Apache-2.0 unless noted:

| Crate | Used for |
|---|---|
| `tokio` | async runtime, UDP and TCP sockets |
| `axum` | HTTP server and websocket |
| `reqwest` | HTTP client for state polling |
| `serde`, `serde_json`, `toml` | serialisation and config |
| `aes-gcm` | encrypting instance tokens at rest |
| `uuid` | instance ids |
| `futures` | concurrent fan-out and polling |
| `if-addrs` | enumerating local subnets for discovery |
| `anyhow` | error handling |
| `tracing`, `tracing-subscriber`, `tracing-appender` | logging |
| `time` | log timestamps |

Run `cargo tree` for the full transitive set and `cargo license` for a
machine-readable licence breakdown.

## Protocol

**Open Sound Control 1.0** — the specification is public
(<https://opensoundcontrol.stanford.edu/spec-1_0.html>). The codec in
`crates/osc` is an original implementation written against that spec; no OSC
library is vendored or linked.
