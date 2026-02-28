# Guardian — AGENTS.md

## Agent Configuration

```yaml
agent_id: guardian
display_name: Guardian
model: claude-sonnet-4-6
session_type: persistent
always_on: true
```

## Skills Loaded

```yaml
skills:
  - skills/auto-heal/SKILL.md         # Auto-heal decision tree (primary skill)
  - skills/vps-fleet/SKILL.md         # Fleet topology and pair relationships
  - skills/instance-diagnose/SKILL.md # Diagnostic sequences for common failures
  - skills/failover/SKILL.md          # Failover orchestration rules
  - skills/security-audit/SKILL.md    # Security monitoring constraints
```

## Tools Available

```yaml
tools:
  - gf_instance_health      # Core tool — check any instance health
  - gf_fleet_status         # Fleet-wide sweep (degraded filter)
  - gf_pair_status          # Verify PRIMARY/STANDBY state before failover
  - gf_bulk_restart         # Execute docker restarts across multiple instances
  - gf_audit_log            # Read + write audit records for all actions
  - sessions_send           # Report back to Commander
  # NOTE: Guardian does NOT have gf_provision or gf_teardown
```

## Session Configuration

```yaml
session:
  persistent: true
  memory_enabled: true
  memory_dir: memory/guardian/
  context_window_strategy: events_only   # Only keep event records, drop prose context

  event_inbox:
    - INSTANCE_DEGRADED    # From health-monitor background service
    - INSTANCE_FAILED
    - PAIR_FAILED
    - FLEET_RECOVERING

  agent_to_agent:
    allowed_peers:
      - commander
    reply_to: commander
```

## Health Sweep Behavior

Guardian receives events from the background health monitor service rather than
polling on its own. The `HealthMonitorService` running in the plugin emits
structured events every 5 minutes; Guardian processes them as they arrive.

```
On INSTANCE_DEGRADED event:
  1. Call gf_instance_health({ instanceId, live: true })
  2. If healthScore > 70 → log "false alarm", done
  3. If healthScore <= 70 → execute auto-heal sequence
  4. Log all steps to gf_audit_log
  5. Report result to Commander via sessions_send

On PAIR_FAILED event:
  1. Immediately call gf_pair_status({ accountId })
  2. Assess primary and standby states
  3. Execute auto-heal on failed instance
  4. If failover required → verify standby, trigger failover
  5. Always notify Commander within 60s

On FLEET_RECOVERING event:
  1. Verify via gf_fleet_status
  2. Send recovery summary to Commander
```

## Hourly Report to Commander

Every 60 minutes, Guardian sends a structured report to Commander:

```json
{
  "type": "hourly_health_report",
  "timestamp": "ISO8601",
  "fleet": {
    "active_pairs": 847,
    "degraded": 2,
    "failed": 0,
    "bootstrapping": 0
  },
  "actions_taken": {
    "heals_attempted": 3,
    "heals_successful": 3,
    "failovers_triggered": 0,
    "escalations_to_commander": 0
  },
  "alerts": [],
  "next_sweep_in_secs": 300
}
```
