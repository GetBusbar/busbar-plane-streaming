# Streaming ordinary-test migration

Baseline for this continuation: `55c2652` over accepted product `9d3a602`.
The contract/loader pin stays `a35493a5e74cc935ff7dcc15c1af869290da91cb`;
all peer pins, SDK source, locks, publication and rendered CI policy stay unchanged.
The A fixture-package-diet discussion plan was read. No applicable AGENTS.md was
found in this checkout or its ancestors.

## Actual product and existing assertions

The two active workspace members remain the actual logic crate and its actual
cdylib packaging crate. No fixture implementation, shared/local replacement kit,
Subject/Recorder/Fold, step interpreter or conformance macro remains in their
sources or manifests. `conformance.rs` remains an ordinary Rust integration target.
`both_ways.rs` keeps all **43** existing named tests and all their assertions;
`support/admission_ticket.rs` keeps all **3** existing named tests and assertions.
The preceding three direct warning tests and all logic/codec/allocation tests are
untouched by this continuation.

TicketHost now directly implements the actual contract's HostServices. Its
entitlement grants, ticket mint bytes/expiry, captured record, one-time unit-50
admission, deterministic random bytes and counters are unchanged. AdmissionHost
likewise directly implements HostServices: Key/Refused/Failed/Broken outcomes,
once-only take, random bytes and all take/draw counters remain. Required unrelated
services are explicitly unsupported on these concrete domain hosts; no blanket
adapter, generic defaults trait, wrapping Double or fake plane is introduced.
Optional contract methods retain the contract's unsupported behavior.

## Former macro product-script mapping

`both_ways::pinned_cases::published_streaming_cases_keep_exact_crossings_and_both_actual_deliveries`
adds the **33 executed steps** of this product's former pinned plane script. It
uses this repository's existing concrete frame/buffer helpers, with explicit exact
first-invocation counter assertions (`crossings - resumes`) around every call.
There is no new generic recorder or kind routing. Both actual delivery paths run
the same concrete cases and compare their complete recorded answers.

| Former script assertion | Ordinary assertion |
| --- | --- |
| facts / zero calls | Kind/name/max-inflight transcript, exact zero delta |
| validate bad #0/#1 | Dealt unknown-key/zero-ceiling inputs refuse with nonempty text; exactly one crossing each |
| validate | Dealt m-cap settings READY, exactly one crossing |
| arrive unopened | REFUSED before any crossing |
| open | READY, exact full generation-1 snapshot and claims/public URL, one crossing |
| ready | Successful actual ready result, optional-op exact 0/1 count |
| open again | REFUSED, zero crossings |
| hydrate/start | Actual generation-1 lifecycle, READY, one each |
| arrive claimed | POST calls, unit 7, with the original canary authorization field, READY, one crossing |
| arrive unclaimed | Claim 9/unit 8 exact REFUSED status 404, one crossing |
| live arrive | Plain m-cap realtime stream 20 READY, one crossing |
| live initial provider lock / caller audio | Same empty initial lock and 2 seconds PCM16 input; READY, exact far-output flag and GET target, one each |
| live turn attempt / tool call / usage | Same attempt, function-call c1/lookup, and audio/text/image usage inputs; READY and one crossing each |
| live caller end | READY and actual EMIT_DONE flag, one crossing |
| Gemini audio end / usage / caller end | Same exact inputs; READY/one each, exact usage `2:1=10 3:1=5 7:1=20 8:1=1`, actual done flag |
| every declared billable class emitted | Actual unit class indices cover the entire declared class list exactly across both sessions |
| refusal | Actual gate/403/denied with bounded output buffers, READY; body/status/headers compared both ways |
| serve/project | Actual product slots, one crossing each; non-FAULT results and output compared both ways |
| drive | Wake the actual registered driver, require READY and exactly one crossing; ready session count compared |
| release none | Exact READY or REFUSED `release: no such lease` branch, one crossing |
| tick | Actual driver ticket tick at 1000ns, READY/next-tick transcript, one crossing |
| cancel | Actual cancel, READY and one crossing; detailed disposition negatives remain in existing both_ways tests |
| refresh/retire | Actual next settings/generation-2 full snapshot and claims, then generation-1 retirement, READY/one each |
| close / arrive after close | Actual CLOSE READY/one; later arrival FAULT/zero |
| configured credential never returned | Original header canary is planted; arrival errors/routes, all recorded outputs including audits, and actual Statement bytes reject its appearance |

The old `request_unit: false` disabled the generic request/narrow/short/empty
branches; those were not missing executions. Existing ordinary request mint,
narrow-buffer, projection recall, SDP admission and native-initialization tests
continue to exercise those product behaviors independently. Former `serve_first`
and far-end fixtures were absent from these inputs, so their generic branches
also never executed here. The former input JSON remains as historical fixture
data; ordinary tests own their explicit concrete inputs.

## Admission and host negatives

`conformance::door_refusals` retains ABI +/-1 exact pack/linked/dropped-manifest
errors, kind-layout xor1/zero exact pack+linked errors/rebuild text, plane-answer
layout host-1 exact pack+linked error/numbers/rebuild text, all three valid foreign
Statement mismatch branches, all six other kinds through both deliveries, absent
connection table through both deliveries with exact plugin/need count, and Probe
success. Only metadata of the actual static operation table is mutated.

`conformance::host_refusals` retains malicious failing READY on the actual opened
plane, exact failure text and one READY invocation/first crossing. The actual
per-product missing-table/Probe arms remain in door_refusals.

The formerly synthetic undeclared-need/cross-instance arms transfer to the actual
GitHub plugin's declared connection needs under the coordinator-approved packet:
`A/diet-loader-d306/transfer-packets/generic-connection-negative-transfer-ack.json`,
SHA256 `ef00f652e3a7fa0f3f762e59f98e958fc586b225b3b7d1c1f351beaa75773573`. Reviewer `/root/doc_macro_bindings` records
SOURCE_EQUIVALENCE_ACK_FOR_THREE_ARMS_NATIVE_PENDING for actual GitHub candidate
`abd7663be59b670c8cceabaf30475334e771c566`, tree
`86a49682b4f52fbe99edd6247e73b207e4398285`. Its ordinary custody test preserves
actual declared-need open and close, next/max undeclared exact refusal, owner
write, foreign READ/WRITE/CLOSE exact NotOwner, subsequent owner write/close, both
delivery paths. Its actual missing-table test also preserves exact named
NoConnectionTable and Probe success; this product retains its own actual
missing-table check additionally. No metadata-grafted network need, fake socket
adapter, added connector peer dependency or synthetic carrier test remains here.
The source ACK is not native or release-coverage closure; actual GitHub native
qualification remains outstanding.

Actual linked/dropped Statements match byte-for-byte, with matching facts.
Declared network schemes equal the actual Statement. BUSBAR_EXPECT_RELEASE still
refuses a debug build.

## Retired framework-only checks

- Perturbed/doubled Recorder/Fold count-comparator selfchecks: that comparator is
  deleted; exact ordinary counter assertions remain on actual product calls.
- Artificial added-but-unreported class comparator selfcheck: no comparator API
  remains; real emitted-class coverage stays mandatory.
- Export/auth outbound byte-off/no-field/double-fetch injection branches and
  Store newer-schema/transport FramePiece arms: no-op for this plane.
- Generic arbitrary input-shape/interpreter checks are not reproduced as a new
  local framework. Product-specific input/failure assertions listed above remain.

## Artifact and qualification boundaries

BUSBAR_PLUGIN_CDYLIB remains mandatory and absolute. The exact file must exist;
there is no stale search, linked replacement, or absent-artifact skip. The build
lane proves freshness by building the immutable source first and recording the
library hash. The filesystem timestamp comparison is retired as a tooling
heuristic: archive extraction times do not establish source or artifact identity.
The actual loader admits/dlopens that file against the actual linked Statement.
The preceding CI policy still requires coordinated export of this path and removal
of the retired BUSBAR_CONFORMANCE_RED comparator controls.

`cargo fmt --all -- --check` and `git diff --check` pass. **No build, test, clippy,
dependency resolution, allocation measurement or actual cdylib run was performed**;
no build resource lane was approved for this continuation. The added source still
requires pinned/final-SDK native compilation and actual both-door execution. No
passing runtime or full structural ACK is claimed. Cargo.lock is deliberately
untouched for the coordinated resolver step.

### Separate-runner migration closure

The excluded voice runner was subsequently transferred to ordinary Rust cases and
retired. `VOICE-ORDINARY-MIGRATION.json` records the exact original hashes,
fixture hashes, case/predicate crosswalk and bounded native evidence scope.
The fixtures remain unchanged; observations retain their non-gating status.


## Streaming credential envelope restoration

The original a35493a5e7 canary assertion also inspected `take_heard()` diagnostic
names/text and metric labels. The ordinary pinned 33-case test now binds its
actual linked and freshly built dropped image to the existing concrete `HostLog`
sink rather than `NoSink`. It scans owned complete envelope bytes, both actual
Statements, the existing answer/audit transcript, and arrival errors/routes for
the configured credential. Dropped envelope records fail the privacy assertion.
The original seven spellings remain: raw, percent upper/lower, standard base64
without padding, URL-safe base64 without padding, and lower/upper hex. The
encoding is local to this one credential assertion; no new Cargo dependency or
runner/checker framework is introduced.

The existing `an_unreadable_usage_bills_the_floor_and_its_line_reaches_the_host_log`
case retains its exact billing and one-warning assertions and additionally proves
that the complete-byte capture receives the known diagnostic through each actual
image, with no dropped envelopes. The 33-case lifecycle/session script and every
existing crossing assertion remain unchanged. This successor is source-reviewed
and formatted only; its native qualification is separate from the immutable
preceding ecdf9ca6 run. No production source, manifest, lock, or excluded voice
harness changed.


The same source review also restored fields previously omitted from ordinary
linked/dropped comparison: every concrete Called error/lease, all arrival
refusal/status and Ready class/principal/dialect/units/route fields, session
need/lane, and cancel disposition. The top-level PROJECT now uses the original
claimed target and empty request body, unit zero, 256-byte arena, four signals,
no message buffers, and compares `signals_len`. SERVE retains its original
status/reply-written contract plus Called fields (the original did not compare
its body/fields).

The nine existing session pieces now use public `Dispatcher::submit_lent` with
one actual minted ticket per session reused for its pieces; SERVE uses its own
actual request ticket. Session buffers use the original eight unit/field/record
slots and 1024-byte arena, with the fixture's reply sizes, member `m-cap`, and
caller reference. A streaming-specific owned value keeps input bytes and output
buffers alive through completion/cancellation; UnsafeCell and its two Send/Sync
implementations permit only crossing writes while submitted and host output
access after Done returns the joined frame. Reclamation does not assume when a
worker releases its last owner clone. The existing Bufs defaults remain the same
for all prior both-ways tests; Vec storage permits only these pinned buffers to
use the original capacities. Original per-step first-crossing pins stay in place;
actual resume deltas are reported separately without a scheduling-dependent pin.


A subsequent source-only scope correction moves the complete Called-error and
raw reply/arena canary checks from the shared Bufs renderer into the pinned
submitted-piece completion, after joined Done and before rendering. Thus every
pinned credential sink remains scanned, while unrelated original direct-call
capacity tests do not repeatedly scan whole allocated buffers. Their loops,
limits, thresholds, assertions, and product behavior are unchanged. The prior
22f7294 native receipt remains evidence for that immutable source only; this
normal successor requires its own qualification.


The final independent review restored the absolute RELEASE-none predicate as
well as parity: lease must be zero; READY error must be absent/empty; REFUSED
must carry exactly `release: no such lease`. These are the two exact original
`release_none_contract` alternatives, not a linked/dropped agreement substitute.
