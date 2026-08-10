# AGENTS.md — bringing an LLM up to speed on rookery

Orientation for an AI assistant (or a new human) picking this up cold.
`CLAUDE.md` holds the short command reference; this explains the model and
the traps.

---

## 1. What this is

A fleet control panel for **[WebLinked](https://github.com/stoatworks-labs/weblinked)**
instances — one web UI that drives any number of them, in groups, over OSC.
Modelled on [flock](https://github.com/stoatworks-labs/flock), which does the
same job for BirdDog Play decoders.

Rust service: a registry, a fan-out engine, two OSC surfaces and a web UI.

## 2. The one idea everything else follows from

**Commands go out over OSC. State comes back over HTTP. They are different
protocols with different failure modes, and rookery never pretends otherwise.**

- OSC is fire-and-forget UDP. A successful send means *the datagram left this
  machine*. There is no acknowledgement, no reply, and WebLinked sends
  nothing back.
- The HTTP poll of `/api/sources` is the only way rookery ever learns whether
  a command landed.

So: no method anywhere collapses "send" and "confirm" into one call, no UI
element reports a send as a confirmed change, and `Fanout` always carries the
per-instance breakdown rather than a single ok/err. If you find yourself
adding an `apply_and_confirm`, stop — the network cannot support it.

## 3. Layout

```
crates/
  core/            Instance, Registry, Command, state types — no sockets
    instance.rs      metadata: host, two ports, prefix, tags, token, poll flag
    registry.rs      JSON-persisted; groups are DERIVED from tags, never stored
    command.rs       the verb set, and both directions of the OSC mapping
    state.rs         WebLinked's /api/state, and the health rules
    crypto.rs        token encryption at rest (adapted from flock)
  osc/             OSC 1.0 codec + UDP sender and receiver
  fleet/           fan-out, the poller, the northbound address grammar
  instance-live/   the real transport: OSC out, HTTP state in
  instance-mock/   a simulated WebLinked on REAL sockets — see §5
  discovery/       active subnet probe (WebLinked does not advertise itself)
  web/             axum REST + websocket + embedded control page
  rookery/         the binary: config, wiring, northbound listener
  diag/            vendored from flock, unchanged
```

## 4. The architectural rules

1. **`core` has no sockets.** Transport lives behind `InstanceClient`.
2. **Groups are derived from tags at send time.** There is no group entity to
   keep in sync, which is what gives "one instance, N groups" for free and
   makes retagging take effect on the very next cue.
3. **An empty fan-out is never success.** A cue that matched nothing is one
   of the most dangerous things a show-control system can do quietly. `web`
   answers 404, the northbound dispatcher logs a warning, and `Fanout`
   reports it as not fully sent.
4. **`Command` is exactly WebLinked's OSC verb set, and no more.** WebLinked's
   HTTP API is much wider, but a `Command` variant that quietly fell back to
   HTTP would hide a real operational difference between a datagram and a
   blocking call that can fail with a reason.

## 5. `instance-mock` is not a toy — but it is not evidence either

It binds a real UDP socket and decodes real OSC, and serves real HTTP. So a
test using it exercises the encoder, the socket, the address grammar and the
JSON shape — nothing is bypassed. That matters because the bugs this project
is most exposed to are wire-format bugs, and an in-process mock that takes a
`Command` and returns a `SourceState` would catch none of them. WebLinked's
own padding bug would have sailed through such a mock.

**But passing against the mock only shows that rookery agrees with rookery's
reading of WebLinked's protocol.** Only running against the real binary tests
the reading. `docs/verification.md` is the authority on what has actually
been run; keep it honest and keep it updated with commands and output, not
ticks.

## 6. Two traps this project will keep producing

### The OSC string padding residue

An OSC string is NUL-terminated *then* padded to a multiple of four; the
terminator is mandatory even when the length is already a multiple of four.
So the wire size is `(n + 4) & !3`, and `padded()` must be passed the **text**
length, never the length with the NUL already added.

Getting this wrong fails for **one input in four**, not always — which is how
WebLinked shipped a decoder that silently dropped a quarter of all
`/weblinked/url` messages. `crates/osc` sweeps all four residues, and so does
the over-the-wire test in `crates/instance-live`. Never delete those.

Related: **a blob has no terminator**, so blob padding is `(n + 3) & !3`, not
the string rule.

### Cumulative counters are not health signals

`pacing.dropped_ticks` is cumulative since the instance started. A real,
perfectly functional headless WebLinked measured 346 dropped ticks out of
2205 and climbing — macOS throttles a backgrounded process and the clock
falls behind. Colouring on the cumulative count makes every mature instance
permanently amber, and an indicator that is always amber says nothing.

rookery tracks the **delta between polls** (`SourceState::dropping`, filled in
by `Fleet`, `skip_deserializing` so a response cannot spoof it). The question
worth answering mid-show is "is it dropping *now*". Any new counter-derived
indicator should follow the same rule.

## 7. Commands

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features
cargo run -p rookery -- config/rookery.toml
```

## 8. A caution specific to this project

rookery changes **what is on air, on several machines at once**. A bad
group cue does not fail a test — it blacks out every graphic in a venue
simultaneously. Treat the fan-out paths with more care than the read paths:
the UI names the machines in its confirmation prompt for disruptive commands
rather than showing a count, and that is deliberate.

Instance tokens live in `crates/core/src/crypto.rs`. Don't log them, and note
that `Instance::redacted()` is what the API layer returns — the frontend
echoes `********` back on an unchanged token, and `InstanceBody::apply_to`
must keep treating that as "leave it alone" or a save would lock rookery out
of the instance.

## 9. Conventions

- MIT, public, and carries the fleet's AI-assisted disclaimer in the README.
- "Commit" means commit **and** push.
- Diagnostics via the vendored `crates/diag`: wire it as the **first** thing
  in `main` and **hold the returned guard** — dropping it silently stops the
  log file being written.
