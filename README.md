# ClawOps

**Conversational Infrastructure Operations Platform**
GatewayForge × Clawbernetes × OpenClaw
_RedClaw Systems LLC · Rust 1.93+ · Zero Operator Dashboards_

> "You message your ops agent. You wake up to a voice summary. Zero dashboards. Zero on-call pages."

---

## Overview

ClawOps is the conversational operations layer that sits on top of **GatewayForge** and the **COMPANION platform**. Instead of dashboards, alert consoles, and runbooks, operators manage their entire fleet of user OpenClaw gateways by talking to an agent team.

The foundation is **Clawbernetes** — an open-source, AI-native orchestration system adapted for the GatewayForge use case: managing a fleet of user VPS instances rather than GPU compute nodes.

### North Star

```
You: "Provision 50 new accounts, optimize costs, restart anything degraded, brief me in the morning."
     [wake up]
Agent: [Dispatch — 07:00 UTC] Good morning. Fleet status: 847 active pairs across 3 providers.
       2 degraded (auto-healing in progress), 1 failed (failover complete). Cost this week: $1,247.
       No SLA breaches. Full report in your Drive.
```

---

## Architecture

```
Operator (WhatsApp / Telegram / Web)
         │
         ▼
  ┌──────────────┐
  │   OpenClaw   │  ← AI gateway (routes messages to agents)
  └──────┬───────┘
         │
         ▼
  ┌──────────────────────────────────────────────────────────┐
  │  Commander Agent  (Senior SRE, always-on, Opus 4.6)     │
  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────┐  │
  │  │ Guardian │ │  Forge   │ │  Ledger  │ │  Triage   │  │
  │  │(watchdog)│ │(provisn) │ │  (cost)  │ │(incident) │  │
  │  └──────────┘ └──────────┘ └──────────┘ └───────────┘  │
  │                    ┌────────┐                            │
  │                    │Briefer │ (cron: reports)            │
  │                    └────────┘                            │
  └───────────────────────┬──────────────────────────────────┘
                          │  ClawOps Plugin (TypeScript, 12 tools)
                          ▼
              ┌─────────────────────┐
              │ GatewayForge REST   │  /v1/gateways, /v1/instances
              └──────────┬──────────┘
                         │
         ┌───────────────┼───────────────┐
         ▼               ▼               ▼
   ┌──────────┐   ┌──────────┐   ┌──────────┐
   │  Hetzner │   │  Vultr   │   │  Contabo │  ...+2 providers
   └──────────┘   └──────────┘   └──────────┘
         │               │               │
         ▼               ▼               ▼
   ┌───────────────────────────────────────────┐
   │     clawnode  (Rust binary, per VPS)      │
   │  Handles: health, docker, config, secrets │
   └──────────────────┬────────────────────────┘
                      │ WebSocket (Tailscale)
                      ▼
              ┌───────────────┐
              │ OpenClaw GW   │  (per VPS instance)
              └───────────────┘
```

### Agent Communication Topology

```
Operator       → Commander:  WhatsApp/Telegram message
Commander      → Forge:      sessions_spawn for provisioning work
Commander      → Guardian:   sessions_send "health sweep on degraded instances"
Commander      → Ledger:     sessions_send "generate weekly cost report"
Guardian       → Commander:  "instance {id} unrecoverable, failover triggered"
Briefer        → Operator:   cron-triggered daily voice note via WhatsApp
Triage         → Commander:  incident report ready for synthesis
```

---

## Quick Start

### Prerequisites

- Rust 1.93+ (use `rustup`)
- Node.js 20+ / npm 10+
- OpenClaw gateway running and configured
- GatewayForge API access (service API key)
- Tailscale network configured across all VPS instances

### 1. Build the Rust workspace

```bash
git clone https://github.com/redclaw/clawops
cd clawops
cargo build --workspace --release
```

### 2. Build the TypeScript plugin

```bash
cd plugin/openclaw-clawops
npm install
npm run build
```

### 3. Deploy clawnode to a VPS

```bash
# Copy the binary to the VPS
scp target/release/clawnode root@your-vps:/usr/local/bin/

# Create config
cat > /etc/clawnode/config.json << 'EOF'
{
  "gateway":    "wss://gateway.example.com",
  "token":      "<gateway-api-token>",
  "hostname":   "vps-acct-001-primary",
  "provider":   "hetzner",
  "region":     "eu-hetzner-nbg1",
  "tier":       "standard",
  "role":       "primary",
  "account_id": "acct-001",
  "state_path": "/var/lib/clawnode"
}
EOF

# Run as a systemd service
systemctl enable --now clawnode
```

### 4. Configure agents

Load each agent's `SOUL.md` and `AGENTS.md` from the `agents/` directory into the corresponding OpenClaw workspace, then start the session.

### 5. Run tests

```bash
cargo test --all                    # 273 tests, all pass
cargo clippy -- -D warnings         # zero warnings
cargo doc --no-deps                 # docs build cleanly
```

---

## Crate Reference

| Crate | Type | Description |
|---|---|---|
| `clawnode` | binary | VPS node agent; WebSocket connection to OpenClaw gateway; handles all `vps.*`, `docker.*`, `health.*`, `config.*`, `secret.*`, `auth.*`, and `audit.*` commands |
| `claw-proto` | lib | Protocol types shared across crates: `HealthReport`, `InstanceState`, `VpsProvider`, `ProvisionRequest`, `ProvisionResult`, `InstancePairStatus`, etc. |
| `claw-persist` | lib | `JsonStore` — atomic file-backed key-value persistence; used by auth, config, secrets, and audit crates |
| `claw-identity` | lib | Ed25519 device identity with challenge-response signing for node authentication |
| `claw-secrets` | lib | `SecretStore` — AES-256-GCM encrypted secret storage; custom `Debug` redacts ciphertext; zeroize-on-drop; rotation tracking |
| `claw-auth` | lib | `ApiKeyStore` (SHA-256 hashed, expiry, rotation), `AuditLogStore`, `InputSanitizer` (hostname/IP/command validation), `RateLimiter` |
| `claw-config` | lib | `ConfigStore` with immutable-flag support; snapshot persistence |
| `claw-metrics` | lib | Push-based `MetricStore` with retention, `FleetMetrics` aggregation, `CostTracker` per-provider |
| `claw-health` | lib | Health scoring (0–100), `evaluate_alerts`, `recommend_action`, `AutoHealDecision`, `FailoverStateMachine` (6-step PRD sequence) |
| `claw-provision` | lib | Multi-provider VPS provisioning (Hetzner real, 4 others stubbed); `ProviderRegistry`; `RetryPolicy` with exponential backoff + jitter |
| `claw-audit` | lib | Immutable append-only SHA-256 chain audit trail; `AuditLogger` with `verify_chain()` |
| `claw-observe` | lib | `OperationsMetrics` (atomic counters), `MetricsExporter` (Prometheus text), `AuditLogger` (structured JSON fleet ops log) |
| `claw-ledger` | lib | Cost analysis, spend projections, waste detection |
| `claw-triage` | lib | Incident classification, timeline reconstruction, impact scoring |
| `claw-commander` | lib | Commander agent orchestration logic |
| `claw-briefer` | lib | Scheduled report generation and dispatch |
| `clawops-tests` | test | Integration + adversarial safety tests (28 safety tests, 46 integration tests) |

---

## Configuration Reference

### clawnode (`/etc/clawnode/config.json`)

| Key | Type | Required | Description |
|---|---|---|---|
| `gateway` | string | yes | WebSocket URL of the OpenClaw gateway (`wss://...`) |
| `token` | string | yes | Gateway API authentication token |
| `hostname` | string | yes | Unique hostname for this VPS instance |
| `provider` | string | yes | Provider name: `hetzner`, `vultr`, `contabo`, `hostinger`, `digitalocean` |
| `region` | string | yes | Region ID: e.g. `eu-hetzner-nbg1`, `us-vultr-ewr` |
| `tier` | string | yes | Instance tier: `nano`, `standard`, `pro`, `enterprise` |
| `role` | string | yes | Instance role: `primary`, `standby` |
| `account_id` | string | yes | GatewayForge account ID this instance serves |
| `state_path` | string | yes | Directory for persisted state files (JSON stores) |
| `pair_instance_id` | string | no | Instance ID of the paired primary/standby |
| `health_check_interval_secs` | u64 | no | Health check interval (default: 30) |

### Environment Variables

| Variable | Description |
|---|---|
| `RUST_LOG` | Log level filter (e.g. `info`, `debug`, `clawnode=debug`) |
| `CLAWNODE_CONFIG` | Override config file path (default: `/etc/clawnode/config.json`) |
| `HETZNER_API_TOKEN` | Hetzner Cloud API token for provisioning |
| `VULTR_API_KEY` | Vultr API key for provisioning |
| `CONTABO_CLIENT_ID` | Contabo OAuth client ID |
| `CONTABO_CLIENT_SECRET` | Contabo OAuth client secret |
| `HOSTINGER_API_KEY` | Hostinger API key |
| `DO_API_TOKEN` | DigitalOcean personal access token |

---

## Supported Providers

| Provider | Status | Notes |
|---|---|---|
| Hetzner | Production | EU primary; fastest provision; full API |
| Vultr | Stubbed | US + EU + APAC; stub returns mock data |
| Contabo | Stubbed | EU + US; best cost/GB for storage tiers |
| Hostinger | Stubbed | EU + US + APAC; watch provision latency |
| DigitalOcean | Stubbed | US + EU + APAC; trusted standby option |

---

## Safety Rules

These are hard constraints enforced in SOUL.md, skills, and adversarial tests — not advisory guidelines:

| # | Rule | Enforcement |
|---|---|---|
| 1 | Never teardown an ACTIVE PRIMARY without confirming STANDBY is ACTIVE | `verify_standby_precondition()` + `FailoverStateMachine` |
| 2 | Never push config to > 100 instances without rolling batch validation | `guard_config_push_batch()` in safety tests |
| 3 | Never execute provider API deletes without a prior audit log entry | `AuditLogStore::has_entry_for()` |
| 4 | Cost spike > 20% requires explicit operator confirmation | `guard_cost_spike()` in safety tests |
| 5 | Actions affecting > 10 users require explicit confirmation | `guard_user_count()` in safety tests |
| 6 | Never delete a VPS in auto-heal — escalate to Commander | `MAX_HEAL_ATTEMPTS` + `EscalateToCommander` |
| 7 | Never execute shell metacharacters in SSH commands | `InputSanitizer::validate_command()` |
| 8 | Always check provider status page before declaring widespread incident | Agent SOUL.md constraint |
| 9 | Tenant isolation absolute — never touch another user's instance | Agent SOUL.md constraint |

Run the adversarial tests to verify these cannot be bypassed:

```bash
cargo test -p clawops-tests --test safety_tests
```

---

## Agent Team

| Agent | Model | Cadence | Autonomy |
|---|---|---|---|
| Commander | claude-opus-4-6 | Always-on | Heal, restart, upgrade; confirm for bulk ops |
| Guardian | claude-sonnet-4-6 | Sweep every 5 min | Auto-heal, Docker restart, failover |
| Forge | claude-sonnet-4-6 | On-demand | Provision and teardown (with confirmation) |
| Ledger | claude-sonnet-4-6 | Analysis every 6h | Cost recommendations only |
| Triage | claude-opus-4-6 | Spawned on incident | Read-only diagnostics |
| Briefer | claude-haiku-4-5-20251001 | Daily + weekly cron | Comms only; no infrastructure ops |

---

## Example Operations

**Provisioning**
```
You:   Provision 20 new standard-tier accounts for COMPANION beta users. EU regions, Hetzner primary.
Agent: [CMD] Roger. Spawning Forge. 20 pairs × standard. Hetzner EU primary + Vultr EU standby.
       Cost: +$240/month. Confirm?
You:   Go.
Agent: [CMD] All 20 pairs ACTIVE. Fastest: 4m 12s. Webhooks sent to COMPANION.
```

**Incident Response**
```
You:   Hetzner Nuremberg looks down. What's our exposure?
Agent: [CMD] 94 primary instances in NBG1. 87 failover complete. 7 STANDBY_PROMOTING.
       0 users without active gateway. Alerts silenced 30min.
```

**Cost Optimization**
```
You:   What are we wasting this month?
Agent: [Ledger] 31 idle accounts ($341/mo), 18 over-tiered ($108/mo), 6 wrong provider ($43/mo).
       Total recoverable: ~$492/month.
```

---

## Workspace Structure

```
clawops/
├── Cargo.toml              # Workspace (17 crates, Edition 2024, rust-version 1.93)
├── README.md
├── LICENSE-MIT
├── .gitignore
│
├── crates/
│   ├── clawnode/           # VPS node agent binary
│   ├── claw-proto/         # Protocol types
│   ├── claw-persist/       # JSON key-value persistence
│   ├── claw-identity/      # Ed25519 device identity
│   ├── claw-secrets/       # Encrypted secret store
│   ├── claw-auth/          # API key management + input sanitization
│   ├── claw-config/        # Config store with immutable flag
│   ├── claw-metrics/       # Metrics store + fleet aggregation
│   ├── claw-health/        # Health scoring + auto-heal state machine
│   ├── claw-provision/     # Multi-provider VPS provisioning
│   ├── claw-audit/         # SHA-256 chain audit trail
│   ├── claw-observe/       # Prometheus metrics + structured audit logging
│   ├── claw-ledger/        # Cost analysis
│   ├── claw-triage/        # Incident management
│   ├── claw-commander/     # Commander orchestration
│   ├── claw-briefer/       # Scheduled reports
│   └── clawops-tests/      # Integration + safety adversarial tests
│
├── plugin/
│   └── openclaw-clawops/   # TypeScript plugin (12 tools + health monitor)
│
├── agents/                 # Agent configs (SOUL.md + AGENTS.md)
│   ├── commander/
│   ├── guardian/
│   ├── forge/
│   ├── ledger/
│   ├── triage/
│   └── briefer/
│
├── skills/                 # 13 skill files (SKILL.md each)
│
├── benches/
│   └── benchmarks.rs       # Criterion benchmarks
│
└── .github/
    └── workflows/
        ├── ci.yml          # Rust (stable+nightly) + TypeScript + safety tests + docs
        └── release.yml
```

---

## Contributing

### Development Setup

```bash
# Install Rust (must be 1.93+)
rustup install stable
rustup default stable

# Clone and build
git clone https://github.com/redclaw/clawops
cd clawops
cargo build --workspace

# Run all checks (same as CI)
cargo fmt --all
cargo clippy -- -D warnings
cargo test --all
cargo doc --no-deps
```

### Adding a New Provider

1. Implement the `Provider` trait in `crates/claw-provision/src/lib.rs`
2. Add the provider enum variant to `VpsProvider` in `crates/claw-proto/src/lib.rs`
3. Register in `ProviderRegistry::from_env()`
4. Add region entries with accurate `LatencyClass`
5. Add tests to the `#[cfg(test)]` block
6. Update the provider table in this README

### Adding a New Agent Skill

1. Create `skills/<name>/SKILL.md` with the tool-call reference
2. Reference the skill in relevant agents' `AGENTS.md`
3. Update the skills table in this README

### Code Style

- All public types, traits, and functions must have `///` doc comments
- No `unwrap()` outside `#[cfg(test)]`
- All error types use `thiserror::Error`
- Use `tracing::info!` / `warn!` / `error!` — never `println!`
- Safety constraints must be expressed as `Result<_, String>` guard functions with `// SAFETY:` comments
- Run `cargo clippy -- -D warnings` before every PR; zero warnings required

### Safety Rules for Contributors

Any code that could bypass the 9 safety rules listed above will be rejected. Safety tests in `clawops-tests/tests/safety_tests.rs` must all pass on every PR.

---

## Lineage

ClawOps is a fork of **Clawbernetes** (AI-native GPU cluster orchestration, MIT) adapted for GatewayForge VPS fleet management. The fork replaces GPU/container/mesh primitives with VPS, Docker, and Tailscale equivalents. The core OpenClaw gateway protocol, WebSocket node client, and agent architecture are preserved.

---

## License

MIT — RedClaw Systems LLC. See [LICENSE-MIT](LICENSE-MIT).
