# Instance Diagnostics Skill

## Plugin Tools Used

| Tool | When |
|------|------|
| `gf_instance_health({ instanceId, live: true })` | Primary diagnostic tool — always call first |
| `gf_pair_status({ accountId })` | Understand pair context (PRIMARY vs STANDBY) |
| `gf_audit_log({ resource: instanceId })` | Recent actions on this instance |
| `gf_provider_health({})` | Rule out provider-wide issues |

### Standard Diagnostic Sequence

```
1. gf_instance_health({ instanceId, live: true })
   → Check: health_score, openclawStatus, dockerRunning, tailscaleConnected
2. gf_pair_status({ accountId })
   → Confirm role (PRIMARY/STANDBY) and pair health
3. gf_audit_log({ resource: instanceId, limit: 20 })
   → Any recent config pushes, restarts, or failed actions?
4. If provider suspected: gf_provider_health({})
   → Rule out infrastructure-level issues
```

## Diagnostic Decision Guide

Use this skill when an instance is degraded and you need to identify root cause before deciding on remediation.

## Common Failure Patterns and Diagnosis

### Pattern 1: OpenClaw Down, Docker Running

**Symptoms:**
- `openclawStatus == "DOWN"` or `openclawHttpStatus != 200`
- `dockerRunning == true`
- Container shows state `exited` or `restarting`

**Diagnosis steps:**
1. `openclaw.logs` — check for OOM kills, config errors, model API failures
2. `docker.ps` — verify container state and restart count
3. If restart_count > 5: container is crash-looping — check logs for root cause

**Remediation:**
- Container crash-looping with config error → `config.rollback`
- Container crash-looping with OOM → tier upgrade may be needed; notify Ledger
- Container exited unexpectedly → `openclaw.restart`

---

### Pattern 2: Docker Down

**Symptoms:**
- `dockerRunning == false`
- `healthScore < 30`
- SSH still reachable (Tailscale connected)

**Diagnosis steps:**
1. SSH to instance: `systemctl status docker`
2. Check if OOM killer triggered: `dmesg | tail -50`
3. Check disk usage: `df -h`

**Remediation:**
- Docker daemon not running → `sudo systemctl start docker` → `openclaw.start`
- OOM killed: check `memUsagePct` trend; if consistently > 90%, flag for Ledger (tier upgrade)
- Disk full: disk alert should have fired first; check what's consuming space

---

### Pattern 3: High Latency / Error Rate (Degraded, Not Down)

**Symptoms:**
- `openclawStatus == "DEGRADED"`
- `healthScore` 50–69
- HTTP 200 but `openclaw.metrics` shows high latency or error rate

**Diagnosis steps:**
1. Check CPU: if `cpuUsage1m > 90%` for > 5 minutes → instance is overloaded
2. Check memory: if `memUsagePct > 90%` → potential OOM pressure
3. Check model API: look for `429` or `503` in openclaw.logs (upstream provider rate limiting)

**Remediation:**
- CPU overload: check if it's a temporary spike (wait 5 min) or persistent (Ledger: tier resize)
- Model API errors: transient, usually self-resolves; monitor for 10 minutes
- Memory pressure: flag for Ledger if persistent

---

### Pattern 4: Tailscale Disconnected

**Symptoms:**
- `tailscaleConnected == false`
- `tailscaleLatencyMs == null`
- Instance may still have public IP access

**Diagnosis steps:**
1. Cannot SSH via Tailscale — check GatewayForge control API for public IP
2. If public IP available: SSH via public IP, run `tailscale status`
3. Check Tailscale auth expiry: `tailscale status | grep Expires`

**Remediation:**
- Auth expired: `tailscale up --authkey <fresh-key>` (requires key rotation in GatewayForge)
- Tailscale daemon crashed: `sudo systemctl restart tailscaled`
- Network issue at provider: check provider status page

---

### Pattern 5: Disk Usage Critical

**Symptoms:**
- `diskUsagePct > 90%`
- May cause OpenClaw to fail writes (conversation history, logs)

**Diagnosis steps:**
1. `docker.logs` — are logs filling disk?
2. Check `/var/lib/docker` size — unused images accumulate
3. Check OpenClaw memory dir size

**Remediation:**
- Log bloat: `docker system prune -f` (removes unused images, stopped containers)
- Memory dir bloat: flag for operator review — user data, don't auto-delete
- Persistent growth: flag for Ledger (tier upgrade to larger disk)

## Diagnostic Tool Reference

| Command | What it tells you |
|---------|------------------|
| `gf_instance_health({ live: true })` | Full health snapshot |
| `openclaw.health` | OpenClaw app-level status |
| `openclaw.logs` | Recent container log lines |
| `docker.ps` | All container states + restart counts |
| `vps.metrics` | Full CPU/RAM/disk/network time series |
| `tailscale.status` | Tailscale connection + auth state |
| `firewall.status` | UFW/iptables — check nothing is blocking ports |
