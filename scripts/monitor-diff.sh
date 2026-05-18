#!/usr/bin/env bash
# monitor-diff.sh — live view of a running block_kernel_diff
#
# Usage:
#   ./scripts/monitor-diff.sh [log_dir]
#
# log_dir defaults to /tmp/blvm-diff (matches the run wrapper below).
# The script polls every 2 seconds and shows:
#   - current height / end height / % done
#   - blocks/sec (instantaneous + cumulative)
#   - divergence count and first divergence
#   - last few divergence lines (if any)
#   - RSS memory of the running process
#   - last progress line from the run log

set -euo pipefail

LOG_DIR="${1:-/tmp/blvm-diff}"
PROGRESS_LOG="$LOG_DIR/run.log"
DIV_LOG="$LOG_DIR/divergences.jsonl"
JSONL_LOG="$LOG_DIR/all.jsonl"

clear_screen() { printf '\033[2J\033[H'; }

last_height=0
last_time=0

while true; do
    clear_screen

    # ── process status ───────────────────────────────────────────────────────
    DIFFPID=$(pgrep -f "block_kernel_diff" | head -1 || true)
    if [[ -z "$DIFFPID" ]]; then
        echo "═══════════════════════════════════════════════════"
        echo "  block_kernel_diff — NOT RUNNING"
        echo "═══════════════════════════════════════════════════"
        if [[ -f "$PROGRESS_LOG" ]]; then
            echo ""
            echo "Last summary:"
            grep "KERNEL_DIFF_SUMMARY\|KERNEL_DIFF_STATUS" "$PROGRESS_LOG" | tail -3
        fi
    else
        VMRSS=$(awk '/VmRSS/{printf "%.0f MB", $2/1024}' /proc/$DIFFPID/status 2>/dev/null || echo "?")
        VMPEAK=$(awk '/VmPeak/{printf "%.0f MB", $2/1024}' /proc/$DIFFPID/status 2>/dev/null || echo "?")

        echo "═══════════════════════════════════════════════════"
        printf "  block_kernel_diff  PID=%-8s  RSS=%-8s Peak=%s\n" "$DIFFPID" "$VMRSS" "$VMPEAK"
        echo "═══════════════════════════════════════════════════"
    fi

    echo ""

    # ── parse last PROGRESS line ─────────────────────────────────────────────
    if [[ -f "$PROGRESS_LOG" ]]; then
        LAST_PROG=$(grep "KERNEL_DIFF_PROGRESS" "$PROGRESS_LOG" | tail -1)
        if [[ -n "$LAST_PROG" ]]; then
            HEIGHT=$(echo "$LAST_PROG" | grep -oP 'height=\K[0-9]+' || echo "?")
            COMPARED=$(echo "$LAST_PROG" | grep -oP 'compared=\K[0-9]+' || echo "?")
            END=$(echo "$LAST_PROG" | grep -oP 'compared=[0-9]+/\K[0-9?]+' || echo "?")
            DIVS=$(echo "$LAST_PROG" | grep -oP 'divergences=\K[0-9]+' || echo "0")
            BPS=$(echo "$LAST_PROG" | grep -oP 'bps=\K[0-9.]+' || echo "?")
            AVG=$(echo "$LAST_PROG" | grep -oP 'avg=\K[0-9.]+' || echo "?")
            ELAPSED=$(echo "$LAST_PROG" | grep -oP 'elapsed=\K[0-9.]+' || echo "?")

            if [[ "$END" =~ ^[0-9]+$ ]] && [[ "$COMPARED" =~ ^[0-9]+$ ]]; then
                PCT=$(awk "BEGIN{printf \"%.1f\", $COMPARED/$END*100}" 2>/dev/null || echo "?")
                ETA_S=$(awk "BEGIN{r=$BPS+0; if(r>0){printf \"%.0f\", ($END-$COMPARED)/r}else{print \"?\"}}" 2>/dev/null || echo "?")
                if [[ "$ETA_S" =~ ^[0-9]+$ ]]; then
                    ETA_M=$(( ETA_S / 60 ))
                    ETA_H=$(( ETA_M / 60 ))
                    ETA_DISP="${ETA_H}h$((ETA_M%60))m"
                else
                    ETA_DISP="?"
                fi
            else
                PCT="?"
                ETA_DISP="?"
            fi

            printf "  Height    : %-10s / %s  (%s%%)\n" "$HEIGHT" "$END" "$PCT"
            printf "  Compared  : %-10s\n" "$COMPARED"
            printf "  BPS       : %-8s  (avg %s)\n" "$BPS" "$AVG"
            printf "  Elapsed   : %ss  ETA %s\n" "$ELAPSED" "$ETA_DISP"
            printf "  Divergences: %s\n" "$DIVS"
        else
            LAST_RUN=$(grep "KERNEL_DIFF_RUN" "$PROGRESS_LOG" | tail -1 || true)
            if [[ -n "$LAST_RUN" ]]; then
                echo "  Run started: $LAST_RUN"
                echo "  (no progress lines yet)"
            else
                echo "  (no progress data yet)"
            fi
        fi
    else
        echo "  Log not found: $PROGRESS_LOG"
    fi

    echo ""

    # ── divergences ──────────────────────────────────────────────────────────
    if [[ -f "$DIV_LOG" ]]; then
        DIV_COUNT=$(wc -l < "$DIV_LOG")
        if (( DIV_COUNT > 0 )); then
            echo "─── DIVERGENCES ($DIV_COUNT) ─────────────────────────────"
            tail -5 "$DIV_LOG" | while read -r line; do
                H=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('height','?'))" 2>/dev/null || echo "?")
                BH=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('block_hash','?')[:16])" 2>/dev/null || echo "?")
                BLVM=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('blvm','?'))" 2>/dev/null || echo "?")
                CORE=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('core','?'))" 2>/dev/null || echo "?")
                BD=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('blvm_detail','')[:60])" 2>/dev/null || echo "")
                printf "  height=%-8s hash=%s  blvm=%-8s core=%-8s\n" "$H" "$BH" "$BLVM" "$CORE"
                if [[ -n "$BD" ]]; then printf "         blvm_detail: %s\n" "$BD"; fi
            done
        else
            echo "─── DIVERGENCES: none so far ──────────────────────"
        fi
    else
        echo "─── DIVERGENCES: log not created yet ──────────────"
    fi

    echo ""

    # ── jsonl stats ──────────────────────────────────────────────────────────
    if [[ -f "$JSONL_LOG" ]]; then
        JSONL_LINES=$(wc -l < "$JSONL_LOG")
        printf "  JSONL log : %s lines  (%s)\n" "$JSONL_LINES" "$JSONL_LOG"
    fi

    echo ""
    printf "  [refreshes every 2s — Ctrl-C to quit]  %s\n" "$(date '+%H:%M:%S')"

    sleep 2
done
