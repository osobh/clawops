# Triage — AGENTS.md

## Agent Configuration

```yaml
agent_id: triage
display_name: Triage
model: claude-opus-4-6     # Needs deep reasoning for root cause analysis
session_type: ephemeral    # Spawned on-demand only; not always running
always_on: false
spawn_timeout_secs: 30
max_session_duration_mins: 240  # Up to 4 hours for major incidents
```

## Skills Loaded

```yaml
skills:
  - skills/incident-response/SKILL.md   # Incident management playbooks
  - skills/instance-diagnose/SKILL.md   # Diagnostic sequences
  - skills/vps-fleet/SKILL.md           # Fleet topology for blast radius assessment
  - skills/provider-health/SKILL.md     # Provider status interpretation
  - skills/auto-heal/SKILL.md           # Understand what Guardian already did
```

## Tools Available

```yaml
tools:
  # READ-ONLY tools only — Triage never executes remediation
  - gf_fleet_status       # Fleet-wide blast radius assessment
  - gf_instance_health    # Detailed instance diagnostics
  - gf_pair_status        # Account pair status
  - gf_provider_health    # Provider status + history
  - gf_audit_log          # Full audit trail for root cause analysis
  - gf_incident_report    # Create and update incident records
  - sessions_send         # Report to Commander (only A2A output)
```

## Session Configuration

```yaml
session:
  persistent: false
  memory_enabled: false   # Gets full context from Commander spawn payload

  agent_to_agent:
    allowed_peers:
      - commander
    reply_to: commander
    max_ping_pong_turns: 10
```

## Spawn Protocol

Commander spawns Triage with an incident context payload:

```json
{
  "task": "investigate_incident",
  "triggered_by": "operator_question",
  "operator_context": "Hetzner Nuremberg looks down. What's our exposure?",
  "scope": {
    "suspected_providers": ["hetzner"],
    "suspected_regions": ["eu-hetzner-nbg1"],
    "time_window_start": "ISO8601",
    "known_affected_instances": []
  },
  "priority": "sev2",
  "requested_by": "commander"
}
```

## Incident Report Format

Triage delivers to Commander in this structure:

```json
{
  "type": "incident_report",
  "incidentId": "inc-2024-001",
  "severity": "sev2",
  "title": "Hetzner Nuremberg Network Outage",
  "detectedAt": "ISO8601",
  "reportedAt": "ISO8601",

  "blast_radius": {
    "total_affected_accounts": 94,
    "users_without_active_gateway": 0,
    "failovers_complete": 87,
    "failovers_in_progress": 7,
    "eta_all_recovered_mins": 2
  },

  "timeline": [
    { "ts": "ISO8601", "event": "Guardian detected 12 heartbeat timeouts in NBG1", "agent": "guardian", "automated": true },
    { "ts": "ISO8601", "event": "Guardian triggered auto-heal on 12 instances — all failed Step 2 (SSH timeout)", "agent": "guardian", "automated": true },
    { "ts": "ISO8601", "event": "Guardian triggered failover on 12 instances — standbys active", "agent": "guardian", "automated": true },
    { "ts": "ISO8601", "event": "Hetzner status page updated: network issues in NBG1", "agent": "triage", "automated": false }
  ],

  "root_cause": {
    "hypothesis": "Hetzner Nuremberg network infrastructure failure",
    "confidence": "high",
    "evidence": [
      "Hetzner status page confirms NBG1 network issues",
      "All 94 affected instances are in NBG1 datacenter",
      "SSH timeouts (not application errors) — infrastructure-level",
      "Instances in NBG2 (same account) unaffected"
    ]
  },

  "actions_taken": {
    "by_guardian": "87 failovers complete, 7 in progress",
    "by_commander": "None yet — awaiting triage report",
    "recommended": [
      "Silence NBG1 health alerts for 30 minutes",
      "Queue reprovision of 94 standbys for when NBG1 recovers",
      "Brief operator with blast radius and ETA"
    ]
  },

  "open_items": [
    "7 pairs still in STANDBY_PROMOTING state (ETA 2 min)",
    "Monitor for reprovision when NBG1 recovers"
  ]
}
```
