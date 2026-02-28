# Auto-Heal Skill

## Plugin Tools Used

| Step | Tool | Purpose |
|------|------|---------|
| Step 1 | `gf_instance_health({ instanceId, live: true })` | Confirm health score |
| Step 2 | `gf_bulk_restart({ instanceIds: [id], service: "openclaw" })` | Docker restart |
| Step 3 | `gf_instance_health({ instanceId, live: true })` | Verify heal |
| Step 4 | `gf_pair_status({ accountId })` | Verify PRIMARY/STANDBY roles |
| Step 5 | `gf_audit_log` (write) + failover call | Log intent then trigger failover |
| All steps | `gf_audit_log` (write) | Every action must be logged |

This skill defines the exact auto-heal decision tree Guardian follows. Do not improvise or skip steps.

## Auto-Heal Decision Tree

When Guardian detects an instance with health_score < 50 or missing heartbeat > 3min:

**Step 1 — Verify:** Call `gf_instance_health({ instanceId, live: true })` on the instance. If health_score recovers > 70, log and continue. No further action needed.

**Step 2 — Docker restart:** SSH to instance via Tailscale. Run: `docker compose restart openclaw`. Log to audit trail BEFORE executing.

**Step 3 — Wait 90s.** Call `gf_instance_health({ instanceId, live: true })`. If HTTP 200 received and health_score >= 70, log HEALED and notify Commander.

**Step 4 — If still failing:** check if this is the PRIMARY of a pair. Call `gf_pair_status({ accountId })`. Verify the instance role. If PRIMARY, verify STANDBY is ACTIVE. Do not skip this step.

**Step 5 — If STANDBY ACTIVE:** trigger failover. Notify Commander immediately.

**Step 6 — If STANDBY NOT ACTIVE:** CRITICAL — notify Commander immediately. Do not act alone. Wait for instructions.

---

**NEVER:** delete a VPS.
**NEVER:** touch another user's instance.
**NEVER:** skip Step 4 verification.
**NEVER:** retry more than 3 times without escalating.

---

## Health Score Thresholds Reference

| Score | Status | Action |
|-------|--------|--------|
| ≥ 70 | Healthy | No action needed |
| 50–69 | Degraded | Monitor; auto-heal if dropping |
| < 50 | Critical | Execute auto-heal sequence immediately |
| Missing heartbeat > 3min | Assume Critical | Execute auto-heal sequence |

## What "Docker Restart" Means

The specific command executed on the instance:
```bash
docker compose restart openclaw
```

This gracefully restarts only the OpenClaw container. It does NOT:
- Restart the OS
- Restart other containers (nginx, Tailscale, etc.)
- Modify any config

If the docker restart fails (SSH timeout, Docker daemon down, etc.):
- Log the failure
- Attempt Step 3 anyway (direct HTTP health check)
- If Step 3 also fails, proceed to Step 4

## What "HEALED" Means

An instance is HEALED when:
- health_score >= 70 (checked via live gf_instance_health call)
- openclawStatus == "HEALTHY"
- openclawHttpStatus == 200

All three conditions must be true. A partial recovery (docker running but HTTP failing) is NOT healed — continue to Step 4.

## Failover Trigger Protocol

When triggering failover (Step 5):
1. Log intent to gf-audit before calling failover
2. Verify standby health_score >= 70 (one final check)
3. Call failover via GatewayForge API (gf-failover crate)
4. Notify Commander via sessions_send within 60 seconds:
   ```json
   { "type": "failover_triggered", "failed_instance": "...", "new_primary": "...", "user_impact_seconds": N }
   ```
5. Queue reprovision of new standby (do not leave account without a standby)

## Escalation to Commander

When escalating (Step 6), send:
```json
{
  "type": "critical_escalation",
  "priority": "immediate",
  "instance_id": "...",
  "account_id": "...",
  "situation": "PRIMARY_FAILING_NO_STANDBY",
  "health_score": N,
  "standby_status": "...",
  "heal_attempts": N,
  "waiting_for": "commander_instructions"
}
```

Do not take any further action until Commander responds.
