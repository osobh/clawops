# Forge — AGENTS.md

## Agent Configuration

```yaml
agent_id: forge
display_name: Forge
model: claude-sonnet-4-6
session_type: ephemeral        # Spawned by Commander when needed; not always running
always_on: false
spawn_timeout_secs: 30
max_session_duration_mins: 120  # Auto-terminate after 2 hours of inactivity
```

## Skills Loaded

```yaml
skills:
  - skills/provision/SKILL.md         # Provisioning workflows and provider selection
  - skills/vps-fleet/SKILL.md         # Fleet topology and pair structure
  - skills/security-audit/SKILL.md    # Security requirements for new instances
```

## Tools Available

```yaml
tools:
  - gf_provision            # Core tool — provision a new pair
  - gf_pair_status          # Monitor provision progress
  - gf_provider_health      # Check provider before provisioning
  - gf_instance_health      # Verify new instances are truly healthy
  - gf_audit_log            # Write provision audit records
  - gf_teardown             # STANDBY ONLY — teardown failed standby for reprovision
  - sessions_send           # Report back to Commander
```

## Session Configuration

```yaml
session:
  persistent: false
  memory_enabled: false      # Forge is stateless — gets full context from Commander dispatch

  agent_to_agent:
    allowed_peers:
      - commander
    reply_to: commander
    max_ping_pong_turns: 50  # May have many back-and-forths during large provision batches
```

## Spawn Protocol

Commander spawns Forge with a task payload:

```json
{
  "task": "provision_pairs",
  "authorized_by": "operator",
  "confirmation_token": "tok_xyz",
  "accounts": [
    { "accountId": "acct-001", "tier": "standard" },
    { "accountId": "acct-002", "tier": "standard" }
  ],
  "primary": {
    "provider": "hetzner",
    "region": "eu-hetzner-nbg1"
  },
  "standby": {
    "provider": "vultr",
    "region": "eu-vultr-ams"
  },
  "tags": ["beta", "companion"],
  "concurrency": 5,
  "companion_webhook_url": "https://api.companion.app/webhooks/provision"
}
```

## Progress Reporting to Commander

Forge reports back to Commander every batch of 5 completions:

```json
{
  "type": "provision_progress",
  "total": 20,
  "active": 4,
  "bootstrapping": 11,
  "creating": 3,
  "failed": 2,
  "failed_accounts": ["acct-019", "acct-020"],
  "eta_secs": 360,
  "slowest_provision_secs": 468,
  "fastest_provision_secs": 252,
  "elapsed_secs": 240
}
```

Final report:

```json
{
  "type": "provision_complete",
  "total_requested": 20,
  "total_active": 18,
  "total_failed": 2,
  "failed_accounts": [
    { "accountId": "acct-019", "error": "Hetzner API timeout — retried Vultr, succeeded" },
    { "accountId": "acct-020", "error": "Both Hetzner and Vultr failed — requires manual investigation" }
  ],
  "fastest_provision_ms": 252000,
  "slowest_provision_ms": 468000,
  "avg_provision_ms": 310000,
  "provider_stats": {
    "hetzner": { "success": 16, "failed": 2, "avg_ms": 290000 },
    "vultr": { "success": 18, "failed": 0, "avg_ms": 330000 }
  },
  "webhooks_sent": 18,
  "audit_record_ids": ["rec-001", "..."]
}
```
