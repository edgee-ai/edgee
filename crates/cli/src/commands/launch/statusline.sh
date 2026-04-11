#!/usr/bin/env bash
set -euo pipefail

# Read stdin (Claude Code passes session JSON)
INPUT=$(cat)

# Env vars set by "edgee launch claude"
SESSION_ID="${EDGEE_SESSION_ID:-}"
API_URL="${EDGEE_CONSOLE_API_URL:-https://api.edgee.app}"

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

# ── cache ────────────────────────────────────────────────────────────
CACHE_DIR="${HOME}/.config/edgee/cache"
mkdir -p "$CACHE_DIR"
CACHE_FILE="${CACHE_DIR}/statusline-${SESSION_ID}.json"
CACHE_MAX_AGE=8

USE_CACHE=false
if [ -f "$CACHE_FILE" ]; then
    if [ "$(uname)" = "Darwin" ]; then
        FILE_AGE=$(( $(date +%s) - $(stat -f %m "$CACHE_FILE") ))
    else
        FILE_AGE=$(( $(date +%s) - $(stat -c %Y "$CACHE_FILE") ))
    fi
    [ "$FILE_AGE" -lt "$CACHE_MAX_AGE" ] && USE_CACHE=true
fi

if [ "$USE_CACHE" = true ]; then
    STATS=$(cat "$CACHE_FILE")
else
    STATS=$(curl -sf --max-time 5 \
        "${API_URL}/v1/sessions/${SESSION_ID}/summary" 2>/dev/null) || STATS=""
    if [ -n "$STATS" ]; then
        echo "$STATS" > "$CACHE_FILE"
    elif [ -f "$CACHE_FILE" ]; then
        STATS=$(cat "$CACHE_FILE")
    fi
fi

# ── render ───────────────────────────────────────────────────────────
SEP=""
[ -n "${EDGEE_HAS_EXISTING_STATUSLINE:-}" ] && SEP="| "

if [ -z "$STATS" ] || ! command -v jq &>/dev/null; then
    echo -e "${SEP}\033[38;5;128m三 Edgee\033[0m"
    exit 0
fi

BEFORE=$(echo "$STATS" | jq -r '.total_uncompressed_tools_tokens // 0')
AFTER=$(echo "$STATS"  | jq -r '.total_compressed_tools_tokens // 0')
REQUESTS=$(echo "$STATS" | jq -r '.total_requests // 0')

PURPLE='\033[38;5;128m'
BOLD_PURPLE='\033[1;38;5;128m'
DIM='\033[2m'
RESET='\033[0m'

if [ "$BEFORE" -gt 0 ] && [ "$AFTER" -lt "$BEFORE" ]; then
    PCT=$(( (BEFORE - AFTER) * 100 / BEFORE ))
    FILLED=$(( PCT * 10 / 100 ))
    BAR=""
    for ((i=0; i<FILLED; i++)); do BAR+="█"; done
    for ((i=FILLED; i<10; i++)); do BAR+="░"; done

    echo -e "${SEP}${PURPLE}三 Edgee${RESET}  ${PURPLE}${BAR}${RESET} ${BOLD_PURPLE}${PCT}%${RESET} tool compression  ${DIM}${REQUESTS} reqs${RESET}"
else
    if [ "$REQUESTS" -gt 0 ]; then
        echo -e "${SEP}${PURPLE}三 Edgee${RESET}  ${DIM}${REQUESTS} reqs${RESET}"
    else
        echo -e "${SEP}${PURPLE}三 Edgee${RESET}"
    fi
fi
