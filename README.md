<!-- fleet:header:begin (rendered by `busbar-release plugin heal` from GetBusbar/busbar-release template/ and busbar's plugins.yaml; edit it there) -->
# busbar-plane-streaming

First-party signed kind:plane plugin cdylib: the streaming plane, packaged as a droppable busbar plugin. Drop the signed tarball into plugins/.

| kind | alias | crate | busbar | license |
|---|---|---|---|---|
| `plane` | `streaming` | `busbar-plane-streaming-plugin` | 1.6.0 (pinned in `.busbar-ref`) | Apache-2.0 |

<!-- fleet:header:end -->

## What it is for

`busbar-plane-streaming` is busbar's streaming plane: live, two-way voice and realtime sessions,
each turn governed and metered as busbar meters a request. It speaks three caller dialects:

- `openai-realtime`: OpenAI Realtime over a WebSocket (`GET /v1/realtime`), the browser's ephemeral
  secret (`POST /v1/realtime/client_secrets`, minted by busbar itself), the SDP offer of a WebRTC
  call (`POST /v1/realtime/calls`, terminated by busbar) and its sideband socket
  (`/v1/realtime/sideband/{call_id}`);
- `gemini-live`: Gemini Live over a WebSocket (`/v1/realtime/gemini/{call_id}`);
- `twilio-media-streams`: Twilio Media Streams telephony (`/twilio/{call_id}`), bridged to OpenAI
  Realtime.

It dials the catalog model the session names. OpenAI Realtime and Twilio doors require an
OpenAI Realtime provider; the Gemini door requires Gemini Live. When the model's provider protocol
is present in the handed settings, an incompatible door is refused before dialing.

## Config

Off unless the deployment writes a `streams:` section. The deployment also needs an absolute
`public_url` for the plane to publish its doors; an absent or invalid URL is logged. The plane declares `streams:` and reads the
`providers:` and `models:` sections beside it (the session model's provider and wire model); it
owns no other section.

- `session`: the locked session every live session opens with (the plane's flat session configuration:
  model, modalities, instructions, voice, audio formats, `turn_detection`, tools, `tool_choice`).
  The plane translates these keys into each provider's wire format; OpenAI GA audio settings
  become nested `audio.input`/`audio.output` objects. Default: server VAD, 500 ms of silence,
  including a written session that omits `turn_detection`; explicit `null` disables VAD. A key the plane does not model is refused, naming its path.
  Gemini Live has no tool-choice policy field: omit `tool_choice` for a Gemini model. An explicit
  policy is refused instead of being silently ignored; OpenAI Realtime supports this setting.
- `session_max_secs`: the wall-clock ceiling of one session, in seconds. Default: none. Zero is
  refused.
- `context_window_tokens`: the context-window ceiling. Default 32768.
- `max_output_tokens`: the output ceiling per response. Default 4096. Must be below
  `context_window_tokens`.
- `media.listen`: the `ip:port` the browser's WebRTC media arrives on. Default: none, and the SDP
  door refuses every offer.

Caller updates cannot replace locked instructions, tools, voice, modality, tool choice, or
ceilings. Input transcription is refused by removing its configuration because the plane does not
meter the separate transcription model. Unsupported or malformed uplink audio is dropped with a
bounded diagnostic. OpenAI response limits are clamped to its supported range, 1–4096 tokens.

A session retains the configuration used for its admission across refreshes. A sideband inherits
its brokered call's lock and does not bill mirrored provider usage again. Sideband projection exposes
no new session parameters and cannot rewrite the existing call; ownership is checked when attaching.
Audio injected directly
on the sideband still contributes ingress seconds. Capacity limits refuse new calls without
evicting live calls.

## Diagnostics

`declares.json` states the plane's needs (`http`, `webrtc`, `ws`) and no diagnostic code of its own:
the plane raises only core-owned codes.

## Build

```bash
cargo build --release -p busbar-plane-streaming-plugin
```

## Tests

```bash
cargo test --workspace --locked
```

## License

Apache-2.0. See [LICENSE](LICENSE).
