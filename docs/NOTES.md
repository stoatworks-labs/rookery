# Notes

Working notes for this repo: status, decisions, and the traps that have actually bitten.
Migrated out of Claude Code's memory on 2026-08-24, so they are written in the first
person and dated by when each thing was learned — that date is usually the useful part.

Cross-cutting notes that are not specific to this repo live in
[fleet-notes](https://github.com/stoatworks-labs/fleet-notes).

*rookery — flock-shaped fleet control panel for WebLinked instances; OSC out for commands, HTTP in for state*

**rookery** — one web UI driving any number of [weblinked](https://github.com/stoatworks-labs/weblinked/blob/main/docs/NOTES.md) (`weblinked`) instances
at once, modelled on **flock**. `~/Projects/rookery`, Rust workspace,
**PUBLIC MIT**, `stoatworks-labs/rookery`. Created and released 2026-08-10.
**v0.1.1 shipped** — 16 assets from CI (6 targets, .deb/.rpm, macOS .pkg/.dmg,
Windows NSIS), signed + notarised. **Video `fMPwYG5TRAs`**, IG reel published. No Tauri launcher, deliberately: it is a headless server with an
embedded web UI, so the release workflow is flock's minus the launcher job.

**The defining design fact, stated everywhere in the repo:** commands go
**out over OSC** (fire-and-forget UDP, no ack, WebLinked replies with
nothing) and state comes **back over HTTP** (`/api/sources` polling). A
successful send means only "the datagram left this machine". So no method
collapses send and confirm, and `Fanout` always carries the per-instance
breakdown rather than one ok/err. An empty fan-out is a **404**, never a
success — a cue that silently matched nothing is the failure the project
exists to avoid.

**Crates:** `core` (Instance, Registry, Command, state — no sockets), `osc`
(own OSC 1.0 codec, no library vendored), `fleet` (fan-out, poller,
northbound grammar), `instance-live`, `instance-mock`, `discovery`, `web`,
`rookery` binary, plus `diag` vendored from flock unchanged. ~4.7k lines,
59 tests, clippy clean.

**Northbound grammar** — `/rookery/{all|group/<tag>|instance/<name-or-id>}/…`
with an optional `source/<id>/` before the verb. **The verbs are WebLinked's
own, unchanged**, so an existing Companion button is retargeted at a whole
group by editing only the front of the address. Scope lives in the address,
not an argument, so a button stays bound. An ambiguous instance *name* is
refused rather than guessed.

**Two traps recorded and pinned by tests:**
- **OSC string padding** — wire size is `(n + 4) & !3`; `padded()` takes the
  **text** length. Getting it wrong fails for **one input in four**, not
  always, which is how WebLinked shipped a decoder that silently dropped a
  quarter of all `/weblinked/url` messages. Both `crates/osc` and the
  over-the-wire test sweep all four residues — never delete those. Related:
  a **blob has no terminator**, so blob padding is `(n + 3) & !3`.
- **Cumulative counters are not health signals.** Found by running against
  the real thing: a functional headless WebLinked measured **346 dropped
  ticks out of 2205 and climbing** (~46% sustained) purely because macOS
  throttles a backgrounded process ([macos app nap](https://github.com/stoatworks-labs/fleet-notes/blob/main/notes/reference_macos_app_nap.md)). Health
  therefore comes from the **delta between polls** (`SourceState::dropping`,
  `skip_deserializing` so a response can't spoof it).

**Verified 2026-08-10 against three real WebLinked 0.7.1 processes** (loopback
only): state polling, url (incl. a 3-mod-4 length), mute, format, output,
script (proven to *execute* — the script navigated the page), group fan-out
hitting exactly its members, and the full northbound chain driven by an
**independent Python OSC encoder** written from the spec. `docs/verification.md`
is the authority. **Not verified:** anything off-machine, discovery on a real
LAN, Windows/Linux, scale past 3, a token-protected real instance,
multi-source against real WebLinked.

**Deliberate gap:** no auth on rookery's UI or the northbound port. WebLinked's
own OSC listener has none either, so a login on rookery alone would look like
protection it can't provide. Instance tokens *are* encrypted at rest. flock's
optional shared login is the obvious next addition.

**Gotcha that will bite an operator:** WebLinked binds HTTP to `127.0.0.1`
unless started with `--bind 0.0.0.0` — such an instance is fully controllable
over OSC and completely unpollable.

## Filming it (2026-08-10)

`stoatworks-backend/video/projects/rookery/` — `rig.py` brings up the whole fleet
(fixture HTTP server + 3 real WebLinked + rookery) and tears it down, so the take is
reproducible. Two things worth reusing:

- **Serve the filmed app's content over HTTP, never `file://`.** rookery renders each
  instance's loaded URL, so a file path would have put the home directory on camera —
  the failure that cost three videos a re-shoot. Fixture on `127.0.0.1:8902`.
- **The northbound OSC beat is fired from `capture.py`**, outside the browser, with an
  encoder written from the OSC spec rather than rookery's own. A take proving rookery
  can decode rookery proves nothing.

The choreography avoids group-wide *disruptive* verbs from the UI on purpose: rookery
raises a browser `confirm()` for those, and a Chrome dialog mid-shot is ugly. Group
changes go over OSC (no guard, not a person clicking); UI beats are non-disruptive
(mute/script) or single-instance.

**Re-shot once**: a caption claimed the lobby machine was untouched while the view was
filtered to the stage group, so nothing on screen supported it. Widen the view *before*
the claim.

## Live preview + interaction (2026-08-10, after v0.1.1, unreleased)

rookery shows a live picture of every instance and can click/type into the
focused one. WebLinked already had both halves (`/api/preview`, `/api/input`);
the work was making them affordable for a fleet.

**The WebLinked facts, all measured against 0.7.1 rather than read:**
- `/api/preview` is **raw BGRA, no compression**. An instance started without an
  explicit `--preview` runs at **factor 1** = 1920x1080 = **8,294,400 bytes a
  frame**. Factor is `spec.optionInt("factor", 4)` clamped 1..16, but the
  *implicit* preview is factor 1.
- **The factor is settable at runtime** via `POST /api/output/update` with
  `{"name":"preview","output":{"kind":"preview","name":"preview",
  "options":{"factor":8}}}`. That is what makes a fleet wall possible: 8.3 MB
  becomes 129,600 bytes.
- **`X-Frame-Sequence` is a PAINT id, not a tick counter.** It stays put on a
  static page (a scoreboard held 0 forever) and climbs at the paint rate on an
  animated one (~50/s at 1080p50). I briefly mistook the static case for a bug —
  it is not. It is the right thing to hang an ETag on.
- **`/api/input` `character` is the CHARACTER code, not the key code**:
  `key_code` 72 + `character` 104 types `h`; `character` 72 types `H`. Both
  fields needed on the keydown as well as the char event. A mismatched pair
  types nothing.
- Positions are normalised 0..1, so a thumbnail of any size can drive it.

**Measured bandwidth through rookery** (JPEG q55, ETag on the sequence):
3 animated instances at factor 8, ~4 fps = **0.19 Mbit/s**; 3 static instances =
**zero image bytes** (every request 304). The pre-measurement estimate was
33 Mbit/s for eight — **60x pessimistic**, because it sized the wall on the raw
frame rather than the JPEG. Measure before designing around a number.

**Design rules that came out of it:**
- Proxy, never let the browser fetch the instance directly — no CORS on
  WebLinked, and the token must stay server-side.
- `preview_factor` is **opt-in per instance**: the factor belongs to that
  instance's preview output and its own control page shows the same one.
- Reconcile the factor in the **poller** (survives an instance restart), and
  only when the current value is visible AND differs — the first version treated
  "cannot see it" as "does not match" and rewrote on every poll for ever.
- **Arm interaction, never leave it ambient.** On a dashboard the pointer is
  over a preview most of the time and a click reaches a page that is on air.
- A 500ms state push that rebuilds the DOM **destroys keyboard focus between
  keystrokes** — the expanded pane has to be preserved across renders, not
  rebuilt. Typing was impossible until that was fixed.
