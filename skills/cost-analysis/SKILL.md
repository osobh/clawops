# Cost Analysis Skill

## Cost Analysis Framework

Ledger analyzes fleet costs to identify waste and optimize spending. The goal is maximum value per dollar — not minimum spend at the cost of reliability.

## Cost Baseline

Approximate monthly costs per pair (primary + standby):

| Tier | Hetzner+Vultr | Contabo+Hetzner | Hostinger+DO |
|------|--------------|-----------------|-------------|
| nano | $8/month | $6/month | $7/month |
| standard | $24/month | $18/month | $20/month |
| pro | $48/month | $36/month | $42/month |
| enterprise | $96/month | $72/month | $84/month |

## Waste Categories

### Category 1: Idle Accounts

**Definition:** Account with no meaningful activity for 14+ consecutive days.

**What counts as activity:**
- HTTP requests to OpenClaw (any endpoint)
- Active user sessions
- Webhook calls from COMPANION

**What does NOT count:**
- Health monitor calls (these come from our infrastructure)
- Heartbeat checks
- Automated cron tasks

**Recommendation thresholds:**
- 14–30 days idle: Flag for review — might be a trial user who churned
- 30+ days idle: Recommend teardown with 30-day S3 archive
- 60+ days idle: Strong recommend teardown (archive still applies)

**How to identify:**
1. Call `gf_cost_report({ includeIdleAnalysis: true })`
2. Review `wasteItems` where `category == "idle_accounts"`
3. Cross-reference with COMPANION user activity data when available

### Category 2: Overprovisioned Instances

**Definition:** Instance where P95 CPU < 20% AND P95 RAM < 30% for 7+ consecutive days.

**Why 7 days:** Prevents false positives from temporary low-usage periods (weekends, holidays).

**Typical finding:** standard tier accounts (2vCPU/4GB) running at nano-level workloads.

**Recommendation:**
- standard → nano: saves ~$8/month per pair
- pro → standard: saves ~$24/month per pair
- pro → nano: saves ~$32/month per pair (review carefully — unusual)

**Confidence levels:**
- High confidence: 14+ days of consistent data below threshold
- Medium confidence: 7–13 days
- Low confidence: < 7 days — do not recommend downsize yet

### Category 3: Suboptimal Provider

**Definition:** Current provider costs more than an alternative with equivalent or better reliability score.

**How to calculate:**
1. Get current provider monthly cost for the instance tier
2. Get alternative provider cost for same tier
3. Compare reliability score (provision_success_rate, uptime, avg_latency)
4. If alternative is both cheaper AND same/better reliability: flag

**Common findings:**
- Contabo primaries where Hetzner hourly billing is cheaper (short-tenure accounts)
- Hostinger primaries where health_score < 75 consistently

**Recommendation format:**
```
"6 Contabo primary instances (standard tier, < 30 days tenure):
 Hetzner hourly billing would cost $0.80/month less per instance.
 Hetzner 30-day reliability: 98.5% vs Contabo 97.2%.
 Total savings: $43/month. Effort: requires reprovisioning (zero downtime with standby)."
```

## Reporting Format

Always structure Ledger reports with:
1. Dollar amounts first (total actual, projected, deviation)
2. Top 3 waste items by dollar value (not count)
3. Total recoverable (summed)
4. One recommended action per item (specific, actionable)
5. Confidence level for each recommendation

## Cost Trend Indicators

**Normal variance:** ±5% month-over-month (expected from user growth/churn)
**Yellow flag:** +10% above projection (investigate — likely unplanned provisions)
**Red flag:** +15% above projection (COST_ANOMALY event triggered)
**Hard investigate:** -10% (users leaving — monitor alongside COMPANION churn data)
