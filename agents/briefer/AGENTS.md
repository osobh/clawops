# Briefer — AGENTS.md

## Agent Configuration

```yaml
agent_id: briefer
display_name: Briefer
model: claude-haiku-4-5-20251001  # Fast and cheap — just synthesis and formatting
session_type: ephemeral            # Wakes up, generates report, sends, sleeps
always_on: false
spawn_timeout_secs: 30
max_session_duration_mins: 15      # Should complete a briefing in under 15 minutes
```

## Skills Loaded

```yaml
skills:
  - skills/vps-fleet/SKILL.md    # Fleet state interpretation
  - skills/cost-analysis/SKILL.md  # Cost data interpretation
```

## Tools Available

```yaml
tools:
  - gf_fleet_status       # Current fleet state
  - gf_cost_report        # Cost summary for the period
  - gf_incident_report    # Overnight incidents
  - gf_audit_log          # Actions taken overnight
  # NO write tools — Briefer is read-only
```

## Session Configuration

```yaml
session:
  persistent: false
  memory_enabled: true
  memory_dir: memory/briefer/
  # Briefer keeps incident pattern history for trend recognition
  memory_schema:
    - incident_log.json      # Historical incidents for pattern detection
    - briefing_history.json  # Last 30 days of briefing summaries

  agent_to_agent:
    allowed_peers:
      - commander
      - ledger    # Receives weekly cost data from Ledger
```

## Schedule

```yaml
schedule:
  # Daily voice briefing
  - cron: "0 7 * * *"
    task: daily_voice_briefing
    channel: whatsapp
    format: voice_note
    data_sources:
      - gf_fleet_status({})
      - gf_incident_report({ action: "list", listFilter: { from: "yesterday_00:00Z", status: "RESOLVED" } })
      - gf_cost_report({ period: "last_7d" })
      - gf_audit_log({ from: "yesterday_00:00Z" })

  # Weekly cost digest (Monday)
  - cron: "0 8 * * 1"
    task: weekly_cost_digest
    channel: telegram
    format: telegram_markdown
    data_sources:
      - ledger_weekly_report   # Ledger sends this Monday 07:00; Briefer uses it at 08:00
      - gf_fleet_status({})
      - gf_incident_report({ action: "list", listFilter: { period: "last_7d" } })

  # Monthly capacity planning report
  - cron: "0 9 1 * *"
    task: monthly_capacity_report
    channel: telegram
    format: telegram_markdown
```

## Output Channels

```yaml
channels:
  whatsapp:
    recipient: "+1XXXXXXXXXX"
    voice_note: true            # ElevenLabs TTS integration
    voice_model: "eleven_turbo_v2"
    voice_id: "calm_professional_male"

  telegram:
    recipient: "@omar_username"
    format: markdown
    parse_mode: "MarkdownV2"
```

## Briefing Generation Steps

```
On daily_voice_briefing trigger:
  1. Fetch all data sources in parallel
  2. Check if any incidents are still OPEN (escalate to Commander if so)
  3. Generate voice script (target: 75 words = ~60 seconds)
  4. Send via WhatsApp voice note (ElevenLabs TTS)
  5. Log delivery to gf_audit_log
  6. If > 1 significant item: send follow-up Telegram text summary

On weekly_cost_digest trigger:
  1. Wait for Ledger's Monday report (retry up to 3× if not received)
  2. Fetch current fleet status
  3. Format Telegram markdown digest
  4. Send to Telegram
  5. Log delivery
```
