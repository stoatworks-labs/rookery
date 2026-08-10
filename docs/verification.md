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

## 1. Automated tests — 76, all passing

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

---

## 4. Live preview and interaction — 2026-08-10

Added after v0.1.1 and exercised against the same three-instance rig. Every
figure below was measured, not estimated.

### The picture

| Check | Result |
|---|---|
| A frame arrives whole | 1920x1080, 8,294,400 bytes of BGRA, buffer matches the headers |
| Colours survive the BGRA→JPEG swap | a `#0a1020` navy scoreboard rendered navy, not brown |
| JPEG on the browser leg | 37,662 bytes at factor 1; **220x** smaller than the raw frame |
| Preview factor set at runtime | instance reported `factor: 8` after the request; frame became 240x135 / 1,508 bytes |
| Reconciliation is idempotent | one `preview factor set` log line, then none across ten further polls |
| `--no-preview` | 404 from the instance → 204 with `X-Preview-Unavailable: not-configured` |

### Bandwidth, which is the whole reason for the design

Three instances rendering an **animated** page, all at factor 8, polled at
~4 fps through rookery: **0.19 Mbit/s** total, 2,231 bytes per frame.
Extrapolated to eight instances, about **0.5 Mbit/s**.

The same three showing **static** graphics: **zero image bytes**. Every one of
24 consecutive requests answered `304`, because WebLinked's `X-Frame-Sequence`
does not advance without a repaint and rookery hangs its ETag on it.

For scale, the naive version of this feature — polling `/api/preview` at
factor 1 without conditional requests — would be **95 MB/s** for the same three
instances.

Worth recording that the design estimate before measuring was 33 Mbit/s for
eight instances, about **60x pessimistic**: it sized the wall on the raw frame
rather than on the JPEG that actually crosses the wire.

### Interaction

Driven end to end through rookery's proxy, against a real page, with the result
confirmed **in the preview** rather than in a return code:

- A click at normalised (0.498, 0.248) turned a green button red — and the
  coordinates came from measuring the preview itself, which is the point: the
  picture is what you aim with.
- Typing `hi` into a real `<input>` after clicking it: read back out of
  `document.getElementById('f').value` as `hi`.
- **Un-armed clicks are blocked.** The same click dispatched at the pane before
  arming left the button green; after arming, it landed. That is the guard
  doing its job, tested rather than assumed.

`character` is the **character** code, not the key code: `key_code` 72 with
`character` 104 types `h`; `character` 72 types `H`. Both fields are needed on
the keydown as well as the char event.

### What is still unverified

- All of it on one machine over loopback, like everything else here.
- No instance has ever been asked for a preview **while it was also feeding
  SDI or NDI under load**; the cost of the preview output to a busy instance is
  unmeasured.
- The `wheel` event is implemented and typed but has not been driven against a
  scrolling page.
- Only one pipeline per instance has been previewed. `?source=` is passed
  through and tested against the mock, but a real multi-source instance has not
  been previewed per-pipeline.
