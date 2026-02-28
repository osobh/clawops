# Provisioning Skill

## Provisioning Overview

Provisioning creates a primary/standby VPS pair for a user account. Forge owns this process. Standard provision time: 4–8 minutes.

## Pre-Provision Checklist

Before starting any provision batch:

1. **Provider health**: Call `gf_provider_health({})`. Verify target providers have score >= 75.
2. **Region availability**: Confirm target region is not under incident.
3. **Quota headroom**: Verify provider quota won't be exceeded by this batch.
4. **Operator authorization**: Confirm provision count and cost were approved.
5. **Audit log**: Log provision intent to gf-audit BEFORE first API call.

## Provider Selection Rules

```
Primary provider selection:
  ✅ health_score >= 75
  ✅ provision_success_rate_7d >= 95%
  ✅ avg_provision_time < 10 minutes
  ❌ Active incident on target region → do not provision there

Standby provider selection:
  - Must be DIFFERENT provider than primary
  - Must be DIFFERENT city than primary
  - Prefer DIFFERENT continent for enterprise tier
  - Apply same health score requirements
```

Default pair configurations:

| Primary | Standby | Use case |
|---------|---------|---------|
| Hetzner EU (nbg1) | Vultr EU (ams) | Default EU accounts |
| Hetzner EU (hel1) | Vultr EU (ams) | EU redundancy option |
| Vultr US (ewr) | DigitalOcean US (nyc3) | US accounts |
| Contabo EU (de) | Hetzner EU (fsn1) | Budget EU |

## Provision Sequence (Per Pair)

```
1. Log to gf-audit: { action: "provision_primary", account_id, tier, provider, region }
2. Call gf_provision({ accountId, tier, primaryProvider, primaryRegion, standbyProvider, standbyRegion })
3. Receive provisionRequestId
4. Poll gf_pair_status({ accountId, provisionRequestId }) every 60 seconds
5. States to watch: CREATING → BOOTSTRAPPING → ACTIVE
6. Timeout: if not ACTIVE after 15 minutes, log TIMEOUT and report to Commander
7. On ACTIVE: call gf_instance_health({ instanceId, live: true }) — verify score >= 70
8. Send COMPANION webhook with pair details
9. Log to gf-audit: { action: "provision_complete", instance_ids, duration_ms }
```

## Retry Logic

```
Attempt 1: Try stated primary provider
If FAILED:
  Attempt 2: Try same provider in different region (if available)
If FAILED again:
  Notify Commander — do not auto-switch to a different provider
  Wait for Commander instruction before proceeding

Exception: If provider has active incident confirmed (status page)
  → Switch to next-best healthy provider
  → Notify Commander of the switch
  → Document in provision audit record
```

## Bulk Provision Management

For N accounts (after operator confirmation):
- Run provisions concurrently (default: 5 at a time)
- Report progress to Commander every 5 completions
- If > 20% of batch fails: pause and notify Commander
- Log failed accounts for manual review
- Never exceed authorized count

## Standby Provision

After primary is ACTIVE, immediately provision standby:
- Use pair_instance_id = primary's instanceId
- This links them in GatewayForge DB
- Standby may be CREATING while primary is ACTIVE — this is normal
- Fleet is not "protected" until standby also reaches ACTIVE

## What "Bootstrap" Does

When gf-clawnode boots on a new VPS, it:
1. Authenticates with GatewayForge using instance API key
2. Connects to OpenClaw gateway via Tailscale WebSocket
3. Registers all command handlers
4. Sends first heartbeat
5. Waits for `config.push` from Forge with the user's OpenClaw config
6. Starts OpenClaw via `docker compose up -d`
7. Confirms ACTIVE when HTTP 200 received from OpenClaw

Bootstrap failures are usually:
- Cloud-init failed to install Docker or Tailscale
- Network issue during image pull
- GatewayForge API unreachable (check Tailscale)
