# gray-context-pin

Pin files or notes into the prompt on **every turn** so context compaction
can't drop them. `/pin` records a pin in the per-session store; the
`prompt/context` hook re-injects all pins under a `## Pinned context` block
each turn.

`/pin <arg>` becomes a **file pin** when `arg` resolves to an existing file
(absolute, `~/…`, or relative to the session cwd), otherwise a **note pin**
holding the literal text. File pins re-read the file at injection time, so
the pin always shows current contents (capped at 24 KB).

## Commands

- `/pin <path-or-note>` — record a pin
- `/pins` — numbered list of this session's pins
- `/unpin <n|all>` — remove pin `n` (see `/pins`) or all of them

## Wire

`plugin/manifest`, `prompt/context`, `command/run`, `plugin/shutdown`.
Protocol 1.1. Pins persist per session at
`~/.gray/context-pin/<session-key>.json` — they survive sidecar
restarts, which is the point: pins outlive both restarts and compaction.

## Install

```sh
cargo build --release
gray plugin install ./target/release/gray-context-pin
```

## Limits

File pins are read fresh each turn — a pinned file's *content* isn't frozen,
only its presence is. Pin text is injected verbatim into the model's
context; pinning large files burns tokens (24 KB cap per file). Sessions are
keyed by `session.id`, falling back to `session.cwd`.
