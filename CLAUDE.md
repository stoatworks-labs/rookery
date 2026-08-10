# CLAUDE.md — rookery working reference

Commands and the rules that bite. Read `AGENTS.md` for the model behind them.

## Build and test

```bash
cargo build
cargo test                                    # 76 tests, seconds
cargo clippy --all-targets --all-features
```

Tests need no hardware and no WebLinked: `rookery-instance-mock` starts
simulated instances on loopback sockets.

## Run

```bash
cp config/example.toml config/rookery.toml
cargo run -p rookery -- config/rookery.toml
```

UI on <http://127.0.0.1:8090>. Log in `~/Library/Logs/rookery/`, or set
`ROOKERY_LOG_DIR`. `ROOKERY_LOG=debug` raises the level — `debug` is what
shows every datagram.

## Verify against a real WebLinked (do not skip this)

rookery's own "sent" only means a datagram left the machine. Check the far
end.

```bash
# A WebLinked with no hardware — the preview output is enough.
~/Projects/weblinked/build/Release/WebLinked.app/Contents/MacOS/WebLinked \
  --url https://example.com --format 1080p50 --port 7654 --osc-port 7655 --headless &

curl -X POST -H 'Content-Type: application/json' \
  -d '{"name":"gfx-1","host":"127.0.0.1","tags":["stage"]}' \
  http://127.0.0.1:8090/api/instances

curl -X POST -H 'Content-Type: application/json' \
  -d '{"verb":"url","url":"https://example.com/abc"}' \
  http://127.0.0.1:8090/api/groups/stage/send

# The actual check — at the instance, not in rookery's report.
curl -s http://127.0.0.1:7654/api/state | jq .source.loaded_url
```

Use a URL whose length is **3 mod 4** when testing by hand. That is the
residue an OSC padding bug drops, and it is the one that will look like
"nothing happened" rather than an error.

To test the northbound path with something that is not rookery's own encoder,
`docs/verification.md` §3 has an independent Python sender.

## Rules

1. **Never claim a command works because it passes against the mock.** The
   mock is rookery agreeing with rookery. `docs/verification.md` is the
   authority; update it with commands and output, not ticks.
2. **A send is not a confirmation.** OSC is fire-and-forget. Nothing in the
   API or the UI may report a successful send as a confirmed change.
3. **An empty fan-out is a failure, not a no-op.** 404 from the API, a
   warning in the log, `fully_sent() == false`.
4. **`padded()` takes the text length**, never the length including the NUL.
   Keep the four-residue sweeps in `crates/osc` and
   `crates/instance-live/tests/wire.rs`.
5. **Health comes from deltas, not cumulative counters.** See
   `SourceState::dropping`.
6. **`Command` stays exactly WebLinked's OSC verb set.** Anything only
   reachable over HTTP does not become a `Command`.
7. **Groups are derived from tags**, never stored as entities.
7b. **Never change an instance's preview factor without being asked.** It is
   that instance's own output and its local control page shows the same one.
   `Instance::preview_factor` is opt-in; the poller reconciles it only when it
   can see the current value and it differs, because writing blind means
   rewriting on every poll for ever.
7c. **Interaction is armed, never ambient.** A click on a preview changes what
   is on air. The UI gates it behind take-control and the un-armed case is
   tested.
8. `diag::init` first in `main`, and hold the guard.
9. Commit means commit **and** push.

## Layout

| Path | What |
|---|---|
| `crates/core/` | instance model, registry, groups, commands, state — no sockets |
| `crates/osc/` | OSC 1.0 codec, UDP sender and receiver |
| `crates/fleet/` | fan-out, poller, northbound address grammar |
| `crates/instance-live/` | the real transport: OSC out, HTTP state in |
| `crates/instance-mock/` | simulated WebLinked on real sockets |
| `crates/discovery/` | mDNS browse for `_weblinked._tcp`, plus the active subnet probe |
| `crates/web/` | REST, websocket, embedded control page (no build step) |
| `crates/rookery/` | binary: config, wiring, northbound listener |
| `crates/diag/` | vendored from flock, unchanged |
| `docs/protocol.md` | both OSC surfaces and the HTTP state path |
| `docs/verification.md` | what has actually been run |
