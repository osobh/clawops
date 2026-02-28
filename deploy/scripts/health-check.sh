#!/usr/bin/env bash
# GatewayForge gf-clawnode health check
# Returns 0 if healthy, 1 if unhealthy
#
# Usage:
#   ./health-check.sh                    # check local instance
#   ./health-check.sh --verbose          # verbose output
#   ./health-check.sh --json             # JSON output for monitoring systems
#
# Used by: systemd watchdog, monitoring scripts, Docker HEALTHCHECK

set -euo pipefail

HEALTH_PORT="${HEALTH_PORT:-8090}"
HEALTH_ENDPOINT="http://127.0.0.1:${HEALTH_PORT}/health"
TIMEOUT_SECS=5
VERBOSE=false
JSON_OUTPUT=false

# ─── Args ─────────────────────────────────────────────────────────────────────

for arg in "$@"; do
    case "$arg" in
        --verbose|-v) VERBOSE=true ;;
        --json)       JSON_OUTPUT=true ;;
        --help|-h)
            echo "Usage: $0 [--verbose] [--json]"
            echo "Environment: HEALTH_PORT (default: 8090)"
            exit 0
            ;;
    esac
done

# ─── Checks ───────────────────────────────────────────────────────────────────

check_systemd_service() {
    if ! command -v systemctl &>/dev/null; then
        return 0  # Not a systemd system — skip
    fi
    systemctl is-active --quiet clawops-node 2>/dev/null
}

check_process_running() {
    pgrep -x gf-clawnode &>/dev/null
}

check_http_health() {
    local http_code
    http_code=$(curl -sf \
        --connect-timeout "$TIMEOUT_SECS" \
        --max-time "$TIMEOUT_SECS" \
        -o /dev/null \
        -w '%{http_code}' \
        "$HEALTH_ENDPOINT" 2>/dev/null) || return 1
    [[ "$http_code" == "200" ]]
}

check_heartbeat_recent() {
    # Check if heartbeat was sent recently (within 2x the interval = 60s)
    local heartbeat_file="/var/lib/gf-clawnode/last_heartbeat"
    if [[ ! -f "$heartbeat_file" ]]; then
        return 0  # No file yet — can't check
    fi
    local mtime
    mtime=$(stat -c '%Y' "$heartbeat_file" 2>/dev/null) || return 0
    local now
    now=$(date +%s)
    local age=$(( now - mtime ))
    [[ "$age" -lt 60 ]]
}

# ─── Main ─────────────────────────────────────────────────────────────────────

declare -A results
declare -i overall=0

# Check 1: systemd service active
if check_systemd_service; then
    results[systemd]="ok"
else
    results[systemd]="failed"
    overall=1
fi

# Check 2: process running
if check_process_running; then
    results[process]="ok"
else
    results[process]="not_running"
    overall=1
fi

# Check 3: HTTP health endpoint (optional — may not be exposed on all configs)
if check_http_health; then
    results[http]="ok"
else
    results[http]="unavailable"
    # HTTP unavailable is a warning, not always fatal
    # (binary communicates via WebSocket, not HTTP in all deployments)
fi

# Check 4: recent heartbeat
if check_heartbeat_recent; then
    results[heartbeat]="recent"
else
    results[heartbeat]="stale_or_missing"
fi

# ─── Output ───────────────────────────────────────────────────────────────────

if [[ "$JSON_OUTPUT" == "true" ]]; then
    status="healthy"
    [[ $overall -ne 0 ]] && status="unhealthy"
    printf '{"status":"%s","checks":{"systemd":"%s","process":"%s","http":"%s","heartbeat":"%s"},"timestamp":"%s"}\n' \
        "$status" \
        "${results[systemd]}" \
        "${results[process]}" \
        "${results[http]}" \
        "${results[heartbeat]}" \
        "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
elif [[ "$VERBOSE" == "true" ]]; then
    echo "=== gf-clawnode health check ==="
    echo "systemd:   ${results[systemd]}"
    echo "process:   ${results[process]}"
    echo "http:      ${results[http]}"
    echo "heartbeat: ${results[heartbeat]}"
    if [[ $overall -eq 0 ]]; then
        echo "status: HEALTHY"
    else
        echo "status: UNHEALTHY"
    fi
fi

exit $overall
