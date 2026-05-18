#!/usr/bin/env bash
# watch-kernel-diff.sh — “what the hell is going on” for block_kernel_diff
#
# Defaults match a typical 0→500k run with logs next to the chunk cache:
#   $BLOCK_CACHE_DIR/kernel_diff_0_500k.{runner.log,jsonl,divergences.jsonl}
#
# Usage (default: one line — run, compare window, height, div, bps, phase):
#   ./scripts/watch-kernel-diff.sh              # refresh every 2s
#   ./scripts/watch-kernel-diff.sh --verbose    # full dashboard
#   ./scripts/watch-kernel-diff.sh --no-clear   # compact: newline each tick (log-friendly);
#                                                 verbose: still redraws from top (no scroll spam)
#   ./scripts/watch-kernel-diff.sh --once       # one line to stdout
#   ./scripts/watch-kernel-diff.sh --follow     # tail -f runner log
#
# Optional:
#   WATCH_KERNEL_DIFF_DETAIL_MAX (default 90) — max chars of phase detail on the one-liner.
#   WATCH_KERNEL_DIFF_STATE — file storing last height+time for instant bps (default:
#     ~/.cache/blvm-bench/watch-kernel-diff-$STEM.state)
#   bps_now — Δheight / Δwall between refreshes (subsecond time). Shows "-" when height did not
#     change in that interval. Trust bps= (rolling window from stderr) for a stable rate.
#   WATCH_KERNEL_DIFF_JSONL_START_TAIL_LINES — lines of jsonl tail to search for the --start row
#     when proving jsonl belongs to this run (default 500000). Only used when h_prog is empty.
#   WATCH_KERNEL_DIFF_LOG_TAIL_LINES (default 120000) — read only this many **lines** from the
#     end of the runner log (avoids slurping multi-GB append-only logs every refresh).
#
# Override paths:
#   BLOCK_CACHE_DIR=/path/to/cache   (default: $HOME/.cache/blvm-bench, then try
#                                     $HOME/.local/share/blvm-kernel-diff/chunk-cache)
#   KERNEL_DIFF_LOG_STEM=my_run      (default: kernel_diff_0_500k)
#   Or set explicitly:
#   KERNEL_DIFF_RUNNER_LOG=... KERNEL_DIFF_JSONL_LOG=... KERNEL_DIFF_DIVERGENCE_LOG=...
#
# Requires: bash, standard Unix tools; optional python3 for pretty divergence JSON.

set -uo pipefail

REFRESH_SEC="${REFRESH_SEC:-2}"
CLEAR_SCREEN=true
MODE="watch"
ONCE=false
VERBOSE=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-clear) CLEAR_SCREEN=false; shift ;;
    --once|-1) ONCE=true; shift ;;
    --follow|-f) MODE="follow"; shift ;;
    --verbose|-v) VERBOSE=true; shift ;;
    -h|--help)
      sed -n '2,22p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

has_chunks_meta() {
  local root="$1"
  [[ -f "$root/chunks/chunks.meta" || -f "$root/chunks.meta" ]]
}

# Search dirs for runner logs, pick the most recently written one.
_find_newest_runner_log() {
  local best_t=0 best_f="" f t
  local search_dirs=()
  [[ -n "${BLOCK_CACHE_DIR:-}" ]] && search_dirs+=("$BLOCK_CACHE_DIR")
  search_dirs+=(
    "$HOME/.local/share/blvm-kernel-diff/chunk-cache"
    "$HOME/.cache/blvm-bench"
  )
  for d in "${search_dirs[@]}"; do
    for f in "$d"/kernel_diff_*.runner.log; do
      [[ -f "$f" ]] || continue
      t=$(stat -c '%Y' "$f" 2>/dev/null || stat -f '%m' "$f" 2>/dev/null || echo 0)
      if (( t > best_t )); then
        best_t=$t
        best_f="$f"
      fi
    done
  done
  echo "$best_f"
}

# Resolve RUNNER / STEM / BLOCK_CACHE_DIR / JSONL / DIVL.
# Priority: explicit env vars > newest runner log found by mtime.
if [[ -n "${KERNEL_DIFF_RUNNER_LOG:-}" ]]; then
  RUNNER="$KERNEL_DIFF_RUNNER_LOG"
  BLOCK_CACHE_DIR="${BLOCK_CACHE_DIR:-$(dirname "$RUNNER")}"
  STEM="${KERNEL_DIFF_LOG_STEM:-$(basename "$RUNNER" .runner.log)}"
else
  _newest="$(_find_newest_runner_log)"
  if [[ -n "$_newest" ]]; then
    RUNNER="$_newest"
    BLOCK_CACHE_DIR="${BLOCK_CACHE_DIR:-$(dirname "$_newest")}"
    STEM="${KERNEL_DIFF_LOG_STEM:-$(basename "$_newest" .runner.log)}"
  else
    # Nothing found — fall back to conventional defaults so error messages are useful.
    BLOCK_CACHE_DIR="${BLOCK_CACHE_DIR:-$HOME/.cache/blvm-bench}"
    STEM="${KERNEL_DIFF_LOG_STEM:-kernel_diff_0_500k}"
    RUNNER="$BLOCK_CACHE_DIR/${STEM}.runner.log"
  fi
fi

JSONL="${KERNEL_DIFF_JSONL_LOG:-$BLOCK_CACHE_DIR/${STEM}.jsonl}"
DIVL="${KERNEL_DIFF_DIVERGENCE_LOG:-$BLOCK_CACHE_DIR/${STEM}.divergences.jsonl}"

if [[ "$MODE" == "follow" ]]; then
  echo "Following: $RUNNER (Ctrl-C to stop)"
  [[ -f "$RUNNER" ]] || { echo "No file yet: $RUNNER" >&2; exit 1; }
  exec tail -n 30 -f "$RUNNER"
fi

clear_screen() {
  # Move cursor to top-left WITHOUT blanking first, then write, then clear trailing
  # lines.  The old \033[2J approach wiped the screen before painting the new frame,
  # causing a visible blank-flash every 2 s.  \033[H (home) + write + \033[J (erase
  # below) keeps the terminal continuously painted.
  $CLEAR_SCREEN && printf '\033[H'
}

# Compact one-liner must fit on one physical row or \r only clears the last wrapped line
# and the terminal scrolls forever.  Truncate to terminal width minus 1.
_truncate_line() {
  local line="$1" max_cols="${2:-120}"
  local w
  w=$(tput cols 2>/dev/null || echo "$max_cols")
  [[ "$w" =~ ^[0-9]+$ ]] || w=$max_cols
  (( w < 20 )) && w=80
  if [[ ${#line} -ge "$w" ]]; then
    echo "${line:0:$((w - 1))}…"
  else
    echo "$line"
  fi
}

# Trim leading whitespace so one-line detail stays readable when truncated.
_strip_kernel_log_detail() {
  printf '%s' "$1" | sed 's/^[[:space:]]*//'
}

# From KERNEL_DIFF_RUN line: start=… end=Some(900001) or end=None
_kernel_diff_parse_compare_window() {
  local line="$1"
  WS="" WE=""
  [[ "$line" =~ start=([0-9]+) ]] && WS="${BASH_REMATCH[1]}"
  if [[ "$line" =~ end=Some\(([0-9]+)\) ]]; then
    WE="${BASH_REMATCH[1]}"
  elif [[ "$line" =~ end=None ]]; then
    WE="?"
  fi
  if [[ -n "$WS" && -n "$WE" ]]; then
    echo "${WS}-${WE}"
  elif [[ -n "$WS" ]]; then
    echo "$WS-?"
  fi
}

# Printed after seed/import; until then runner log has no KERNEL_DIFF_RUN — use argv.
_kernel_diff_window_from_proc() {
  local pid="$1" args=""
  [[ -n "$pid" ]] && [[ -r "/proc/$pid/cmdline" ]] || return
  args=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null) || return
  local ws we=""
  [[ "$args" =~ --start[[:space:]]+([0-9]+) ]] && ws="${BASH_REMATCH[1]}"
  [[ "$args" =~ --end[[:space:]]+([0-9]+) ]] && we="${BASH_REMATCH[1]}"
  if [[ -n "$ws" && -n "$we" ]]; then
    echo "${ws}-${we}"
  elif [[ -n "$ws" ]]; then
    echo "${ws}-?"
  fi
}

_kernel_diff_start_from_proc() {
  local pid="$1" args=""
  [[ -n "$pid" ]] && [[ -r "/proc/$pid/cmdline" ]] || return
  args=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null) || return
  [[ "$args" =~ --start[[:space:]]+([0-9]+) ]] && echo "${BASH_REMATCH[1]}"
}

# Last jsonl height, with resume fix when append order goes high→low across runs.
# Only reads the last few lines (never slurps a multi-GB jsonl).
_kernel_diff_jsonl_tail_height() {
  local f="$1"
  [[ -f "$f" ]] && command -v python3 >/dev/null 2>&1 || return
  tail -n 24 "$f" 2>/dev/null | python3 -c '
import json, sys
lines = [ln.strip() for ln in sys.stdin if ln.strip()]
if not lines:
    sys.exit(0)
try:
    h2 = json.loads(lines[-1]).get("height")
except (json.JSONDecodeError, TypeError, AttributeError, ValueError):
    sys.exit(0)
if h2 is None:
    sys.exit(0)
if len(lines) >= 2:
    try:
        h1 = json.loads(lines[-2]).get("height")
    except (json.JSONDecodeError, TypeError, AttributeError, ValueError):
        h1 = None
    if h1 is not None and h2 < h1:
        print(int(h2))
        raise SystemExit(0)
print(int(h2))
'
}

# True if a recent jsonl tail contains a row for this compare's --start (proves compare began).
# Default tail is large: after hundreds of thousands of blocks the --start line is no longer in
# the last few thousand lines, and without this we would drop jsonl_ok and freeze h= on PROGRESS
# every 100 blocks (bps_now stuck at "-" / 0).
_kernel_diff_jsonl_has_start_height() {
  local f="$1" start="$2" n="${3:-${WATCH_KERNEL_DIFF_JSONL_START_TAIL_LINES:-500000}}"
  [[ -f "$f" ]] || return 1
  [[ "$start" =~ ^[0-9]+$ ]] || return 1
  tail -n "$n" "$f" 2>/dev/null | grep -q -E "\"height\"[[:space:]]*:[[:space:]]*$start([^0-9]|$)"
}

# Infer what block_kernel_diff is doing *right now* from the tail of the current-run log slice.
# Prints: PHASE_KEY|short_detail (detail is ASCII-ish, length-capped).
_kernel_diff_infer_phase() {
  local slice="$1"
  local L short phase detail maxd=90
  maxd="${WATCH_KERNEL_DIFF_DETAIL_MAX:-90}"

  while IFS= read -r L; do
    [[ -z "${L//[[:space:]]/}" ]] && continue
    case "$L" in
      *KERNEL_DIFF_PROGRESS*) continue ;;
    esac
    short="$(_strip_kernel_log_detail "$L")"
    [[ ${#short} -gt "$maxd" ]] && short="${short:0:$((maxd - 1))}…"

    if [[ "$L" == *KERNEL_DIFF_MEM_RECOVERY* ]]; then
      printf 'mem_recovery|%s\n' "$short"
      return
    fi
    if [[ "$L" == *KERNEL_DIFF_MEM_AVAILABLE* ]]; then
      printf 'mem_available|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'[disk-utxo] flush overlay'* ]]; then
      printf 'disk_utxo_flush|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'materializing utxo_'* ]]; then
      printf 'utxo_materialize|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'loading utxo_'* ]] || [[ "$L" == *'RocksDB at height'* ]] || [[ "$L" == *'RocksDB reloaded'* ]]; then
      printf 'rocksdb_utxo_load|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'imported '* ]] && [[ "$L" == *'into Core chainstate'* ]]; then
      printf 'core_import_snapshot|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'block headers for Core seed'* ]]; then
      printf 'core_seed_headers|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'collecting '* ]] && [[ "$L" == *'block headers'* ]]; then
      printf 'core_seed_collect|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'headless seed'* ]] || [[ "$L" == *'wiping Core block-tree'* ]]; then
      printf 'core_chainstate_prep|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'utxo_'*'.bin already present'* ]] || [[ "$L" == *'skipping materialize'* ]]; then
      printf 'utxo_checkpoint_ready|%s\n' "$short"
      return
    fi
    if [[ "$L" == *KERNEL_DIFF_RUN* ]]; then
      printf 'starting_compare|%s\n' "$short"
      return
    fi
    if [[ "$L" == *'Opening chunk'* ]] || [[ "$L" == *Seeking* ]]; then
      printf 'chunk_io|%s\n' "$short"
      return
    fi
    if [[ "$L" == *KERNEL_DIFF_RSS_LIMIT* ]]; then
      printf 'rss_limit|%s\n' "$short"
      return
    fi
  done < <(echo "$slice" | tail -n 120 | tac)

  L=$(echo "$slice" | tail -n 5 | grep -v '^[[:space:]]*$' | tail -1)
  short="$(_strip_kernel_log_detail "${L:-}")"
  [[ ${#short} -gt "$maxd" ]] && short="${short:0:$((maxd - 1))}…"
  printf 'unknown|%s\n' "${short:-(no recent log lines)}"
}

# Runner log is **append-only** across restarts. `grep KERNEL_DIFF_PROGRESS | tail -1` without
# scoping returns the *previous* run's last line while the new process is still seeking or
# loading — you see run=1 with a frozen height/bps for a long time. Anchor each run at the last
# line matching the binary's startup banner (printed once per invocation).
_kernel_diff_log_start_line() {
  local f="$1" line
  [[ -f "$f" ]] || { echo 1; return; }
  # ASCII substring (avoid relying on the leading emoji in all locales).
  line=$(grep -n 'BLVM assume-valid: skipping' "$f" 2>/dev/null | tail -1 | cut -d: -f1)
  # When --blvm-assume-valid-height is 0 the banner is omitted; use the MemAvailable banner instead.
  if [[ -z "$line" || ! "$line" =~ ^[0-9]+$ ]]; then
    line=$(grep -n 'MemAvailable floor: will stop' "$f" 2>/dev/null | tail -1 | cut -d: -f1)
  fi
  [[ -n "$line" && "$line" =~ ^[0-9]+$ ]] || { echo 1; return; }
  echo "$line"
}

_kernel_diff_log_since() {
  local f="$1" start="$2"
  tail -n +"$start" "$f" 2>/dev/null
}

# Map internal phase keys to short human-readable labels for the one-liner.
_kernel_diff_phase_shortname() {
  case "$1" in
    mem_recovery) echo "mem_recovery" ;;
    mem_available) echo "mem_pressure" ;;
    disk_utxo_flush) echo "disk_utxo_flush" ;;
    utxo_materialize) echo "utxo_materialize" ;;
    rocksdb_utxo_load) echo "rocksdb_utxo_load" ;;
    core_import_snapshot) echo "core_import" ;;
    core_seed_headers) echo "core_seed(headers)" ;;
    core_seed_collect) echo "core_seed(collect)" ;;
    core_chainstate_prep) echo "core_prep" ;;
    utxo_checkpoint_ready) echo "checkpoint_ready" ;;
    starting_compare) echo "starting" ;;
    chunk_io) echo "chunk_io" ;;
    rss_limit) echo "rss_limit" ;;
    unknown) echo "unknown" ;;
    *) echo "$1" ;;
  esac
}

# One line: run, compare window, height, div, bps (window from stderr), bps_now (derivative), phase.
snapshot_compact() {
  local h_out="-" div="-" bps_w="-" bps_now="-" last="" run=0 err="" jsonl_nonempty=false log_start=1
  local win="" phase_line="" phase_key="" phase_detail="" phase_short="" avg="" rss=""
  local bkd_pid="" _run_line="" h_prog="" h_json="" jsonl_ok=0 st_start="" now_ts=""
  local state_file="${WATCH_KERNEL_DIFF_STATE:-$HOME/.cache/blvm-bench/watch-kernel-diff-$STEM.state}"
  local jsonl_trust_file="${state_file%.state}.jsonl-trust"
  local tail_lines="${WATCH_KERNEL_DIFF_LOG_TAIL_LINES:-120000}"

  if bkd_pid="$(pgrep_block_kernel_diff)"; then
    run=1
  fi
  # Drop jsonl start-cache when nothing is running (otherwise next run could inherit stale trust).
  if [[ "$run" -eq 0 ]]; then
    rm -f "$jsonl_trust_file" 2>/dev/null || true
  fi
  if [[ -f "$RUNNER" ]]; then
    if [[ "$run" -eq 1 ]]; then
      log_start=$(_kernel_diff_log_start_line "$RUNNER")
    else
      log_start=1
    fi
    # Never read the whole runner log into RAM — tail from EOF only.
    last=$(tail -n "$tail_lines" "$RUNNER" 2>/dev/null | grep "KERNEL_DIFF_PROGRESS" | tail -1)
    _run_line=$(tail -n "$tail_lines" "$RUNNER" 2>/dev/null | grep -F 'KERNEL_DIFF_RUN compare_window' | tail -1)
  fi
  if [[ -n "$bkd_pid" ]]; then
    win="$(_kernel_diff_window_from_proc "$bkd_pid")"
    st_start="$(_kernel_diff_start_from_proc "$bkd_pid")"
  fi
  if [[ -z "$win" ]]; then
    win="$(_kernel_diff_parse_compare_window "$_run_line")"
  fi
  if [[ -n "$last" ]]; then
    [[ "$last" =~ height=([0-9]+) ]] && h_prog="${BASH_REMATCH[1]}"
    [[ "$last" =~ divergences=([0-9]+) ]] && div="${BASH_REMATCH[1]}"
    [[ "$last" =~ bps=([0-9.]+) ]] && bps_w="${BASH_REMATCH[1]}"
    [[ "$last" =~ avg=([0-9.]+) ]] && avg="${BASH_REMATCH[1]}"
    [[ "$last" =~ rss=([0-9]+)MB ]] && rss="${BASH_REMATCH[1]}"
  fi
  [[ -s "$JSONL" ]] && jsonl_nonempty=true

  # Live height from jsonl (per block) when we can trust it vs stale previous-run tail.
  # Important: we must NOT require the --start row to stay inside a tiny jsonl tail forever — after
  # hundreds of thousands of blocks that row is far above EOF and has_start_height would fail,
  # forcing h= to KERNEL_DIFF_PROGRESS only (every 100 blocks) so bps_now never moves.
  if [[ "$run" -eq 1 ]] && $jsonl_nonempty && command -v python3 >/dev/null 2>&1; then
    h_json="$(_kernel_diff_jsonl_tail_height "$JSONL")"
    if [[ -n "$h_json" ]] && [[ "$h_json" =~ ^[0-9]+$ ]]; then
      if [[ -n "$_run_line" ]] || [[ -n "$st_start" ]]; then
        jsonl_ok=1
        if [[ -n "$st_start" ]]; then
          if [[ "$h_json" -lt "$st_start" ]]; then
            jsonl_ok=0
          elif [[ "$h_json" -eq "$st_start" ]]; then
            :
          elif [[ "$h_json" -gt "$st_start" ]]; then
            if [[ -n "$h_prog" ]] && [[ "$h_prog" =~ ^[0-9]+$ ]]; then
              # Same run: jsonl last line tracks near KERNEL_DIFF_PROGRESS height (≤100 blocks ahead).
              # Reject obvious stale tail (e.g. previous run's tip still at EOF during restart).
              if (( h_json < h_prog - 2 )) || (( h_json > h_prog + 200 )); then
                jsonl_ok=0
              fi
            else
              # No PROGRESS line yet: prove jsonl contains --start, then cache. Do not trust a
              # cached st_start if the tail still looks like a *previous* run's tip (EOF can show a
              # height far above --start until the new process appends).
              local _n="${WATCH_KERNEL_DIFF_JSONL_START_TAIL_LINES:-500000}"
              if (( h_json > st_start + 250000 )); then
                jsonl_ok=0
              elif [[ -f "$jsonl_trust_file" ]] && [[ "$(cat "$jsonl_trust_file" 2>/dev/null)" == "$st_start" ]]; then
                :
              elif _kernel_diff_jsonl_has_start_height "$JSONL" "$st_start" "$_n"; then
                mkdir -p "$(dirname "$jsonl_trust_file")"
                echo "$st_start" > "$jsonl_trust_file"
              else
                jsonl_ok=0
              fi
            fi
          fi
        fi
      else
        jsonl_ok=0
      fi
    fi
  fi

  h_out="-"
  if [[ "$jsonl_ok" -eq 1 ]]; then
    if [[ -n "$h_prog" ]] && [[ "$h_prog" =~ ^[0-9]+$ ]]; then
      if (( h_json > h_prog )); then h_out="$h_json"; else h_out="$h_prog"; fi
    else
      h_out="$h_json"
    fi
  elif [[ -n "$h_prog" ]]; then
    h_out="$h_prog"
  fi

  # Post-mortem height from jsonl (no stale guard needed).
  if [[ "$run" -eq 0 ]] && [[ "$h_out" == "-" ]] && $jsonl_nonempty && command -v python3 >/dev/null 2>&1; then
    h_out="$(_kernel_diff_jsonl_tail_height "$JSONL")"
    [[ -z "$h_out" ]] && h_out="-"
  fi

  if [[ "$div" == "-" ]] && [[ "$run" -eq 0 ]] && $jsonl_nonempty && [[ -f "$DIVL" ]]; then
    div=$(wc -l < "$DIVL" | tr -d ' ')
  fi

  # Instant bps: Δheight / Δwall between watch ticks (comparing only). Use subsecond time so
  # two samples are not forced into the same integer second. When Δheight==0, print "-" not
  # "0.00" — zero delta usually means the height sample did not advance yet (buffering / same
  # PROGRESS line), not that throughput is literally zero.
  now_ts=$(python3 -c 'import time; print(time.time())' 2>/dev/null)
  [[ -z "$now_ts" ]] && now_ts=$(date +%s)
  if [[ "$run" -eq 1 ]] && [[ "$h_out" =~ ^[0-9]+$ ]]; then
    local h_prev="" t_prev=""
    read -r h_prev t_prev < "$state_file" 2>/dev/null || true
    if [[ -n "$h_prev" ]] && [[ "$h_prev" =~ ^[0-9]+$ ]]; then
      bps_now="$(awk -v ho="$h_out" -v hp="$h_prev" -v n="$now_ts" -v tp="${t_prev:-}" '
        BEGIN {
          if (tp == "" || tp !~ /^[+-]?[0-9]*\.?[0-9]+([eE][+-]?[0-9]+)?$/) { print "-"; exit }
          ho += 0; hp += 0; n += 0; tp += 0
          if (ho < hp) { print "-"; exit }
          if (n <= tp) { print "-"; exit }
          d = ho - hp
          t = n - tp
          if (t < 0.001 || t > 600) { print "-"; exit }
          if (d == 0) { print "-"; exit }
          if (d >= 0) printf "%.2f", d / t
          else print "-"
        }')"
    fi
    mkdir -p "$(dirname "$state_file")"
    echo "$h_out $now_ts" > "$state_file"
  elif [[ -f "$state_file" ]]; then
    rm -f "$state_file" 2>/dev/null || true
  fi

  if [[ "$h_out" == "-" ]] && [[ -f "$RUNNER" ]]; then
    phase_line="$(_kernel_diff_infer_phase "$(tail -n 240 "$RUNNER" 2>/dev/null)")"
    phase_key="${phase_line%%|*}"
    phase_detail="${phase_line#*|}"
    phase_short="$(_kernel_diff_phase_shortname "$phase_key")"
    if [[ "$run" -eq 1 ]] && [[ -z "$last" ]] && [[ "$phase_key" == "unknown" ]]; then
      phase_detail="no KERNEL_DIFF_PROGRESS yet — tail log: tail -20 \"$RUNNER\""
    fi
  fi

  if [[ "$run" -eq 0 ]] && [[ -f "$RUNNER" ]]; then
    err=$(grep "^Error:" "$RUNNER" 2>/dev/null | tail -1 | sed 's/^Error:[[:space:]]*//' | tr '\n' ' ' | cut -c1-160)
  fi
  local wpart=""
  [[ -n "$win" ]] && wpart=" win=${win}"

  if [[ -n "$err" ]]; then
    printf 'run=%s%s h=%s div=%s bps=%s bps_now=%s err=%s\n' "$run" "$wpart" "$h_out" "$div" "$bps_w" "$bps_now" "$err"
    return
  fi

  # Comparing: have live height (jsonl and/or progress) after KERNEL_DIFF_RUN, or progress-only.
  if [[ "$h_out" != "-" ]] && [[ "$run" -eq 1 ]]; then
    local extra=""
    [[ -n "$avg" ]] && extra+=" avg=${avg}"
    [[ -n "$rss" ]] && extra+=" rss=${rss}MB"
    printf 'run=%s%s h=%s div=%s bps=%s bps_now=%s%s phase=comparing\n' "$run" "$wpart" "$h_out" "$div" "$bps_w" "$bps_now" "$extra"
    return
  fi

  if [[ "$h_out" != "-" ]] && [[ "$run" -eq 0 ]]; then
    printf 'run=%s%s h=%s div=%s bps=%s bps_now=%s phase=finished\n' "$run" "$wpart" "$h_out" "$div" "$bps_w" "$bps_now"
    return
  fi

  local detail=""
  [[ -n "$phase_detail" ]] && detail=" | ${phase_detail}"
  printf 'run=%s%s h=- div=- bps=- bps_now=- phase=%s%s\n' "$run" "$wpart" "${phase_short:-?}" "$detail"
}

pgrep_block_kernel_diff() {
  # Match real binary path only; avoid false positives on `grep block_kernel_diff`.
  local pid exe base
  for pid in $(pgrep -f '/block_kernel_diff' 2>/dev/null || true); do
    [[ "$pid" == "$$" ]] && continue
    exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null) || continue
    base="${exe##*/}"
    base="${base% (deleted)}"
    [[ "$base" == block_kernel_diff ]] || continue
    echo "$pid"
    return 0
  done
  return 1
}

snapshot() {
  local now diffpid vmrss vmpeak
  now="$(date '+%Y-%m-%d %H:%M:%S %Z')"

  echo "═══════════════════════════════════════════════════════════════════"
  echo "  watch-kernel-diff  $now"
  echo "═══════════════════════════════════════════════════════════════════"
  echo "  BLOCK_CACHE_DIR=$BLOCK_CACHE_DIR"
  echo "  logs: stem=$STEM"
  echo "    runner     $RUNNER"
  echo "    jsonl      $JSONL"
  echo "    divergences $DIVL"
  echo ""

  if diffpid="$(pgrep_block_kernel_diff)"; then
    vmrss=$(awk '/VmRSS/{printf "%.0f MiB", $2/1024}' "/proc/$diffpid/status" 2>/dev/null || echo "?")
    vmpeak=$(awk '/VmPeak/{printf "%.0f MiB", $2/1024}' "/proc/$diffpid/status" 2>/dev/null || echo "?")
    echo "── process ───────────────────────────────────────────────────────"
    printf "  RUNNING  PID=%s  RSS=%s  VmPeak=%s\n" "$diffpid" "$vmrss" "$vmpeak"
    tr '\0' ' ' < "/proc/$diffpid/cmdline" 2>/dev/null | fold -s -w 72 | sed 's/^/  cmd: /'
  else
    echo "── process ───────────────────────────────────────────────────────"
    echo "  NOT RUNNING (no block_kernel_diff executable in /proc/*/exe)"
  fi
  echo ""

  echo "── run header / summary (from runner log) ────────────────────────"
  if [[ -f "$RUNNER" ]]; then
    grep "KERNEL_DIFF_RUN\|KERNEL_DIFF_SUMMARY\|KERNEL_DIFF_STATUS" "$RUNNER" 2>/dev/null | tail -5 | sed 's/^/  /'
    echo ""
    echo "── last progress (tail of runner log — same as compact watch) ─────"
    local log_start=1 _prog _tl="${WATCH_KERNEL_DIFF_LOG_TAIL_LINES:-120000}"
    if [[ -n "$diffpid" ]]; then
      log_start=$(_kernel_diff_log_start_line "$RUNNER")
    fi
    _prog=$(tail -n "$_tl" "$RUNNER" 2>/dev/null | grep "KERNEL_DIFF_PROGRESS" | tail -1)
    if [[ -n "$_prog" ]]; then
      echo "$_prog" | sed 's/^/  /'
    else
      echo "  (no KERNEL_DIFF_PROGRESS after log line $log_start — still startup/seek/import, or stalled)"
    fi
    echo ""
    echo "── compare window + inferred phase (compact one-liner logic) ─────"
    local _run_v _win_v _inf_v _pk_v _pd_v
    _run_v=$(tail -n "$_tl" "$RUNNER" 2>/dev/null | grep -F 'KERNEL_DIFF_RUN compare_window' | tail -1)
    _win_from_proc=""
    if [[ -n "$diffpid" ]]; then
      _win_from_proc="$(_kernel_diff_window_from_proc "$diffpid")"
    fi
    _win_v="$_win_from_proc"
    if [[ -z "$_win_v" ]]; then
      _win_v="$(_kernel_diff_parse_compare_window "$_run_v")"
    fi
    if [[ -n "$_win_v" ]]; then
      if [[ -n "$_win_from_proc" ]]; then
        echo "  compare window: ${_win_v}  (from process argv — log may not have KERNEL_DIFF_RUN yet)"
      else
        echo "  compare window: ${_win_v}  (from KERNEL_DIFF_RUN line)"
      fi
    else
      echo "  compare window: (unknown — no PID argv match and no KERNEL_DIFF_RUN in log)"
    fi
    if [[ -z "$_prog" ]]; then
      _inf_v="$(_kernel_diff_infer_phase "$(tail -n 320 "$RUNNER" 2>/dev/null)")"
      _pk_v="${_inf_v%%|*}"
      _pd_v="${_inf_v#*|}"
      printf "  inferred phase: %s\n" "$(_kernel_diff_phase_shortname "$_pk_v")"
      echo "  detail: ${_pd_v}"
    else
      echo "  inferred phase: comparing (KERNEL_DIFF_PROGRESS present)"
    fi
    echo ""
    echo "── stderr highlights (divergences / errors) ───────────────────────"
    grep -E "KERNEL_DIFF_DIVERGENCE|^Error:|^Error |panicked|FAILED|divergence" "$RUNNER" 2>/dev/null | tail -12 | sed 's/^/  /' || echo "  (none matched)"
    echo ""
    echo "── last 12 lines (raw tail) ──────────────────────────────────────"
    tail -12 "$RUNNER" | sed 's/^/  /'
  else
    echo "  (no runner log: $RUNNER)"
  fi
  echo ""

  echo "── jsonl ───────────────────────────────────────────────────────────"
  if [[ -f "$JSONL" ]]; then
    local jsonl_lines
    jsonl_lines=$(wc -l < "$JSONL")
    printf "  %s lines\n" "$jsonl_lines"
    [[ "$jsonl_lines" -gt 0 ]] && tail -1 "$JSONL" | sed 's/^/  last: /'
  else
    echo "  (missing: $JSONL)"
  fi
  echo ""

  echo "── divergence log ────────────────────────────────────────────────"
  if [[ -f "$DIVL" ]]; then
    local d
    d=$(wc -l < "$DIVL")
    printf "  %s divergence record(s)\n" "$d"
    if [[ "$d" -gt 0 ]]; then
      if command -v python3 >/dev/null 2>&1; then
        tail -5 "$DIVL" | while read -r line; do
          python3 -c "import json,sys; d=json.loads(sys.argv[1]); print(' ', d.get('height'), d.get('block_hash','')[:18], d.get('blvm'), d.get('core'), (d.get('blvm_detail') or '')[:70])" "$line" 2>/dev/null || echo "  $line"
        done
      else
        tail -5 "$DIVL" | sed 's/^/  /'
      fi
    fi
  else
    echo "  (missing: $DIVL)"
  fi
  echo ""

  echo "── chunk cache ─────────────────────────────────────────────────────"
  local chunks="$BLOCK_CACHE_DIR/chunks"
  [[ -f "$BLOCK_CACHE_DIR/chunks.meta" ]] && chunks="$BLOCK_CACHE_DIR"
  if [[ -f "$chunks/chunks.meta" ]]; then
    echo "  ok: chunks.meta at $chunks"
    ls -la "$chunks/chunks.meta" 2>/dev/null | sed 's/^/  /'
  else
    echo "  WARNING: no chunks.meta under $BLOCK_CACHE_DIR or $BLOCK_CACHE_DIR/chunks"
    echo "    block_kernel_diff wants BLOCK_CACHE_DIR = parent of chunks/ (with chunks.meta)."
    local alt="$HOME/.cache/blvm-bench"
    if [[ "$BLOCK_CACHE_DIR" != "$alt" ]] && has_chunks_meta "$alt"; then
      echo "    hint: try  BLOCK_CACHE_DIR=$alt  (chunk tree found there)"
    fi
  fi
  echo ""

  echo "── checkpoint dir (UTXO ladder) ──────────────────────────────────"
  local cp="${UTXO_CHECKPOINT_DIR:-differential_checkpoints_fixed_v1}"
  local cpabs="$BLOCK_CACHE_DIR/$cp"
  if [[ -d "$cpabs" ]]; then
    echo "  $cpabs"
    (cd "$cpabs" && ls -1 utxo_0.bin delta_*.bin 2>/dev/null | head -8 | sed 's/^/  /')
    local nd
    nd=$(ls -1 "$cpabs"/delta_*.bin 2>/dev/null | wc -l)
    printf "  … delta_*.bin count: %s\n" "$nd"
  else
    echo "  (missing dir: $cpabs)"
    echo "    Seed utxo_0.bin + delta_* here, or set UTXO_CHECKPOINT_DIR + BLOCK_CACHE_DIR."
  fi
  echo "═══════════════════════════════════════════════════════════════════"
}

if $ONCE; then
  if $VERBOSE; then
    snapshot
  else
    snapshot_compact
  fi
  exit 0
fi

while true; do
  if $VERBOSE; then
    # Buffer the entire frame then redraw from the top.  Always use \033[H … \033[J so
    # we never append to the scrollback (that was what filled the console).  --no-clear
    # only skips the *full* screen erase (\033[2J); we still home+erase-below to replace.
    _frame=$(snapshot; printf "\n  refresh every %ss  (%s — Ctrl-C to quit)\n" \
      "$REFRESH_SEC" "$($CLEAR_SCREEN && echo 'clears' || echo 'no-clear')")
    # Home + erase-below redraws the whole dashboard without \033[2J (no blank flash)
    # and without appending to scrollback (fixes runaway line count).
    printf '\033[H%s\033[J' "$_frame"
  else
    # Compact one-liner: one physical line, truncated so \r never leaves wrapped junk.
    _line=$(snapshot_compact | tr -d '\n')
    _line=$(_truncate_line "$_line" 120)
    if $CLEAR_SCREEN; then
      printf '\r\033[K%s' "$_line"
    else
      printf '%s\n' "$_line"
    fi
  fi
  sleep "$REFRESH_SEC"
done
