# Protocol

Two OSC surfaces, pointing opposite ways, plus one HTTP client. Getting these
straight is most of understanding rookery.

```
   a desk, QLab,                                          WebLinked
   a Companion button                                     instances
        │                                                     ▲
        │  OSC in  (northbound)                     OSC out   │
        │  /rookery/…                          /weblinked/…   │
        ▼                                                     │
   ┌─────────────────────────────────────────────────────┐    │
   │                      rookery                        │────┘
   │                                                     │
   │   web UI  ◄── HTTP poll /api/sources ────────────────────┘
   └─────────────────────────────────────────────────────┘
```

## Southbound: rookery → WebLinked

Plain WebLinked OSC, unchanged. rookery sends to each instance's `osc_port`
(default 7655) using its `osc_prefix` (default `/weblinked`).

| Address | Args | Command |
|---|---|---|
| `/weblinked/url` | `s` | Navigate |
| `/weblinked/reload` | `i` | 1 bypasses the cache |
| `/weblinked/script` | `s` | Run JavaScript in the page |
| `/weblinked/mute` | `i` | Non-zero mutes |
| `/weblinked/format` | `s` | e.g. `1080p50` |
| `/weblinked/output/<name>` | `i` | 1 starts, 0 stops |

Each also exists under `/weblinked/source/<id>/…` to address one pipeline in
a multi-source instance. A bare address means the primary.

**Bools go out as `i`, not `T`/`F`.** Both reach WebLinked's `firstBool` — its
decoder pushes `T`/`F` into the same integer list — but `i` is what its
documented API and Companion examples use, and it renders legibly in a
generic OSC monitor when someone is debugging a show at 2am.

**Multiple verbs from one action go as a bundle.** Two separate datagrams can
arrive in either order, so a "navigate then reload" pair sent loose can have
the reload race the navigation it was meant to follow.

## Northbound: desk → rookery

```
/rookery/all/<verb>
/rookery/group/<tag>/<verb>
/rookery/instance/<name-or-id>/<verb>

…with an optional pipeline before the verb:
/rookery/group/<tag>/source/<source-id>/<verb>
```

The verbs are **WebLinked's own, unchanged**. That is the point: a button
already aimed at one WebLinked is retargeted at a whole group by editing the
front of the address and nothing else.

```
before:  /weblinked/source/lower-third/output/Graphic
after:   /rookery/group/stage/source/lower-third/output/Graphic
```

Three decisions worth not re-litigating:

- **The scope lives in the address, not in an argument.** Same reason
  WebLinked puts its source id there: a desk sends a fixed address per
  button, so one button binds to one group and stays bound. A scope passed as
  an argument would let the same button point anywhere depending on state
  nobody can see from the front panel.
- **An instance can be named rather than UUID'd**, because nobody wants a
  UUID in a QLab cue. An *ambiguous* name is refused rather than guessed —
  firing a graphic change at the wrong machine because two are called `gfx`
  is precisely the failure a fleet tool must not invent.
- **A malformed address sends nothing at all** and logs the reason. A cue
  that half-applies across a fleet is worse than one that visibly does
  nothing.

There is no reply. OSC has no acknowledgement here in either direction, so a
failed northbound cue surfaces only in rookery's log and its UI — the desk
will never hear about it.

## The state path: rookery → WebLinked over HTTP

`GET /api/sources` per instance, per poll. Falls back to `/api/state` on a
404, which is what a WebLinked older than multi-source answers.

**This is the only way rookery ever learns whether anything worked.** OSC is
fire-and-forget; the dashboard is the confirmation. Nothing in the UI presents
a successful send as a confirmed change.

Two things about reaching a real instance:

- **WebLinked binds HTTP to `127.0.0.1` by default.** An instance started
  without `--bind 0.0.0.0` is fully controllable over OSC and completely
  unpollable. This is the single most likely reason a newly added instance
  shows commands going out and no state coming back.
- **`--token` protects HTTP only.** WebLinked's OSC listener has no
  authentication of any kind. Anyone who can reach UDP 7655 can change what
  is on air regardless of what rookery holds.

## The OSC string padding rule

Stated once, here, because it has already cost this fleet a shipped bug.

An OSC string is NUL-terminated and *then* padded to a multiple of four, and
the terminator is not optional — a string whose length is already a multiple
of four still gets four NULs. So the wire size of an `n`-character string is
`(n + 4) & !3`.

WebLinked shipped a decoder that counted the terminator twice
(`padded(textLength + 1)`), which over-advances by four for any string whose
length is 3 mod 4. The read ran off the end and the *whole message* was
discarded silently. `/weblinked/url` worked for most addresses and did
nothing for a quarter of them.

The generalisable lesson: **a length-residue error in an OSC codec fails for
one input in four, not always.** That is how it survives casual testing. Both
`crates/osc` and the over-the-wire tests in `crates/instance-live` sweep all
four residues, and any change to the codec must keep doing so.

One related distinction the same code has to get right: **a blob is padded to
four bytes but has no terminator**, so a 4-byte blob occupies 4 bytes where a
4-character string occupies 8. Using the string rule for blobs over-advances
on every blob whose length is a multiple of four.
