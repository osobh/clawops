# Provider Health Skill

## Plugin Tools Used

| Tool | When |
|------|------|
| `gf_provider_health({})` | Primary tool — check all 5 providers |
| `gf_fleet_status({ provider: "X" })` | Instance counts per provider for risk assessment |
| `gf_audit_log({ action: "provision" })` | Recent provision success rate per provider |

### Provider Check Sequence

```
1. gf_provider_health({}) → get health scores for all 5 providers
2. For any provider with score < 75: gf_fleet_status({ provider: "X" }) → count at-risk instances
3. If score < 50: recommend pausing provisions on that provider
4. If score < 30: recommend emergency failover to other providers
```

## Provider Health Scoring

Each provider is scored 0–100 based on 7-day rolling performance metrics. This score drives provisioning decisions.

## Score Calculation

```
Health Score = (
  provision_success_rate_7d × 40   +   (40 points max)
  uptime_pct_7d × 30               +   (30 points max)
  provision_speed_score × 20       +   (20 points max)
  api_stability_score × 10             (10 points max)
)

provision_speed_score:
  < 4 min avg: 20 points
  4–6 min avg: 15 points
  6–8 min avg: 10 points
  8–10 min avg: 5 points
  > 10 min avg: 0 points

api_stability_score:
  0 API errors in 7 days: 10 points
  1–2 errors: 7 points
  3–5 errors: 5 points
  > 5 errors: 0 points
```

## Provider Recommendations by Score

| Score | Recommendation | Action |
|-------|---------------|--------|
| 85–100 | Primary | Use as primary for new provisions |
| 75–84 | Primary OK | Acceptable primary, watch trend |
| 65–74 | Standby Only | Demote to standby-only |
| 50–64 | Pause | No new provisions; existing instances OK |
| < 50 | Emergency | No new provisions; consider emergency migration |

## Provider Profiles

### Hetzner
- Typical score: 90–98
- Strengths: Fastest EU provision (~4 min), excellent uptime, responsive support
- Weaknesses: EU-only datacenters; occasional Nuremberg (NBG1) network events
- Watch for: NBG1 incidents (most common datacenter issue)
- Status page: status.hetzner.com

### Vultr
- Typical score: 85–95
- Strengths: Global presence (US, EU, APAC), reliable API
- Weaknesses: Slightly slower provision than Hetzner; higher cost for equivalent specs
- Watch for: NYC/NJ region congestion; status page sometimes lags incidents
- Status page: status.vultr.com

### Contabo
- Typical score: 80–90
- Strengths: Best cost/GB (especially for storage-heavy tiers), stable EU presence
- Weaknesses: Slower provision (~6 min avg); less frequent region updates
- Watch for: Provision time drift (indicator of capacity issues)
- Status page: status.contabo.com

### Hostinger
- Typical score: 65–80 (variable)
- Strengths: Competitive pricing, APAC presence
- Weaknesses: Higher provision time variance (4–12 min); occasional API instability
- Policy: If score < 75 for > 3 consecutive checks, remove from primary pool
- Status page: status.hostinger.com

### DigitalOcean
- Typical score: 88–95
- Strengths: Reliable API, good US/EU/APAC coverage, mature platform
- Weaknesses: Higher cost than Hetzner/Contabo; limited storage options
- Watch for: NYC datacenter incidents (most common)
- Status page: status.digitalocean.com

## Provider Selection Updates

When updating provider selection algorithm:
1. Document current scores and rationale
2. Log to audit trail (AuditAction::UpdateProviderSelection)
3. Only affects NEW provisions — existing instances are unaffected
4. Commander must confirm any change to primary provider pool
5. Ledger recommends, Commander approves, audit trail records

## Interpreting Provider Health Events

**PROVIDER_DEGRADED event (from health-monitor):**
- Score dropped below 75
- Action: Commander evaluates — should new provisions pause?
- If active incident confirmed: pause new provisions immediately
- If transient API blip: monitor, don't pause yet

**Provision failure spike (>5 consecutive failures):**
- Treat as probable provider incident even if status page doesn't confirm
- Pause new provisions to that provider
- Notify Commander
- Check status page manually
