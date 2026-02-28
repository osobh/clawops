# Config Push Skill

## Config Deployment Overview

Config pushes update the OpenClaw configuration on running instances. Most config changes can be hot-reloaded (no restart required). Model changes, channel additions, and memory directory changes require awareness of timing.

## When Config Push Is Safe

**Safe to push without downtime:**
- Model routing changes (model.primary, model.fallbacks)
- System prompt updates
- Tool additions/removals
- Skill additions
- Log level changes

**Requires restart (brief downtime, ~30s per instance):**
- Memory directory path changes
- Channel credential updates (WhatsApp, Telegram tokens)
- Port or TLS configuration changes

**Requires operator awareness:**
- Changing default model: affects every user's next interaction
- Disabling a tool: any in-flight sessions using that tool will error
- Changing memory directory: existing conversation history won't be accessible at new path

## Pre-Push Validation

Before pushing to any instance:
1. Call `config.diff` — verify the diff looks as expected
2. Check target instance health_score >= 60 (don't push to already-degraded instances)
3. Verify the config passes syntax validation (GatewayForge validates before accepting)
4. For model changes: verify new model ID is available in the target OpenClaw version

## Rolling Push Strategy (Required for > 10 instances)

```
Batch size: 50 instances
Wait between batches: 60 seconds (allow validation to settle)
Success criteria per batch: >= 90% applied successfully
Failure threshold: if > 10% of a batch fails, STOP

Process:
  Batch 1 (50 instances):
    → Push config
    → Wait 60s
    → Check health for all 50 (openclaw.health)
    → If >= 45 healthy: proceed to Batch 2
    → If < 45 healthy: STOP, notify Commander, do NOT continue

  Batch 2, 3, ... N:
    Same process

On STOP:
  → All unmodified instances remain on old config
  → All modified instances: initiate rollback if error rate > 20%
  → Never push to more instances when a batch is failing
```

## All-At-Once Push (Immediate Strategy)

Only use when:
- Target count <= 10 instances
- Operator has explicitly requested it
- You have a valid confirmationToken for > 100 instances

The risk: if the new config is broken, ALL instances fail simultaneously. Rolling mode protects against this.

## After a Push

1. Wait 5 minutes, then check random sample (10%) of pushed instances
2. Verify error rate hasn't spiked (openclaw.metrics → http_error_rate)
3. If error rate increased: rollback all instances in the batch
4. Log final outcome to audit trail (success count, failure count, version)

## Config Version Tracking

Each successful config push bumps the config_version field in GatewayForge DB. Always note the version in audit logs. If rollback is needed, you're rolling back to version N-1.

```
Current version: v47
After push: v48
If rollback needed: v47 is restored
```

## Common Config Push Mistakes

| Mistake | Consequence | Prevention |
|---------|-------------|-----------|
| All-at-once to > 100 instances | Fleet-wide outage if config broken | Always use rolling |
| Pushing to degraded instances | Makes bad situation worse | Check health before push |
| Skipping validation | Config accepted but invalid | Always run config.diff first |
| Not logging the change | Can't audit who changed what | Audit trail is mandatory |
| Pushing standby AND primary simultaneously | Both fail if config is bad | Push primary first, verify, then standby |
