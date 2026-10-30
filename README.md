<!-- fleet:header:begin (rendered by `busbar-release plugin heal` from GetBusbar/busbar-release template/ and busbar's plugins.yaml; edit it there) -->
# busbar-plane-streaming

First-party signed kind:plane plugin cdylib: the streaming plane, packaged as a droppable busbar plugin. Drop the signed tarball into plugins/.

| kind | alias | crate | busbar | license |
|---|---|---|---|---|
| `plane` | `streaming` | `busbar-plane-streaming-plugin` | 1.6.0 (pinned in `.busbar-ref`) | Apache-2.0 |

[![ci](https://github.com/GetBusbar/busbar-plane-streaming/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/GetBusbar/busbar-plane-streaming/actions/workflows/ci.yml)
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

It dials the provider (`openai` as OpenAI Realtime, `gemini` as Gemini Live) of the catalog model
the session names.

## Config

Off unless the deployment writes a `streams:` section. The plane declares `streams:` and reads the
`providers:` and `models:` sections beside it (the session model's provider and wire model); it
owns no other section.

- `session`: the locked session every live session opens with (the GA realtime `session` object:
  model, modalities, instructions, voice, audio formats, `turn_detection`, tools, `tool_choice`).
  Default: server VAD, 500 ms of silence. A key the plane does not model is refused, naming its path.
- `session_max_secs`: the wall-clock ceiling of one session, in seconds. Default: none. Zero is
  refused.
- `context_window_tokens`: the context-window ceiling. Default 32768.
- `max_output_tokens`: the output ceiling per response. Default 4096. Must be below
  `context_window_tokens`.
- `media.listen`: the `ip:port` the browser's WebRTC media arrives on. Default: none, and the SDP
  door refuses every offer.

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
