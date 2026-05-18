#!/usr/bin/env bash
# One screen of key=value data. Env: BLOCK_CACHE_DIR, UTXO_CHECKPOINT_DIR, REFRESH_SEC (default 5).
# Compact checkpoints line includes mem=rSSS/aAAA (chunk_utxo VmRSS GiB / system MemAvailable GiB; r- when not running).
# Long help: KERNEL_DIFF_STATUS_VERBOSE=1 ./scripts/kernel-diff-status.sh --help
#
# Env: CHUNK_UTXO_CHECKPOINT_LOG, CHUNK_UTXO_STATUS_FILE, CHUNK_UTXO_REHYDRATE_STATUS_FILE,
# KERNEL_DIFF_LOG, RESUME_STATE_PATH
#
# Disk-utxo rehydration: `{BLOCK_CACHE_DIR}/.chunk_utxo_rehydrate.status` (LOADED TOTAL CP_H TS)
# while streaming a checkpoint into RocksDB — compact line shows rehydr=… until import finishes.
# After import the file is removed → rehydr=-.  **Manual LSM compact** (rare; shrinking on disk) is shown as
# rehydr=compact and bps=compact when the Rust binary writes `.chunk_utxo_compact.status`.
# If the live log is not under `BLOCK_CACHE_DIR` (e.g. stderr » `/tmp/blvm-checkpoint.log` or
# `/tmp/chunk_utxo_fixed_v1.log`), set **`CHUNK_UTXO_CHECKPOINT_LOG`** to that path — or leave it unset:
# the script auto-picks the **newest** mtime among
# `{BLOCK_CACHE_DIR}/chunk_utxo_fixed_v1.log`, `/tmp/chunk_utxo_fixed_v1.log`, and `/tmp/blvm-checkpoint.log`,
# so `seek=` and log-derived hints follow where you actually tee’d output.
#
# BPS uses `{BLOCK_CACHE_DIR}/.chunk_utxo_checkpoints.status` (HEIGHT unix_secs): HEIGHT is the
# last **connected** block (default every 10 blocks via DISK_UTXO_STATUS_WRITE_EVERY). Durable resume tip
# is `.chunk_utxo_disk_tip` / RocksDB meta — do not mix them. Rate = Δheight / Δwall_clock. State:
# `{BLOCK_CACHE_DIR}/.kernel_diff_status_bps.state` (line: HEIGHT UNIX_TS PID LASTBPS) so
# ` --once` works, not only the watch loop. Override with KERNEL_DIFF_STATUS_POLL_STATE_FILE.
#
# During **disk-utxo rehydration**, block height does not advance; `bps=` shows **UTXO rows/s**
# (e.g. `359k/s` or `~165k/s`) from the log’s `[disk-utxo] commit … (NNN k/s) … total A/B` lines
# plus a short wall-clock window. State: `{BLOCK_CACHE_DIR}/.kernel_diff_status_rehydr.state`.
# Override: KERNEL_DIFF_STATUS_REHYDR_STATE_FILE.
# Self-test: `./scripts/test-kernel-diff-status-bps.sh`
#
# kdiff=idle means block_kernel_diff is not running (UTXO checkpointing does not use it).
# Verbose multi-line mode: KERNEL_DIFF_STATUS_VERBOSE=1

set -uo pipefail

MODE="all"
CHK_ANCHOR_H=""
CHK_ANCHOR_T=""
CHK_LAST_BPS=""
CHK_PREV_H=""
CHK_PREV_SEEK_PCT=""
CHK_PREV_SEEK_T=""
CHK_POLL_H=""
CHK_POLL_T=""
CHK_LAST_REHYDR_BPS=""
ONCE=false

# Persisted across separate ` --once` invocations (same BLOCK_CACHE_DIR). Lines: HEIGHT UNIX_TS PID
bps_poll_state_file() {
  if [[ -n "${KERNEL_DIFF_STATUS_POLL_STATE_FILE:-}" ]]; then
    echo "$KERNEL_DIFF_STATUS_POLL_STATE_FILE"
  elif [[ -n "$BLOCK_CACHE_DIR" ]]; then
    echo "$BLOCK_CACHE_DIR/.kernel_diff_status_bps.state"
  else
    echo ""
  fi
}

rehydr_poll_state_file() {
  if [[ -n "${KERNEL_DIFF_STATUS_REHYDR_STATE_FILE:-}" ]]; then
    echo "$KERNEL_DIFF_STATUS_REHYDR_STATE_FILE"
  elif [[ -n "$BLOCK_CACHE_DIR" ]]; then
    echo "$BLOCK_CACHE_DIR/.kernel_diff_status_rehydr.state"
  else
    echo ""
  fi
}

# Match the real binary only. `pgrep -f chunk_utxo_checkpoints` false-positives on any
# command line containing that substring (e.g. `cat …/.chunk_utxo_checkpoints.status`).
chunk_utxo_pid() {
  if [[ "${KERNEL_DIFF_STATUS_MOCK_RUNNING:-0}" == 1 ]]; then
    echo "${KERNEL_DIFF_STATUS_MOCK_PID:-999999}"
    return 0
  fi
  local pid exe base
  for pid in $(pgrep -f '/chunk_utxo_checkpoints' 2>/dev/null); do
    [[ "$pid" == "$$" ]] && continue
    exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null) || continue
    base="${exe##*/}"
    base="${base% (deleted)}"   # binary replaced on disk after rebuild; still the right process
    [[ "$base" == chunk_utxo_checkpoints ]] || continue
    echo "$pid"
    return 0
  done
  return 1
}

chunk_utxo_running() {
  [[ "${KERNEL_DIFF_STATUS_MOCK_RUNNING:-0}" == 1 ]] && return 0
  chunk_utxo_pid >/dev/null 2>&1
}

# Wall "now" for status age/stale tests; tests set KERNEL_DIFF_STATUS_MOCK_NOW.
status_now_s() {
  if [[ -n "${KERNEL_DIFF_STATUS_MOCK_NOW:-}" ]]; then
    echo "${KERNEL_DIFF_STATUS_MOCK_NOW}"
  else
    date +%s
  fi
}

REFRESH_SEC="${REFRESH_SEC:-5}"
VERBOSE="${KERNEL_DIFF_STATUS_VERBOSE:-0}"

usage() { sed -n '2,6p' "$0" | sed 's/^# \?//'; exit "${1:-0}"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help|help) usage 0 ;;
    --once|-1) ONCE=true; shift ;;
    checkpoints|differential|all) MODE="$1"; shift ;;
    *) echo "Unknown: $1" >&2; usage 1 ;;
  esac
done

BLOCK_CACHE_DIR="${BLOCK_CACHE_DIR:-}"
CHECKPOINT_SUBDIR="${UTXO_CHECKPOINT_DIR:-differential_checkpoints_fixed_v1}"
CHUNK_LOG="${CHUNK_UTXO_CHECKPOINT_LOG:-}"
[[ -z "$CHUNK_LOG" && -n "$BLOCK_CACHE_DIR" ]] && CHUNK_LOG="$BLOCK_CACHE_DIR/chunk_utxo_fixed_v1.log"
# Default log path often does not exist (nohup » /tmp). Prefer real + freshest file for seek/BPS/tail parsing.
if [[ -z "${CHUNK_UTXO_CHECKPOINT_LOG:-}" ]]; then
  _tmp_chunk_log="/tmp/chunk_utxo_fixed_v1.log"
  if [[ -f "$_tmp_chunk_log" ]]; then
    if [[ ! -f "$CHUNK_LOG" ]] || [[ "$_tmp_chunk_log" -nt "$CHUNK_LOG" ]]; then
      CHUNK_LOG="$_tmp_chunk_log"
    fi
  elif [[ -n "$CHUNK_LOG" && ! -f "$CHUNK_LOG" ]]; then
    CHUNK_LOG=""
  fi
  # Common redirect: `2>/tmp/blvm-checkpoint.log` (not chunk_utxo_fixed_v1.log).
  _blvm_cp_log="/tmp/blvm-checkpoint.log"
  if [[ -f "$_blvm_cp_log" ]]; then
    if [[ -z "$CHUNK_LOG" ]] || [[ ! -f "$CHUNK_LOG" ]] || [[ "$_blvm_cp_log" -nt "$CHUNK_LOG" ]]; then
      CHUNK_LOG="$_blvm_cp_log"
    fi
  fi
fi

# Paths to scan for `[disk-utxo]` lines (commit / compact). Deduped.
disk_utxo_log_paths() {
  [[ -n "${CHUNK_UTXO_CHECKPOINT_LOG:-}" ]] && echo "${CHUNK_UTXO_CHECKPOINT_LOG}"
  [[ -n "$BLOCK_CACHE_DIR" ]] && echo "$BLOCK_CACHE_DIR/chunk_utxo_fixed_v1.log"
  [[ -f /tmp/chunk_utxo_fixed_v1.log ]] && echo /tmp/chunk_utxo_fixed_v1.log
  [[ -f /tmp/blvm-checkpoint.log ]] && echo /tmp/blvm-checkpoint.log
}

chunk_utxo_status_path() {
  if [[ -n "${CHUNK_UTXO_STATUS_FILE:-}" ]]; then
    echo "$CHUNK_UTXO_STATUS_FILE"
  elif [[ -n "$BLOCK_CACHE_DIR" ]]; then
    echo "$BLOCK_CACHE_DIR/.chunk_utxo_checkpoints.status"
  else
    echo ""
  fi
}

chunk_utxo_rehydrate_status_path() {
  if [[ -n "${CHUNK_UTXO_REHYDRATE_STATUS_FILE:-}" ]]; then
    echo "$CHUNK_UTXO_REHYDRATE_STATUS_FILE"
  elif [[ -n "$BLOCK_CACHE_DIR" ]]; then
    echo "$BLOCK_CACHE_DIR/.chunk_utxo_rehydrate.status"
  else
    echo ""
  fi
}

chunk_utxo_compact_status_path() {
  if [[ -n "${CHUNK_UTXO_COMPACT_STATUS_FILE:-}" ]]; then
    echo "$CHUNK_UTXO_COMPACT_STATUS_FILE"
  elif [[ -n "$BLOCK_CACHE_DIR" ]]; then
    echo "$BLOCK_CACHE_DIR/.chunk_utxo_compact.status"
  else
    echo ""
  fi
}

# True when a compact status file is present and fresh (<10 min old).
disk_utxo_compact_in_progress() {
  [[ "${KERNEL_DIFF_STATUS_MOCK_RUNNING:-0}" == 1 ]] && return 1
  local f bytes_before start_ts now age_s
  f="$(chunk_utxo_compact_status_path)"
  [[ -n "$f" && -f "$f" ]] || return 1
  read -r bytes_before start_ts <"$f" || return 1
  [[ "$bytes_before" =~ ^[0-9]+$ && "$start_ts" =~ ^[0-9]+$ ]] || return 1
  now="$(status_now_s)"
  age_s=$((now - start_ts))
  [[ "$age_s" -lt 0 ]] && age_s=0
  [[ "$age_s" -le 600 ]] || return 1
  return 0
}

# Disk-utxo rehydrate %, `compact` during optional LSM shrink, or `-`.
format_rehydrate_token() {
  local f loaded total cph ts now age_s pct l_m t_m
  # 1. Active rehydration (streaming .bin → RocksDB).
  f="$(chunk_utxo_rehydrate_status_path)"
  if [[ -n "$f" && -f "$f" ]]; then
    read -r loaded total cph ts <"$f" || { echo "-"; return; }
    [[ "$loaded" =~ ^[0-9]+$ && "$total" =~ ^[0-9]+$ && "$total" -gt 0 ]] || { echo "-"; return; }
    now="$(status_now_s)"
    age_s=$((now - ts))
    [[ "$age_s" -lt 0 ]] && age_s=0
    if [[ "$age_s" -gt 600 ]]; then
      echo "?stale"
      return
    fi
    pct="$(awk -v l="$loaded" -v t="$total" 'BEGIN { printf "%.0f", 100 * l / t }')"
    l_m="$(awk -v l="$loaded" 'BEGIN { printf "%.1f", l/1000000 }')"
    t_m="$(awk -v t="$total" 'BEGIN { printf "%.1f", t/1000000 }')"
    echo "${pct}%(${l_m}/${t_m}M)"
    return
  fi
  # 2. Post-rehydration compact (status file written by Rust, deleted on finish).
  if disk_utxo_compact_in_progress; then
    echo "compact"
    return
  fi
  echo "-"
}

# Prints LOADED TOTAL [KPS] from last `[disk-utxo] commit` log line (KPS = batch k rows/s, optional).
parse_last_disk_utxo_commit() {
  local logf="$1" line loaded total kps
  [[ -n "$logf" && -f "$logf" ]] || return 1
  line="$(tail -c 524288 "$logf" 2>/dev/null | grep '\[disk-utxo\] commit' | tail -1)" || true
  [[ -n "$line" ]] || return 1
  loaded="$(sed -n 's/.*total \([0-9]*\)\/\([0-9]*\).*/\1/p' <<<"$line")"
  total="$(sed -n 's/.*total \([0-9]*\)\/\([0-9]*\).*/\2/p' <<<"$line")"
  [[ "$loaded" =~ ^[0-9]+$ && "$total" =~ ^[0-9]+$ ]] || return 1
  kps="$(sed -n 's/.*(\([0-9.]*\) k\/s).*/\1/p' <<<"$line")"
  [[ -n "$kps" ]] || kps=""
  echo "$loaded $total $kps"
}

# True when rehydrate status file says import still in progress (same rules as format_rehydrate_token).
rehydr_import_in_progress() {
  local f loaded total cph ts now age_s
  f="$(chunk_utxo_rehydrate_status_path)"
  [[ -n "$f" && -f "$f" ]] || return 1
  read -r loaded total cph ts <"$f" || return 1
  [[ "$loaded" =~ ^[0-9]+$ && "$total" =~ ^[0-9]+$ && "$total" -gt 0 ]] || return 1
  [[ "$loaded" -lt "$total" ]] || return 1
  now="$(status_now_s)"
  age_s=$((now - ts))
  [[ "$age_s" -lt 0 ]] && age_s=0
  [[ "$age_s" -le 600 ]] || return 1
  return 0
}

# Prints "HEIGHT TS" from status file; fails if missing or malformed.
read_chunk_utxo_status() {
  local f="$1" h t
  [[ -n "$f" && -f "$f" ]] || return 1
  read -r h t <"$f" || return 1
  [[ "$h" =~ ^[0-9]+$ && "$t" =~ ^[0-9]+$ ]] || return 1
  echo "$h $t"
}
DIFF_LOG="${KERNEL_DIFF_LOG:-}"
[[ -z "$DIFF_LOG" && -n "$BLOCK_CACHE_DIR" ]] && DIFF_LOG="$BLOCK_CACHE_DIR/block_kernel_diff.log"
RESUME_JSONL="${RESUME_STATE_PATH:-}"

checkpoint_every_from_ps() {
  [[ "${KERNEL_DIFF_STATUS_MOCK_RUNNING:-0}" == 1 ]] && { echo 25000; return; }
  local pid line
  pid="$(chunk_utxo_pid 2>/dev/null)" || true
  [[ -z "$pid" ]] && { echo 25000; return; }
  line="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
  if [[ "$line" =~ --checkpoint-every[[:space:]]+([0-9]+) ]]; then
    echo "${BASH_REMATCH[1]}"
  else
    echo 25000
  fi
}

highest_utxo_checkpoint() {
  local d="$1" max=0 n
  shopt -s nullglob
  for f in "$d"/utxo_*.bin; do
    [[ -f "$f" ]] || continue
    n="${f##*/utxo_}"; n="${n%.bin}"
    [[ "$n" =~ ^[0-9]+$ ]] || continue
    (( n > max )) && max=$n
  done
  shopt -u nullglob
  echo "$max"
}

max_block_line_height_from_chunk_log() {
  [[ -f "$CHUNK_LOG" ]] || return
  local tail_chunk h_chk h_perf
  # Byte cap (not line count): tail -n 200k on a multi-GB log is slow on USB HDDs.
  # 32MB tail still holds plenty of seek lines + recent [CHKPT] when status file is missing.
  tail_chunk="$(tail -c 33554432 "$CHUNK_LOG" 2>/dev/null)"
  h_chk="$(echo "$tail_chunk" | grep '\[CHKPT\] height ' | tail -1 | awk '{print $NF}')"
  h_perf="$(echo "$tail_chunk" | grep -oE '\[(TIMING|PERF)\] Block [0-9]+:' | tail -1 | grep -oE '[0-9]+' | head -1)"
  checkpoint_height_hint "$h_chk" "$h_perf"
}

last_checkpoint_write_height_from_log() {
  [[ -f "$CHUNK_LOG" ]] || return
  tail -c 8388608 "$CHUNK_LOG" 2>/dev/null \
    | grep -oE 'wrote .*utxo_[0-9]+\.bin' \
    | tail -1 \
    | grep -oE '[0-9]+' \
    | head -1 || true
}

chunk_log_tail_line() {
  [[ -f "$CHUNK_LOG" ]] || return
  tail -n 1 "$CHUNK_LOG" 2>/dev/null | tr -d '\r' | head -c 160
}

# "40.8GB / 55.2GB" -> 74 (percent)
seek_pct_from_line() {
  local line="$1" a b
  a=$(sed -n 's/.*: *\([0-9.]*\)GB.*/\1/p' <<<"$line")
  b=$(sed -n 's/.*\/ *\([0-9.]*\)GB.*/\1/p' <<<"$line")
  [[ -n "$a" && -n "$b" ]] || return
  awk -v x="$a" -v y="$b" 'BEGIN { if (y > 0) printf "%.0f", 100 * x / y }'
}

checkpoint_height_hint() {
  local max=0 n any=0
  for n in "$@"; do
    [[ "$n" =~ ^[0-9]+$ ]] || continue
    any=1
    (( n > max )) && max=$n
  done
  if (( any )); then echo "$max"; else echo ""; fi
}

last_diff_height_from_log() {
  [[ -f "$DIFF_LOG" ]] || return
  local line
  line="$(tail -c 16777216 "$DIFF_LOG" 2>/dev/null | grep '^{' | tail -1 || true)"
  [[ -n "$line" ]] || return
  sed -n 's/.*"height"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' <<<"$line" | head -1
}

next_checkpoint_target() {
  local h="$1" step="$2"
  [[ -n "$h" && "$h" =~ ^[0-9]+$ ]] || { echo ""; return; }
  echo $(( (h / step + 1) * step ))
}

print_status() {
  local ts step cp_dir max_cp h_log h_wrote h next hdiff now bps last_line seek_pct seek_busy
  local dh_note seek_rate dhp poll_f prev_h prev_t prev_pid cur_pid
  ts="$(date '+%Y-%m-%d %H:%M:%S')"
  if [[ -n "${KERNEL_DIFF_STATUS_MOCK_NOW:-}" ]]; then
    now="${KERNEL_DIFF_STATUS_MOCK_NOW}"
  else
    now=$(date +%s)
  fi
  step="$(checkpoint_every_from_ps)"
  max_cp=""

  if [[ "$VERBOSE" == "1" ]]; then
    echo "kernel-diff status  @  $ts  (every ${REFRESH_SEC}s)"
    echo ""
  fi

  if [[ "$MODE" == "all" || "$MODE" == "checkpoints" ]]; then
    if [[ -n "$BLOCK_CACHE_DIR" ]]; then
      cp_dir="$BLOCK_CACHE_DIR/$CHECKPOINT_SUBDIR"
      [[ -d "$cp_dir" ]] && max_cp="$(highest_utxo_checkpoint "$cp_dir")"
    fi
    h_log="$(max_block_line_height_from_chunk_log)"
    h_wrote="$(last_checkpoint_write_height_from_log)"
    # Stopped / no status: ladder hint from log + disk (can be stale vs in-flight work).
    h_fallback="$(checkpoint_height_hint "$h_log" "$h_wrote" "$max_cp")"

    st_file="$(chunk_utxo_status_path)"
    h_status=""
    t_status=""
    if [[ -n "$st_file" ]]; then
      _st="$(read_chunk_utxo_status "$st_file" 2>/dev/null)" || true
      if [[ -n "$_st" ]]; then
        h_status="${_st%% *}"
        t_status="${_st#* }"
      fi
    fi

    if chunk_utxo_running && [[ -n "$h_status" ]]; then
      h="$h_status"
    else
      h="$h_fallback"
    fi

    bps=""
    dh_note=""
    seek_rate=""
    if chunk_utxo_running; then
      cur_pid="$(chunk_utxo_pid 2>/dev/null)" || cur_pid=""
      poll_f="$(bps_poll_state_file)"
      # BPS: Δheight / Δwall_clock since last sample. State is persisted to POLL_STATE file so
      # repeated ` --once` (cron, manual taps) still works; watch-loop also uses the same file.
      if [[ -n "$h_status" && "$h_status" =~ ^[0-9]+$ ]]; then
        prev_h=""; prev_t=""; prev_pid=""; prev_lb=""
        if [[ -n "$poll_f" && -f "$poll_f" ]]; then
          read -r prev_h prev_t prev_pid prev_lb <"$poll_f" || true
          if [[ -n "$prev_lb" && "$prev_lb" =~ ^[0-9]+\.[0-9][0-9]$ ]]; then
            CHK_LAST_BPS="$prev_lb"
          fi
        fi
        if [[ -z "$prev_h" || -z "$prev_t" ]] && [[ -n "$CHK_POLL_H" && -n "$CHK_POLL_T" ]]; then
          prev_h="$CHK_POLL_H"
          prev_t="$CHK_POLL_T"
          prev_pid="$cur_pid"
        fi
        if [[ "$prev_h" =~ ^[0-9]+$ && "$prev_t" =~ ^[0-9]+$ && -n "$cur_pid" ]]; then
          if [[ -z "$prev_pid" || "$prev_pid" == "$cur_pid" ]]; then
            dh_poll=$((h_status - prev_h))
            dt_poll=$((now - prev_t))
            if (( dt_poll > 0 )); then
              if (( dh_poll > 0 )); then
                bps="$(awk -v d="$dh_poll" -v e="$dt_poll" 'BEGIN { printf "%.2f", d / e }')"
                CHK_LAST_BPS="$bps"
              fi
            fi
          fi
        fi
        CHK_POLL_H="$h_status"
        CHK_POLL_T="$now"
        if [[ -n "$poll_f" ]]; then
          echo "$h_status $now $cur_pid ${CHK_LAST_BPS:-}" >"${poll_f}.new" && mv "${poll_f}.new" "$poll_f"
        fi
      elif [[ -n "$h" && "$h" =~ ^[0-9]+$ ]]; then
        # Fallback (old binary): wall-clock deltas between script polls.
        if [[ -z "$CHK_ANCHOR_H" || ! "$CHK_ANCHOR_H" =~ ^[0-9]+$ ]] || (( h < CHK_ANCHOR_H )); then
          CHK_ANCHOR_H="$h"
          CHK_ANCHOR_T="$now"
        elif (( h > CHK_ANCHOR_H )); then
          elapsed=$((now - CHK_ANCHOR_T))
          if (( elapsed > 0 )); then
            delta=$((h - CHK_ANCHOR_H))
            bps="$(awk -v d="$delta" -v e="$elapsed" 'BEGIN { printf "%.2f", d / e }')"
            CHK_LAST_BPS="$bps"
          fi
          CHK_ANCHOR_H="$h"
          CHK_ANCHOR_T="$now"
        fi
      fi
    else
      CHK_ANCHOR_H=""
      CHK_ANCHOR_T=""
      CHK_LAST_BPS=""
      CHK_POLL_H=""
      CHK_POLL_T=""
      _bpf="$(bps_poll_state_file)"
      [[ -n "$_bpf" && -f "$_bpf" ]] && rm -f "$_bpf"
      _rboff="$(rehydr_poll_state_file)"
      [[ -n "$_rboff" && -f "$_rboff" ]] && rm -f "$_rboff"
      CHK_LAST_REHYDR_BPS=""
      CHK_PREV_H=""
      CHK_PREV_SEEK_PCT=""
      CHK_PREV_SEEK_T=""
    fi

    last_line=""
    seek_busy=false
    seek_pct=""
    if [[ -f "$CHUNK_LOG" ]]; then
      last_line="$(chunk_log_tail_line)"
      if [[ "$last_line" == *'Seeking in chunk'* ]]; then
        seek_busy=true
        seek_pct="$(seek_pct_from_line "$last_line")"
        if [[ -n "$seek_pct" && "$seek_pct" =~ ^[0-9]+$ ]] && [[ -n "$CHK_PREV_SEEK_PCT" && "$CHK_PREV_SEEK_PCT" =~ ^[0-9]+$ ]] && [[ -n "$CHK_PREV_SEEK_T" ]]; then
          ds=$((seek_pct - CHK_PREV_SEEK_PCT))
          dt=$((now - CHK_PREV_SEEK_T))
          if (( dt > 0 && ds != 0 )); then
            seek_rate="$(awk -v d="$ds" -v t="$dt" 'BEGIN { printf "%.2f", d / t }')%/s"
          fi
        fi
        if [[ -n "$seek_pct" ]]; then
          CHK_PREV_SEEK_PCT="$seek_pct"
          CHK_PREV_SEEK_T="$now"
        fi
      else
        CHK_PREV_SEEK_PCT=""
        CHK_PREV_SEEK_T=""
      fi
    fi

    if [[ -n "$h" && "$h" =~ ^[0-9]+$ && -n "$CHK_PREV_H" && "$CHK_PREV_H" =~ ^[0-9]+$ ]]; then
      dhp=$((h - CHK_PREV_H))
      if (( dhp > 0 )); then
        dh_note="+${dhp}"
      elif (( dhp < 0 )); then
        dh_note="${dhp}"
      elif chunk_utxo_running; then
        dh_note="·"
      fi
    fi
    [[ -n "$h" && "$h" =~ ^[0-9]+$ ]] && CHK_PREV_H="$h"

    next="$(next_checkpoint_target "$h" "$step")"

    # Compact line: process RSS vs system headroom (GiB, one decimal).
    memout=""
    ak="$(grep -m1 '^MemAvailable:' /proc/meminfo 2>/dev/null | awk '{print $2}')"
    avail_g="?"
    [[ "$ak" =~ ^[0-9]+$ ]] && avail_g="$(awk -v k="$ak" 'BEGIN { printf "%.1f", k/1024/1024 }')"
    if chunk_utxo_running; then
      _mpid="$(chunk_utxo_pid 2>/dev/null)" || _mpid=""
      if [[ -n "$_mpid" && -r "/proc/$_mpid/status" ]]; then
        rk="$(grep -m1 '^VmRSS:' "/proc/$_mpid/status" 2>/dev/null | awk '{print $2}')"
        if [[ "$rk" =~ ^[0-9]+$ ]]; then
          rss_g="$(awk -v k="$rk" 'BEGIN { printf "%.1f", k/1024/1024 }')"
          memout="r${rss_g}/a${avail_g}g"
        else
          memout="r?/a${avail_g}g"
        fi
      else
        memout="r?/a${avail_g}g"
      fi
    else
      memout="-/a${avail_g}g"
    fi

    # UTXO rows/s during disk-utxo rehydr (compact bps=); log commit lines + poll window.
    rehydr_bpout=""
    if ! rehydr_import_in_progress; then
      _rbcf="$(rehydr_poll_state_file)"
      [[ -n "$_rbcf" && -f "$_rbcf" ]] && rm -f "$_rbcf"
      CHK_LAST_REHYDR_BPS=""
    fi
    if rehydr_import_in_progress && chunk_utxo_running; then
      cur_pid_r="$(chunk_utxo_pid 2>/dev/null)" || cur_pid_r=""
      _rhspf="$(rehydr_poll_state_file)"
      r_loaded=""; r_total=""
      _rhfile="$(chunk_utxo_rehydrate_status_path)"
      if [[ -n "$_rhfile" && -f "$_rhfile" ]]; then
        read -r r_loaded r_total _cph _rts <"$_rhfile" || true
      fi
      if [[ "$r_loaded" =~ ^[0-9]+$ && "$r_total" =~ ^[0-9]+$ && "$r_loaded" -lt "$r_total" ]]; then
        cur_l="$r_loaded"
        cur_kps=""
        _pcl=""
        while IFS= read -r _lf; do
          [[ -f "$_lf" ]] || continue
          _pcl="$(parse_last_disk_utxo_commit "$_lf" 2>/dev/null)" && break
        done < <(disk_utxo_log_paths | awk '!seen[$0]++')
        if [[ -n "$_pcl" ]]; then
          read -r pl pt pk <<<"$_pcl"
          if [[ "$pt" == "$r_total" ]]; then
            cur_l="$pl"
            [[ -n "$pk" ]] && cur_kps="$pk"
          fi
        fi
        prev_rl=""; prev_rt=""; prev_rpid=""; prev_rlast=""
        if [[ -n "$_rhspf" && -f "$_rhspf" ]]; then
          read -r prev_rl prev_rt prev_rpid prev_rlast <"$_rhspf" || true
          [[ -n "$prev_rlast" ]] && CHK_LAST_REHYDR_BPS="$prev_rlast"
        fi
        if [[ -n "$prev_rpid" && -n "$cur_pid_r" && "$prev_rpid" != "$cur_pid_r" ]]; then
          prev_rl=""
          CHK_LAST_REHYDR_BPS=""
        fi
        if [[ "$prev_rl" =~ ^[0-9]+$ && "$prev_rt" =~ ^[0-9]+$ && -n "$cur_pid_r" ]]; then
          dt_r=$((now - prev_rt))
          dl_r=$((cur_l - prev_rl))
          if (( dt_r >= 2 )); then
            if (( dl_r > 0 )); then
              rps_r="$(awk -v d="$dl_r" -v t="$dt_r" 'BEGIN { printf "%.2f", d / t }')"
              rehydr_bpout="$(awk -v r="$rps_r" 'BEGIN {
                if (r >= 1000000) printf "%.2fM/s", r/1000000;
                else if (r >= 1000) printf "%.1fk/s", r/1000;
                else printf "%.0f/s", r
              }')"
              CHK_LAST_REHYDR_BPS="$rehydr_bpout"
            fi
          fi
        fi
        if [[ -z "$rehydr_bpout" && -n "$cur_kps" ]]; then
          rehydr_bpout="~${cur_kps}k/s"
          CHK_LAST_REHYDR_BPS="${cur_kps}k/s"
        fi
        if [[ -z "$rehydr_bpout" && -n "$CHK_LAST_REHYDR_BPS" ]]; then
          rehydr_bpout="~$CHK_LAST_REHYDR_BPS"
        fi
        if [[ -n "$_rhspf" ]]; then
          echo "$cur_l $now $cur_pid_r ${CHK_LAST_REHYDR_BPS:-}" >"${_rhspf}.new" && mv "${_rhspf}.new" "$_rhspf"
        fi
      fi
    fi

    if [[ "$VERBOSE" == "1" ]]; then
      echo "Checkpoints (UTXO ladder)"
      echo "  state: $(chunk_utxo_running && echo running || echo stopped)"
      echo "  disk_max: ${max_cp:-?}  h_prog: ${h:-?}${dh_note}  next_file: ${next:-?}  step: $step"
      echo "  status: ${st_file:-—}  bps: ${bps:-—}  last_bps: ${CHK_LAST_BPS:-—}"
      if $seek_busy; then
        echo "  seek: ${seek_pct:-?}%  ${seek_rate:+rate $seek_rate}"
      fi
      _rhvf="$(chunk_utxo_rehydrate_status_path)"
      if [[ -n "$_rhvf" && -f "$_rhvf" ]]; then
        read -r _rvl _rvt _rvcp _rvts <"$_rhvf" 2>/dev/null || true
        if [[ "$_rvl" =~ ^[0-9]+$ && "$_rvt" =~ ^[0-9]+$ && "$_rvt" -gt 0 ]]; then
          _rvpct="$(awk -v l="$_rvl" -v t="$_rvt" 'BEGIN { printf "%.1f", 100 * l / t }')"
          _nowv="$(status_now_s)"
          _rvage=$((_nowv - _rvts))
          [[ "$_rvage" -lt 0 ]] && _rvage=0
          echo "  rehydrate (disk-utxo): $_rvl / $_rvt (${_rvpct}%)  utxo_${_rvcp}.bin  age ${_rvage}s"
          if rehydr_import_in_progress && chunk_utxo_running; then
            echo "  rehydr throughput (UTXO rows/s → compact bps=): ${rehydr_bpout:-${CHK_LAST_REHYDR_BPS:--}}"
          fi
        fi
      fi
      [[ -n "$last_line" ]] && echo "  tail: $last_line"
      echo ""
    else
      # Compact: one line, data only
      local st cpd hout bpout skout rh_tok
      rh_tok="$(format_rehydrate_token)"
      st="off"
      chunk_utxo_running && st="on"
      cpd="${max_cp:-?}"
      hout="${h:-?}"
      [[ -n "$dh_note" ]] && hout="${hout}${dh_note}"
      if rehydr_import_in_progress && chunk_utxo_running; then
        if [[ -n "$rehydr_bpout" ]]; then
          bpout="$rehydr_bpout"
        elif [[ -n "$CHK_LAST_REHYDR_BPS" ]]; then
          bpout="~$CHK_LAST_REHYDR_BPS"
        else
          bpout="-"
        fi
      elif chunk_utxo_running && disk_utxo_compact_in_progress; then
        bpout="compact"
      elif [[ -n "$bps" ]]; then
        bpout="$bps"
      elif [[ -n "$CHK_LAST_BPS" ]]; then
        bpout="~$CHK_LAST_BPS"
      else
        bpout="-"
      fi
      if $seek_busy; then
        skout="${seek_pct:-?}%"
        [[ -n "$seek_rate" ]] && skout="${skout}@${seek_rate}"
        # During seek, block BPS is 0 but the process is alive — show last connect BPS as hint, not n/a.
        if [[ "$bpout" == "-" && -n "$CHK_LAST_BPS" ]]; then
          bpout="~$CHK_LAST_BPS"
        fi
      else
        skout="-"
      fi
      if [[ "$MODE" == "all" ]]; then
        hdiff="$(last_diff_height_from_log)"
        dst="idle"
        pgrep -f '[b]lock_kernel_diff' >/dev/null 2>&1 && dst="on"
        echo "$ts  chkpt run=$st disk=$cpd h=$hout next=${next:-?} step=$step bps=$bpout seek=$skout rehydr=$rh_tok mem=$memout  |  kdiff=$dst last_h=${hdiff:--}"
      else
        echo "$ts  chkpt run=$st disk=$cpd h=$hout next=${next:-?} step=$step bps=$bpout seek=$skout rehydr=$rh_tok mem=$memout"
      fi
    fi
  fi

  if [[ "$MODE" == "differential" && "$VERBOSE" != "1" ]]; then
    hdiff="$(last_diff_height_from_log)"
    dst="idle"
    pgrep -f '[b]lock_kernel_diff' >/dev/null 2>&1 && dst="on"
    echo "$ts  kdiff=$dst last_h=${hdiff:--}"
  fi

  if [[ "$MODE" == "all" || "$MODE" == "differential" ]] && [[ "$VERBOSE" == "1" ]]; then
    hdiff="$(last_diff_height_from_log)"
    echo "Differential (BLVM vs libbitcoinkernel)"
    echo "  state: $(pgrep -f '[b]lock_kernel_diff' >/dev/null && echo running || echo stopped)"
    echo "  last_h: ${hdiff:--}"
    echo ""
  fi
}

trap 'echo ""; exit 0' INT TERM

if $ONCE; then
  print_status
  exit 0
fi

while true; do
  clear
  print_status
  sleep "$REFRESH_SEC"
done
