# Gateway Manager Skill

## OpenClaw Gateway Lifecycle

Each user account's OpenClaw gateway runs inside a Docker container on their PRIMARY VPS instance. Forge deploys it at provision time; gf-clawnode manages it at runtime.

## Gateway States

| State | Description | Resolution |
|-------|-------------|-----------|
| `RUNNING` | HTTP 200 on /health endpoint | Normal |
| `DEGRADED` | HTTP 200 but error rate high (> 5%) | Monitor; may self-recover |
| `DOWN` | No HTTP response | docker compose restart openclaw |
| `STARTING` | Recently restarted, not yet ready | Wait 30s, recheck |
| `CONFIG_ERROR` | Started but config validation failed | Config rollback |

## OpenClaw Commands (via gf-clawnode)

These are the commands dispatched to instances through the WebSocket node protocol:

```
openclaw.health     — HTTP GET /health, returns status + version + uptime
openclaw.restart    — docker compose restart openclaw (graceful)
openclaw.stop       — docker compose stop openclaw
openclaw.start      — docker compose start openclaw
openclaw.logs       — tail -n 100 openclaw container logs
openclaw.update     — docker pull + rolling restart (use for version upgrades)
config.push         — write new config file + validate + hot-reload
config.get          — return current config (secrets redacted)
config.rollback     — revert to previous config version
```

## Config Push Process

When pushing config changes:

1. Call `config.get` first — understand current state
2. Build config diff — only changed fields
3. Call `config.push` with new config
4. Gateway validates before accepting (syntax, required fields, model availability)
5. Hot-reload if validation passes (no restart needed for most changes)
6. If validation fails: config.rollback automatically reverts
7. Log to audit trail

**Config validation rules:**
- `model.primary` must be a valid model ID
- `model.fallbacks` array must have at least 1 entry
- `channels` must have valid credentials
- `memory_dir` must be a writable path on the instance

## Rolling Config Push (Fleet-Wide)

When pushing to > 1 instance, always use rolling strategy:

```
Batch size: 50 instances
Validation per batch: wait for all 50 to confirm, check error rate
If batch error rate > 10%: STOP, notify Commander, do not continue
Auto-rollback on validation error: always enabled
```

For > 100 instances: require operator confirmation before starting.
For > 500 instances: require Commander + operator confirmation + test batch of 10 first.

## Gateway Version Management

OpenClaw versions are managed per-instance. Version drift is expected and acceptable up to 2 minor versions behind latest.

Signs of version issues:
- `openclaw.health` returns version < N-2 minor versions
- Error rate spike immediately after provision (likely older image)
- `MODEL_NOT_FOUND` errors (model added in newer version)

To update: `openclaw.update` (docker pull + rolling restart, ~90s downtime per instance).
Never update primary and standby simultaneously — update standby first.
