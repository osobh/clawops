# Capacity Planning Skill

## Capacity Planning Overview

Capacity planning ensures the fleet can handle projected user growth without operational surprises. Ledger runs capacity analysis monthly and provides projections to Commander and the operator.

## Key Metrics to Track

### Fleet Growth Rate
- New pairs provisioned per week (7-day rolling)
- Teardowns per week (churn indicator)
- Net growth (provisions minus teardowns)

### Provider Quota Headroom
For each provider, track:
- Current instance count vs quota limit
- Quota consumption rate (instances per week)
- Weeks until quota exceeded at current growth rate

Alert threshold: < 4 weeks of headroom at current growth rate

### Cost Projection

```
Monthly cost projection formula:
  Base cost = current_pair_count × avg_cost_per_pair
  Growth cost = net_new_pairs_per_month × avg_cost_per_pair
  Projected_next_month = Base cost + Growth cost

If actual > projected by > 15%: COST_ANOMALY
```

## Monthly Capacity Report Structure

Briefer delivers this monthly (first of month, 09:00 UTC). Ledger compiles the data.

```
Fleet Capacity Report — [Month Year]

GROWTH
• Active pairs: N (+X from last month, +Y% growth)
• Provisions this month: N | Teardowns: N | Net: +N
• Projected pairs next month: N (based on 90-day trend)
• Projected pairs in 6 months: N

COST TRAJECTORY
• Current monthly: $X
• Projected next month: $Y (growth driven)
• Projected 6-month run rate: $Z/month
• Cost per active account: $A (trending: up/down/flat)

PROVIDER HEADROOM
• Hetzner: N instances (quota: M | X weeks headroom)
• Vultr: N instances (quota: M | X weeks headroom)
• [etc.]

RECOMMENDATION
• [Most important capacity action]
• [Provider to increase quota on]
• [Tier shift that would improve economics]
```

## Growth Scenarios

When projecting growth, use three scenarios:

**Conservative (actual × 0.8):** Plan for this for quota requests
**Base (actual trend):** Use for cost projections
**Aggressive (actual × 1.5):** Use for infrastructure headroom planning

Never commit to a provider contract based on aggressive scenario alone.

## Quota Management

When a provider reaches 80% of quota:
1. Ledger alerts Commander
2. Commander requests quota increase with that provider
3. Lead time varies: Hetzner (~1 day), Vultr (~1 day), Contabo (~3–5 days), Hostinger (~3 days)
4. If quota increase cannot arrive in time: temporarily route new provisions to alternate provider
5. Document in audit trail: reason for provider switch, expected quota resolution date

## Tier Distribution Analysis

Monitor the tier mix monthly:
- What % of accounts are on each tier?
- Is nano usage growing faster than standard? (Indicates Ledger's downsize recommendations working)
- Is pro/enterprise growing? (Positive revenue signal)
- Any accounts still on a tier they no longer qualify for?

## Infrastructure Runway

Define "runway" as months until a specific constraint is hit:
- Provider quota runway
- Cost budget runway (if monthly budget is fixed)
- Team operational capacity (how many accounts can be manually managed if automation fails)

Report all three in monthly capacity report. Highlight any runway < 3 months.
