#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (C) 2026 Busbar Inc and contributors
#
# Run the streaming plane's protocol fixtures through linked and dropped doors.
# --verdict runs each declared leg; --leg NAME [--slice S] runs one leg.
# --selftest exercises process-exit and result handling; --list lists the legs.
# Governance observations do not contribute to conformance verdicts.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# DISCOVERED, NOT ENUMERATED. Overridable so `--selftest` can point the exact same accounting code
# at fixture leg directories instead of the real ones.
VOICE_LEGS_DIR="${VOICE_LEGS_DIR:-$HERE/legs}"

say()  { printf '%s\n' "$*"; }
warn() { printf '%s\n' "$*" >&2; }
die()  { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

# The whole leading comment block, read to wherever it now ends rather than to a line number a later
# edit to the header would silently walk past.
usage() { sed -n '2,/^set -euo pipefail$/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'; }

# ── the leg contract ────────────────────────────────────────────────────────────────────────────
#
# Each `legs/<name>.sh`, when sourced, sets LEG_KIND / LEG_STATUS / LEG_SLICES and defines
# `leg_execute`. `_load_leg` RESETS the contract before every source so one leg's declaration can
# never bleed into the next, and rejects a leg that fails to declare itself — an under-declared leg
# is exactly the silent gap this whole file is arranged to refuse.
_load_leg() {
  local file="$1"
  LEG_KIND=""; LEG_STATUS=""; LEG_SLICES=()
  leg_execute() { die "leg '$1' declares no leg_execute"; }
  # shellcheck disable=SC1090
  . "$file"
  case "$LEG_KIND" in
    conformance|governance) ;;
    *) die "$(basename "$file"): LEG_KIND must be 'conformance' or 'governance', got '${LEG_KIND:-}'" ;;
  esac
  case "$LEG_STATUS" in
    pending|ready) ;;
    *) die "$(basename "$file"): LEG_STATUS must be 'pending' or 'ready', got '${LEG_STATUS:-}'" ;;
  esac
  [ "${#LEG_SLICES[@]}" -ge 1 ] || die "$(basename "$file"): LEG_SLICES must name at least one slice"
}

declared_legs() {
  local f
  for f in "$VOICE_LEGS_DIR"/*.sh; do
    [ -e "$f" ] || continue
    basename "$f" .sh
  done | sort
}

leg_file() { printf '%s/%s.sh' "$VOICE_LEGS_DIR" "$1"; }

# ── processing one leg ──────────────────────────────────────────────────────────────────────────
#
# Prints the leg's per-slice lines and returns:
#   0  pending, or ready and every slice PASS
#   1  ready and at least one slice FAIL (a real conformance finding)
#   2  a ready slice did not complete successfully (including one that produced NO result).
# A governance leg NEVER returns 1: its findings are observations, never a conformance verdict.
_process_leg() {
  local name="$1" only_slice="${2:-}" rc=0
  _load_leg "$(leg_file "$name")"
  # An optional single-slice filter, applied AFTER the (re)load so it is not clobbered by it. This
  # is how the workflow's matrix job runs exactly one dialect slice.
  [ -n "$only_slice" ] && LEG_SLICES=("$only_slice")
  local kindtag="conformance"; [ "$LEG_KIND" = governance ] && kindtag="governance (NOT a conformance result)"

  if [ "$LEG_STATUS" = pending ]; then
    say "  leg $name [$kindtag]: PENDING — scaffolded, no voice runtime yet"
    local s
    for s in "${LEG_SLICES[@]}"; do say "      · $s : PENDING"; done
    return 0
  fi

  # READY. Each slice must yield at least one RESULT line, or the leg is vacuous.
  local s out results execution_rc completion
  for s in "${LEG_SLICES[@]}"; do
    execution_rc=0
    out="$(leg_execute "$s")" || execution_rc=$?
    results="$(printf '%s\n' "$out" | grep -E '^RESULT ' || true)"
    completion="${out##*$'\n'}"
    # Assertion failures are a completed invocation with status 1, including the expected RED
    # probe. The harness prints this terminal line only after its whole script returned. A panic,
    # signal, build failure or an early exit after partial RESULT output is an execution failure.
    if { [ "$execution_rc" -ne 0 ] && [ "$execution_rc" -ne 1 ]; } \
       || [ "$completion" != "VOICE_COMPLETE $execution_rc" ] \
       || { [ "$execution_rc" -eq 1 ] && ! printf '%s\n' "$results" | grep -qE '^RESULT [^ ]+ FAIL'; }; then
      say "      · $s : EXECUTION FAILED — exit $execution_rc, no matching completed invocation"
      if [ -n "${VOICE_RESULT_LOG:-}" ]; then
        printf '%s\t%s\tEXECUTIONFAILED\texit %s, no matching completed invocation\n' \
          "$name" "$s" "$execution_rc" >>"$VOICE_RESULT_LOG"
      fi
      rc=2; continue
    fi
    if [ -z "$results" ]; then
      say "      · $s : NO RESULT — a READY leg that executed nothing is RED"
      # The dialect judge reads these execution failures beside ordinary result rows.
      if [ -n "${VOICE_RESULT_LOG:-}" ]; then
        printf '%s\t%s\tNORESULT\tREADY leg executed nothing\n' "$name" "$s" >>"$VOICE_RESULT_LOG"
      fi
      rc=2; continue
    fi
    if [ -n "${VOICE_RESULT_LOG:-}" ]; then
      printf '%s\n' "$results" | while IFS=' ' read -r _ slice verdict detail; do
        printf '%s\t%s\t%s\t%s\n' "$name" "$slice" "$verdict" "${detail:-}" >>"$VOICE_RESULT_LOG"
      done
    fi
    printf '%s\n' "$results" | while IFS=' ' read -r _ slice verdict detail; do
      say "      · $slice : $verdict ${detail:-}"
    done
    if [ "$LEG_KIND" != governance ] && printf '%s\n' "$results" | grep -qE '^RESULT [^ ]+ FAIL'; then
      [ "$rc" -eq 2 ] || rc=1
    fi
  done
  say "  leg $name [$kindtag]: $([ "$rc" -eq 0 ] && echo READY/ok || echo READY/RED)"
  return "$rc"
}

# ── the verdict emitter ─────────────────────────────────────────────────────────────────────────
#
emit_verdict() {
  local -a declared
  mapfile -t declared < <(declared_legs)

  say "== VOICE conformance battery — verdict =="
  say "   (pending legs are honest, not passing; a READY leg asserts real conformance or is RED)"
  say ""

  local name rc problems=0 pending=0 ready_ok=0 conformance_fail=0 governance_legs=0
  for name in "${declared[@]}"; do
    _load_leg "$(leg_file "$name")"
    [ "$LEG_KIND" = governance ] && governance_legs=$((governance_legs+1))
    rc=0; _process_leg "$name" || rc=$?
    case "$rc" in
      0) [ "$LEG_STATUS" = pending ] && pending=$((pending+1)) || ready_ok=$((ready_ok+1)) ;;
      1) conformance_fail=$((conformance_fail+1)) ;;
      2) problems=$((problems+1)) ;;
    esac
  done

  say ""
  say "  legs declared:       ${#declared[@]}  (governance: $governance_legs, not counted toward conformance)"
  say "  pending:             $pending"
  say "  ready & passing:     $ready_ok"
  say "  conformance failures:$conformance_fail"
  say "  accounting problems: $problems"

  if [ "$problems" -gt 0 ]; then
    warn "FAIL: the battery did not complete cleanly above — no verdict about busbar can be believed."
    return 2
  fi
  if [ "$conformance_fail" -gt 0 ]; then
    die "$conformance_fail conformance leg(s) failed. This IS a finding about busbar."
  fi
  say ""
  if [ "$ready_ok" -eq 0 ]; then
    say "VOICE verdict: SCAFFOLD self-test PASS, $pending leg(s) pending. NOT a conformance pass —"
    say "no voice leg asserts real conformance yet; the shape is enforced and every leg is accounted for."
  else
    say "VOICE verdict: $ready_ok ready leg(s) passed, $pending pending. Conformance holds for the ready legs."
  fi
}

# ── one leg on demand (the workflow's per-leg jobs call this) ─────────────────────────────────────
run_one_leg() {
  local name="$1" only_slice="${2:-}"
  local file; file="$(leg_file "$name")"
  [ -f "$file" ] || die "no such leg '$name' (declared legs: $(declared_legs | paste -sd' ' -))"
  say "== VOICE conformance battery — leg '$name' =="
  _load_leg "$file"
  local rc=0; _process_leg "$name" "$only_slice" || rc=$?
  case "$rc" in
    0) [ "$LEG_STATUS" = pending ] \
         && say "leg '$name': PENDING (scaffold) — exiting 0; this is not a conformance pass" \
         || say "leg '$name': READY and passing" ;;
    1) die "leg '$name': conformance FAIL — a finding about busbar" ;;
    2) warn "FAIL: leg '$name': execution did not complete"; return 2 ;;
  esac
}

main() {
  case "${1:---verdict}" in
    --selftest) python3 -I "$HERE/tests/test_execution.py" ;;
    --verdict)  emit_verdict ;;
    --list)
      local name
      for name in $(declared_legs); do
        _load_leg "$(leg_file "$name")"
        printf '  %-18s kind=%-11s status=%-7s slices=[%s]\n' "$name" "$LEG_KIND" "$LEG_STATUS" "${LEG_SLICES[*]}"
      done ;;
    --leg)
      shift; [ $# -ge 1 ] || die "--leg needs a leg name"
      local legname="$1" slice=""; shift || true
      while [ $# -gt 0 ]; do
        case "$1" in
          --slice) slice="${2:-}"; shift 2 ;;
          *) die "unknown argument to --leg: $1" ;;
        esac
      done
      run_one_leg "$legname" "$slice" ;;
    --help|-h) usage ;;
    *) die "unknown mode: $1 (try --help)" ;;
  esac
}

main "$@"
