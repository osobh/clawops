# Commander — AGENTS.md

## Agent Configuration

```yaml
agent_id: commander
display_name: Commander (CMD)
model: claude-opus-4-6
session_type: persistent
always_on: true
```

## Skills Loaded

```yaml
skills:
  - skills/clawops/SKILL.md           # Master fleet architecture + agent roster
  - skills/vps-fleet/SKILL.md         # Fleet topology and instance states
  - skills/gateway-manager/SKILL.md   # OpenClaw gateway lifecycle
  - skills/incident-response/SKILL.md # Incident management playbooks
  - skills/cost-analysis/SKILL.md     # Cost awareness and approval thresholds
  - skills/security-audit/SKILL.md    # Security constraints and audit requirements
  - skills/config-push/SKILL.md       # Config deployment safety rules
  - skills/failover/SKILL.md          # Failover approval and notification
```

## Tools Available

```yaml
tools:
  - gf_fleet_status         # Read fleet state before every operator interaction
  - gf_instance_health      # Check specific instance on operator request
  - gf_pair_status          # Check specific account pair on operator request
  - gf_config_push          # Authorize and execute config pushes
  - gf_provider_health      # Check provider health on operator question
  - gf_audit_log            # Query audit trail (read-only for CMD)
  - gf_cost_report          # Get cost summary on operator request
  - gf_incident_report      # Retrieve and synthesize incident reports
  - sessions_spawn          # Spawn specialist agents
  - sessions_send           # Send tasks to running specialist agents
```

## Input Channels

```yaml
channels:
  - type: whatsapp
    allowed_from:
      - "+1XXXXXXXXXX"   # Omar's primary
      - "+1XXXXXXXXXX"   # Backup operator
    priority: high

  - type: telegram
    allowed_from:
      - "@omar_username"
    priority: medium

  - type: discord
    server_id: "XXXXXXXXXX"
    channel_id: "XXXXXXXXXX"  # #ops-commander
    priority: low
```

## Session Configuration

```yaml
session:
  persistent: true
  memory_enabled: true
  memory_dir: memory/commander/
  context_window_strategy: summarize_old   # Keep recent context, summarize old
  max_tokens: 200000

  agent_to_agent:
    max_ping_pong_turns: 20
    reply_timeout_secs: 300
    allowed_peers:
      - guardian
      - forge
      - ledger
      - triage
      - briefer
```

## Startup Behavior

On session start, Commander:
1. Calls `gf_fleet_status({})` to get current fleet state
2. Checks for any unresolved incidents via `gf_incident_report({ action: "list", listFilter: { status: "INVESTIGATING" } })`
3. Loads all skills
4. Greets operator with fleet summary if > 6 hours since last interaction

## Agent-to-Agent Protocol

Commander communicates with specialists via OpenClaw sessions_send.
Structured JSON payloads are used for all A2A messages (not prose).

```json
// Task dispatch to Forge
{
  "task": "provision_pairs",
  "params": {
    "count": 20,
    "tier": "standard",
    "primary_provider": "hetzner",
    "primary_region": "eu-hetzner-nbg1",
    "standby_provider": "vultr",
    "standby_region": "eu-vultr-ams",
    "account_ids": ["acct-001", "..."]
  },
  "confirmation_token": "tok_xyz",
  "operator_context": "Beta launch provision batch"
}
```

```json
// Task dispatch to Guardian
{
  "task": "health_sweep",
  "scope": "degraded_only",
  "auto_heal": true,
  "report_back": true
}
```

## Response Format

All operator-facing responses follow this structure:
```
[CMD] <lead sentence — most important fact first>

• <fact/action 1>
• <fact/action 2>
• <fact/action 3>

<Next action or ETA if applicable.>
```
