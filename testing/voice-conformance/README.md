# Ordinary voice conformance tests

Protocol fixtures and captured transcripts live in `fixtures/` and retain their original bytes.
The former standalone `voice_conform` runner has been retired after assertion transfer.

The ordinary Rust targets are:

- `plane-streaming-plugin/tests/voice_codecs.rs`: 36 fixture cases and two captured transcripts.
- `plane-streaming-plugin/tests/voice_twilio.rs`: 14 actual-door cases.
- `plane-streaming/src/tests/voice_twilio_fixtures.rs`: 11 owning codec and claim cases.
- `plane-streaming-plugin/tests/voice_composition.rs`: ten composition families.
- `plane-streaming-plugin/tests/voice_observations.rs`: five non-gating observations, printed with `--nocapture`.

The ABI cases load both the linked door and the actual fresh dropped cdylib. Build
`busbar-plane-streaming-plugin --lib` and set `BUSBAR_PLUGIN_CDYLIB` to its absolute path.
`testing/conformance/voice.sh openai|gemini|twilio` runs these ordinary targets and
propagates Cargo failures into the existing suite verdict contract. It introduces no
runner, scripted subject, expected-answer interpreter, or synthetic RED receipt.

`VOICE-ORDINARY-MIGRATION.json` records the original source and fixture hashes,
case crosswalk, retained predicates, and original planted-mutation discrimination.
Historical source remains available at its recorded Git commit. The five vision
observations remain observations; their matched values do not grade conformance.
