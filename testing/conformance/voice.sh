#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# The voice suites of the conformance check: `voice.sh openai` (voice-openai-realtime) and
# `voice.sh gemini` (voice-gemini-live). Tier spec-conformant: no vendor kit exists, so the suite is
# this repo's own spec battery (testing/voice-conformance: every fixture of the dialect, the captured
# transcripts, the cross-dialect bridge and the door's seams), driving the plane's door BOTH WAYS,
# linked and dropped in, through busbar's loader at the pin ($BUSBAR_CHECKOUT).
#
# THE THREE LEGS, as the brief states them:
#   control   every assertion holds the two doors (linked, dropped) to one answer, and the answer to a
#             check computed from the dialect's own wire (the fixtures), never from the door
#   negative  the RED arm: VOICE_CONFORM_RED=1 makes each leg's planted wrong answer the subject, and
#             every conformance leg must then go red; one that stays green is a battery that cannot
#             tell, and the suite fails
#   subject   the plane built from this checkout; one that cannot build is `fail`, armed
#
# The battery is run ONCE per check (normal, then RED) into a cache beside the verdicts, and each
# suite id judges its own dialect's rows of it: a leg that judges both dialects counts for both, a
# Twilio row (voice-twilio is its own row) is reported and counts for neither.
set -uo pipefail
dialect="${1:?usage: voice.sh openai|gemini}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${CONFORMANCE_SUITE:?}" "${CONFORMANCE_OUT:?}" "${BUSBAR_CHECKOUT:?}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/target}"
run="$(dirname "$CONFORMANCE_OUT")/voice-run"

if [ ! -f "$run/done" ]; then
  rm -rf "$run"; mkdir -p "$run"
  if bash "$here/testing/conformance/voice-overlay.sh" "$here" "$run/ws" >"$run/overlay.log" 2>&1; then
    export VC_WORKSPACE_MANIFEST="$run/ws/Cargo.toml"
    ( export VOICE_RESULT_LOG="$run/results.tsv"; unset VOICE_CONFORM_RED
      bash "$here/testing/voice-conformance/voice-conformance.sh" --verdict >"$run/battery.log" 2>&1 )
    echo "$?" >"$run/battery.rc"
    ( export VOICE_RESULT_LOG="$run/red.tsv" VOICE_CONFORM_RED=1
      bash "$here/testing/voice-conformance/voice-conformance.sh" --verdict >"$run/red.log" 2>&1 )
    echo "$?" >"$run/red.rc"
  else
    echo "the plane's workspace could not be written: $(tail -n 3 "$run/overlay.log" | tr '\n' ' ')" >"$run/subject-failed"
  fi
  touch "$run/done"
fi

# what the harness build said, for the log of a run that could not build it
grep -h -E '^error' -A6 "$run/battery.log" 2>/dev/null | head -n 40
python3 -I - "$dialect" "$run" "$CONFORMANCE_OUT/verdict.json" "$CONFORMANCE_SUITE" <<'PY'
import json, os, sys
dialect, run, out, suite = sys.argv[1:5]
standard = {"openai": "OpenAI Realtime API", "gemini": "Google Gemini Live API"}[dialect]

def verdict(status, reason):
    v = {"suite": suite, "status": status, "armed": True, "standard": standard,
         "plan": "spec battery + replay", "reason": reason}
    with open(out, "w") as f:
        json.dump(v, f, indent=2, sort_keys=True)
        f.write("\n")
    print(status, reason)
    sys.exit(0 if status == "pass" else 1)

def rows(name):
    p = os.path.join(run, name)
    if not os.path.exists(p):
        return None
    r = []
    for line in open(p, encoding="utf-8", errors="replace"):
        f = line.rstrip("\n").split("\t", 3)
        f += [""] * (4 - len(f))
        r.append(f)
    return r

def who(leg, slice_, detail):
    """The dialects a row judges: {'openai'}, {'gemini'}, {'twilio'}, both, or none (governance)."""
    both = {"openai", "gemini"}
    if leg == "governance":
        return set()
    if leg.startswith("twilio-") or slice_ == "twilio":
        return {"twilio"}
    if leg in ("spec-per-dialect",):
        return {slice_} if slice_ in both else both
    if leg == "cross-parity":
        return {"oo": {"openai"}, "gg": {"gemini"}}.get(slice_, both)
    if leg == "gemini-live-route":
        return {"gemini"}
    if leg in ("replay", "tool-reply"):
        for d in ("openai", "gemini"):
            if detail.startswith(d):
                return {d}
    return both

if os.path.exists(os.path.join(run, "subject-failed")):
    verdict("fail", "the subject could not be built: " + open(os.path.join(run, "subject-failed")).read().strip())
normal, red = rows("results.tsv"), rows("red.tsv")
if not normal:
    verdict("fail", "the battery wrote no results (see battery.log)")
if not red:
    verdict("fail", "the RED arm wrote no results (see red.log)")

def mine(rs):
    return [r for r in rs if dialect in who(*r[:2], r[3])]

bad = [r for r in mine(normal) if r[2] != "PASS"]
twilio = [r for r in normal if who(r[0], r[1], r[3]) == {"twilio"} and r[2] != "PASS"]
legs = sorted({r[0] for r in mine(normal)})
if bad:
    ex = "; ".join(f"{r[0]}/{r[1]}: {r[3][:160]}" for r in bad[:6])
    verdict("fail", f"{len(bad)} of {len(mine(normal))} {dialect} assertion(s) failed: {ex}")
if len(legs) < 10:
    verdict("fail", f"only {len(legs)} legs judged {dialect}; the battery is gutted")
# the negative control: every conformance leg that judges this dialect must go red under RED
redrows = mine(red)
stayed = [l for l in legs if not any(r[0] == l and r[2] != "PASS" for r in redrows)]
if stayed:
    verdict("fail", "the RED arm did not bite on: " + ", ".join(stayed))
verdict("pass", f"{len(mine(normal))} {dialect} assertions over {len(legs)} legs held on both doors (linked and dropped); "
        f"the RED arm turned all {len(legs)} red; {len(twilio)} twilio failure(s) reported to voice-twilio, not counted here")
PY
