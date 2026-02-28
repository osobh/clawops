# Failover Orchestration Skill

## Plugin Tools Used

| Tool | When |
|------|------|
| `gf_pair_status({ accountId })` | Verify PRIMARY/STANDBY states before every failover |
| `gf_instance_health({ instanceId, live: true })` | Verify standby score >= 70 before promoting |
| `gf_audit_log` (write) | Log intent BEFORE calling failover API |
| `gf_provision` | Queue new standby after failover completes |

### Failover Call Sequence

```
1. gf_pair_status({ accountId }) → confirm PRIMARY failing, STANDBY role/state
2. gf_instance_health({ instanceId: standbyId, live: true }) → confirm score >= 70
3. gf_audit_log write: { action: "failover_intent", failingPrimary, newPrimary: standbyId }
4. Call failover API → promote standby to PRIMARY
5. gf_pair_status({ accountId }) → confirm new PRIMARY is ACTIVE
6. Notify Commander via sessions_send within 60 seconds
7. Queue reprovision of new standby via gf_provision
```

## Failover Overview

Failover promotes a STANDBY instance to PRIMARY when the PRIMARY fails. The user must always have exactly one ACTIVE gateway. Failover must appear atomic from the user's perspective — their requests should route to the new primary within seconds.

## Failover Trigger Conditions

Failover is triggered when ALL of the following are true:
1. Primary instance health_score < 50 (Critical)
2. Docker restart (Step 2 of auto-heal) did not recover it
3. HTTP check (Step 3 of auto-heal) failed after 90s wait
4. Standby instance health_score >= 70 (ACTIVE)

If condition 4 is NOT met (standby also failing): escalate to Commander — DO NOT trigger failover.

## Failover Sequence

```
Step 1 — Final standby verification (never skip)
  → Call gf_pair_status({ accountId })
  → Confirm standby health_score >= 70
  → Confirm standby status == "ACTIVE"
  → If either fails: ABORT and escalate to Commander

Step 2 — Log to audit trail
  → Log BEFORE any API calls
  → Include: reason, affected_instance, standby_instance, triggered_by

Step 3 — Update pair status in GatewayForge DB
  → Mark failed instance: status = FAILED
  → Mark standby instance: role = PRIMARY, status = ACTIVE
  → This is the atomic step — routing follows from DB state

Step 4 — Update user routing
  → GatewayForge updates DNS/routing to point user subdomain at new primary
  → This takes ~5–30 seconds depending on DNS TTL

Step 5 — Confirm routing updated
  → Verify user can reach new primary (HTTP health check via public endpoint)

Step 6 — Notify Commander within 60 seconds
  { "type": "failover_complete", "failed_instance": "...", "new_primary": "...", "duration_ms": N }

Step 7 — Queue standby reprovision
  → Account now has PRIMARY but no STANDBY — not resilient
  → Add to reprovision queue immediately
  → Use same tier, preferably same region as original primary (if provider recovered)
```

## Bulk Failover (Provider Outage)

When a provider region has an outage affecting many instances:

1. Get all primaries in affected region from gf_fleet_status
2. For each: verify their standby is on a DIFFERENT provider (should be by design)
3. Trigger failovers in batches of 20 (parallel)
4. Report progress to Commander every 10 completions
5. Accounts where standby is ALSO on the affected provider: escalate to Commander (design flaw)
6. After all failovers: notify Commander with full summary

**Provider outage declaration criteria:**
- > 5 instances unreachable in same region within 5 minutes
- Provider status page confirms incident
- SSH timeouts (not application errors) as failure mode

## Post-Failover Actions

Within 1 hour of failover:
- Queue reprovision of new standby for all affected accounts
- Update monitoring: suppress alerts for failed primary (it's intentionally down)
- Commander briefs operator on blast radius and ETA to full recovery

Within 24 hours:
- Verify all affected accounts have new standbys ACTIVE
- Update provider performance scores (failover events affect score)
- Triage documents incident if > 10 accounts affected

## What NOT to Do During Failover

- NEVER delete the failed PRIMARY during failover (might recover)
- NEVER failover when standby health < 70 — you'd just fail twice
- NEVER do primary and standby failover simultaneously (no gateway to serve user)
- NEVER trigger more than 50 concurrent failovers (can overwhelm routing updates)
- NEVER proceed without audit log entry — provider API deletes need a paper trail
