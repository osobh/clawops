# Incident Response Skill

## Incident Response Philosophy

Speed is critical, but accuracy is more important than speed. A wrong root cause leads to wrong remediation. Take 2 extra minutes to verify before acting.

The incident response sequence: Detect → Contain → Investigate → Remediate → Recover → Document.

## Severity Classification

| Severity | Definition | Response SLA |
|----------|-----------|-------------|
| SEV1 | Any users without active gateway; data loss risk | 60 seconds |
| SEV2 | > 50 degraded; provider region outage; failover rate > 10% | 5 minutes |
| SEV3 | 10–50 degraded; isolated provider issue; Guardian handling | 15 minutes |
| SEV4 | < 10 degraded; Guardian auto-healing; no user impact | 60 minutes |

## SEV1 Response (Immediate)

```
1. Alert operator immediately (don't wait for full investigation)
2. Tell Guardian: emergency mode — prioritize failovers over heal attempts
3. Spawn Triage with full context
4. Every 5 minutes: update operator on recovering account count
5. When blast radius under control: detailed Triage report
```

## SEV2 Response

```
1. Spawn Triage with incident context
2. Query Guardian: how many failovers complete vs in progress?
3. Silence affected-region alerts (30 minutes — prevents noise)
4. Operator notification with initial assessment within 5 minutes
5. Full report from Triage within 15 minutes
```

## Provider Outage Protocol

When > 5 instances fail in same provider/region within 5 minutes:

1. Check provider status page (Hetzner: status.hetzner.com, Vultr: status.vultr.com, etc.)
2. If provider confirms incident: declare provider outage, not individual instance failures
3. Initiate bulk failover for all primaries in affected region
4. Verify each failover lands on a DIFFERENT provider (should be by design)
5. Silence individual instance alerts for affected region
6. Set auto-reprovision to fire when provider recovers
7. Update operator: "[Provider] [Region] outage. N accounts failover-complete. ETA: per provider status."

## Operator Communication During Incident

**First message (within 60s of SEV1/2 detection):**
```
[CMD] INCIDENT: [Brief description]. Investigating now.
```

**Progress update (every 5 min for SEV1, 15 min for SEV2):**
```
[CMD] Update: X/Y accounts failover complete. Z still in progress. ETA: N min.
[Guardian handling automatically. Triage report incoming.]
```

**Resolution:**
```
[CMD] RESOLVED. [Summary of what happened, what was done, current state.]
[N users impacted for X minutes. Full Triage report available.]
```

## Incident Documentation Requirements

Every SEV1 and SEV2 must have a `gf_incident_report` record with:
- Timeline (automated via audit log + Triage reconstruction)
- Root cause (Triage's determination)
- Blast radius (accounts affected, minutes of degradation)
- Actions taken (auto-heal steps, failovers, manual interventions)
- Action items (what to prevent recurrence)

## Post-Incident Review Triggers

Conduct a post-incident review when:
- Any SEV1 (always)
- Any SEV2 affecting > 100 accounts
- Any incident where auto-heal/failover did NOT resolve the situation
- Any incident caused by our own action (config push, provision, teardown)

Briefer maintains incident history for pattern detection. If the same provider has > 3 incidents in 30 days, Ledger should evaluate whether to deprioritize it as a primary choice.
