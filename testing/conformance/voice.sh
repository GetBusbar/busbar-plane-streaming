#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Existing suite IDs, backed by ordinary Rust assertions over the actual linked/dropped doors.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$here" || exit 2
dialect="${1:?usage: voice.sh openai|gemini|twilio}"
case "$dialect" in openai|gemini|twilio) ;; *) echo "unknown dialect: $dialect" >&2; exit 2 ;; esac
: "${CONFORMANCE_SUITE:?}" "${CONFORMANCE_OUT:?}" "${BUSBAR_CHECKOUT:?}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/target}"
case "$CARGO_TARGET_DIR" in /*) ;; *) export CARGO_TARGET_DIR="$here/$CARGO_TARGET_DIR" ;; esac
mkdir -p "$CONFORMANCE_OUT"
status=0
# Build the actual image from this source and this normal locked graph before loading it.
cargo +1.98.0 build --locked -p busbar-plane-streaming-plugin --lib >"$CONFORMANCE_OUT/build.log" 2>&1 || status=$?
case "$(uname -s)" in
  Darwin) export BUSBAR_PLUGIN_CDYLIB="$CARGO_TARGET_DIR/debug/libbusbar_plane_streaming_plugin.dylib" ;;
  *) export BUSBAR_PLUGIN_CDYLIB="$CARGO_TARGET_DIR/debug/libbusbar_plane_streaming_plugin.so" ;;
esac
if [ "$status" = 0 ]; then
  if [ "$dialect" = twilio ]; then
    cargo +1.98.0 test --locked -p busbar-plane-streaming-plugin --test voice_twilio -- --nocapture >"$CONFORMANCE_OUT/ordinary.log" 2>&1 || status=$?
    if [ "$status" = 0 ]; then
      cargo +1.98.0 test --locked -p busbar-plane-streaming --lib voice_twilio_fixtures -- --nocapture >"$CONFORMANCE_OUT/codec.log" 2>&1 || status=$?
    fi
  else
    cargo +1.98.0 test --locked -p busbar-plane-streaming-plugin --test voice_codecs "$dialect" -- --nocapture >"$CONFORMANCE_OUT/ordinary.log" 2>&1 || status=$?
    if [ "$status" = 0 ]; then
      cargo +1.98.0 test --locked -p busbar-plane-streaming-plugin --test voice_composition -- --nocapture >"$CONFORMANCE_OUT/composition.log" 2>&1 || status=$?
    fi
    # These print observations. Their matched values never affect the suite verdict.
    if [ "$status" = 0 ]; then
      cargo +1.98.0 test --locked -p busbar-plane-streaming-plugin --test voice_observations -- --nocapture >"$CONFORMANCE_OUT/observations.log" 2>&1 || status=$?
    fi
  fi
fi
python3 -I - "$dialect" "$CONFORMANCE_SUITE" "$CONFORMANCE_OUT/verdict.json" "$status" <<'PY'
import json,sys
name,suite,path,code=sys.argv[1:]
code=int(code)
with open(path,'w') as output:
    json.dump({'suite':suite,'status':'pass' if code==0 else 'fail','armed':True,
               'standard':{'openai':'OpenAI Realtime API','gemini':'Google Gemini Live API','twilio':'Twilio Media Streams'}[name],
               'plan':'ordinary fixture, transcript and linked/dropped ABI assertions',
               'reason':'ordinary Rust assertions passed' if code==0 else f'actual build or ordinary Rust assertions failed (exit {code}); see suite logs'},output,indent=2)
    output.write('\n')
PY
exit "$status"
