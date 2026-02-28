# VPS Fleet Topology Skill

## Fleet Architecture

The GatewayForge fleet is a collection of **user gateway pairs**. Each user account gets:
- 1 PRIMARY VPS running OpenClaw
- 1 STANDBY VPS on a different provider/region (hot spare)

The pair relationship is stored in GatewayForge DB and reflected in every instance's metadata.

## Instance States

| State | Meaning | Action |
|-------|---------|--------|
| `ACTIVE` | Healthy, serving user traffic | No action needed |
| `DEGRADED` | Health score < 70, some services struggling | Guardian: auto-heal |
| `FAILED` | Unrecoverable — not serving traffic | Immediate failover |
| `BOOTSTRAPPING` | Provision in progress, almost ready | Wait — ETA 1–3 minutes |
| `CREATING` | Just provisioned, installing software | Wait — ETA 3–6 minutes |
| `STANDBY_PROMOTING` | Standby is taking over PRIMARY role | Watch — ETA 60–120 seconds |
| `DECOMMISSIONED` | Teardown complete | No longer in fleet |

## Understanding Health Scores

Health score (0–100) computed from:
- OpenClaw HTTP status (40 points — most important)
- Docker container states (up to 20 points deducted per unhealthy container)
- Disk usage (15 points for > 90%)
- CPU + RAM (10 points each for > 95%)

**Thresholds:**
- ≥ 70: Healthy — no action
- 50–69: Degraded — monitor + prepare to heal
- < 50: Critical — trigger auto-heal sequence
- Unknown/missing heartbeat > 3 min: Treat as Critical

## Provider Region Codes

```
Hetzner:
  eu-hetzner-nbg1   Nuremberg, Germany
  eu-hetzner-hel1   Helsinki, Finland
  eu-hetzner-fsn1   Falkenstein, Germany

Vultr:
  us-vultr-ewr      New Jersey, US
  us-vultr-lax      Los Angeles, US
  eu-vultr-ams      Amsterdam, Netherlands
  ap-vultr-sgp      Singapore

Contabo:
  eu-contabo-de     Germany
  us-contabo-us     US Central

Hostinger:
  eu-hostinger-lt   Lithuania
  us-hostinger-us   US

DigitalOcean:
  us-do-nyc3        New York, US
  us-do-sfo3        San Francisco, US
  eu-do-ams3        Amsterdam, Netherlands
  ap-do-sgp1        Singapore
```

## Service Tiers

| Tier | vCPU | RAM | Disk | Bandwidth | Monthly Cost |
|------|------|-----|------|-----------|-------------|
| nano | 1 | 1 GB | 20 GB | 1 TB | ~$4 |
| standard | 2 | 4 GB | 80 GB | 4 TB | ~$12 |
| pro | 4 | 8 GB | 160 GB | 8 TB | ~$24 |
| enterprise | 8 | 16 GB | 320 GB | 20 TB | ~$48 |

Standard is the default for new COMPANION accounts. Most accounts will stay on standard unless Ledger identifies clear overprovisioning or underprovisioning.

## Reading Fleet Status Output

When you call `gf_fleet_status({})`, key fields to surface to operator:

1. **Total active pairs** — main health indicator
2. **Degraded + failed counts** — anything > 0 needs attention
3. **Bootstrapping count** — in-progress provisions
4. **By provider breakdown** — identify if problem is provider-specific
5. **Cost deviation %** — flag if > 10% above projection

## Pair Resilience Design

A pair is resilient when:
- Primary and standby are on DIFFERENT providers
- Primary and standby are in DIFFERENT cities (ideally different continents for enterprise)
- Both instances have health_score >= 70
- Standby is confirmed ACTIVE (not just CREATED)

A pair is at risk when:
- Primary and standby are on the SAME provider (single point of failure)
- Standby health_score < 70 (can't be promoted if primary fails)
- No standby exists (unpaired instance)
