# Verification

What has actually been run, against what, and what that does and does not
establish. This file is the authority — never upgrade "compiles" or "passes
against the mock" to "works".

The fleet rule applies here as everywhere: **the mock is not evidence.**
`rookery-instance-mock` binds real sockets and decodes real OSC, which makes
it far better than an in-process stub, but it is still rookery's own reading
of WebLinked's protocol talking to itself. Only the real-instance section
below tests the reading.

---

## 1. Automated tests — 56, all passing

```bash
cargo test
```

| Crate | Covers |
|---|---|
| `rookery-osc` | encode/decode round trips, the padding residue sweep, bundles, truncation |
| `rookery-core` | command↔OSC mapping, northbound verb parsing, registry and groups, token encryption, `/api/state` parsing, health rules |
| `rookery-fleet` | address grammar, dropped-tick delta tracking, fan-out over real sockets against the simulated fleet |
| `rookery-instance-live` | the whole wire path: encoder → UDP → simulated instance → HTTP → client |

The one to keep: **`every_string_length_residue_round_trips`** and its
over-the-wire sibling **`urls_of_every_length_residue_actually_arrive`**. An
OSC string-padding error does not fail always, it fails for one input in four
— which is exactly how WebLinked shipped a decoder that silently dropped a
quarter of all `/weblinked/url` messages. Both tests sweep all four residues.

## 2. Against a real WebLinked — 2026-08-10

Three real **WebLinked 0.7.1** processes on macOS (Apple silicon), built from
`~/Projects/weblinked`, each rendering a local page at 1080p50 with a preview
output, on ports 7654/7664/7674 (HTTP) and 7655/7665/7675 (OSC). rookery
polling at 500 ms.

### Confirmed working

| What | Evidence |
|---|---|
| **State polling** | `/api/sources` parsed from a real instance; format, loaded URL, outputs and pacing counters all read back correctly |
| **`url`** | Sent `https://example.com/abc` — **23 characters, 3 mod 4**, the exact residue WebLinked's old decoder dropped. `loaded_url` changed. |
| **`mute`** | `audio_muted` went `true`, then back to `false` via the northbound path |
| **`format`** | `1080p50` → `720p25`; the instance reported `1280x720p25` |
| **`output`** | `preview` toggled off (`enabled:false, running:false`) and back on |
| **`script`** | Proven to *execute*, not merely arrive: the script was `location.href='https://example.com/proved-by-script'` and the instance's own `loaded_url` changed to match |
| **Group fan-out** | A `url` cue to the `stage` tag reached gfx-1 and gfx-2 and left gfx-3 (`lobby`) on its previous page |
| **Northbound OSC** | Driven from `osc_send.py`, an **independent** OSC encoder written from the 1.0 spec rather than from rookery's own — so a pass is two implementations agreeing, not one agreeing with itself. All three scopes (`/rookery/all/…`, `/rookery/group/stage/…`, `/rookery/instance/gfx-1/…`) reached the real instances. |

### What this run also established about health

A perfectly functional headless instance reported **`dropped_ticks: 346`
against 2205 ticks**, climbing continuously — measured at ~120 dropped per
254 ticks over 5 s, so roughly 46% of ticks, sustained.

That is real, not a rookery artefact: a backgrounded macOS process with no
window and no hardware output gets throttled, and WebLinked's clock falls
behind. But it means **the cumulative counter is not a health signal** —
every long-running instance would sit permanently amber. rookery therefore
tracks the *delta between polls* (`SourceState::dropping`) and colours a
source by whether it is dropping **now**. Confirmed live: with three real
instances up, one read steady/green and one read dropping/amber at the same
moment, and the amber one recovered to green when it caught up.

### Not verified

- **Any instance not on this machine.** Everything above was loopback.
  Nothing here exercises a real show network, a switch, VLANs, or UDP
  behaviour under load.
- **Discovery.** `crates/discovery` compiles and its subnet-enumeration rules
  are adapted from flock's proven ones, but the probe has never found a real
  WebLinked on a real LAN — only loopback instances, which it does not sweep.
- **Windows and Linux.** Never built or run. Nothing here is
  platform-specific, but that is an argument, not a test.
- **Scale.** Three instances. Nothing establishes behaviour at forty.
- **A token-protected real instance.** The token path is covered against the
  mock only; no real WebLinked was started with `--token`.
- **Multi-source instances.** The `source/<id>` path is covered against the
  mock only. Every real instance in this run was a plain command-line launch
  with a single primary pipeline.

## 3. Reproducing the real-instance run

```bash
# One WebLinked, no hardware needed — the preview output is enough.
~/Projects/weblinked/build/Release/WebLinked.app/Contents/MacOS/WebLinked \
  --url https://example.com --format 1080p50 --port 7654 --osc-port 7655 --headless &

# Point rookery at it.
cat > config/rookery.toml <<'EOF'
bind = "127.0.0.1:8090"
registry_path = "data/registry.json"
poll_interval_ms = 500
osc_bind = "127.0.0.1:7656"
EOF
cargo run --release

# Add it, then drive it.
curl -X POST -H 'Content-Type: application/json' \
  -d '{"name":"gfx-1","host":"127.0.0.1","tags":["stage"]}' \
  http://127.0.0.1:8090/api/instances

curl -X POST -H 'Content-Type: application/json' \
  -d '{"verb":"url","url":"https://example.com/abc"}' \
  http://127.0.0.1:8090/api/groups/stage/send

# …and confirm at the far end, not in rookery's own report.
curl -s http://127.0.0.1:7654/api/state | jq .source.loaded_url
```

That last step is the whole discipline: rookery saying "sent" only means a
datagram left the machine. Check the instance.
