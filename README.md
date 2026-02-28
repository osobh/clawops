# ClawOps

**Conversational Infrastructure Operations Platform**
GatewayForge × Clawbernetes × OpenClaw
_RedClaw Systems LLC · Rust 1.93+ · Zero Operator Dashboards_

> "You message your ops agent. You wake up to a voice summary. Zero dashboards. Zero on-call pages."

---

## Overview

ClawOps is the conversational operations layer that sits on top of **GatewayForge** and the **COMPANION platform**. Instead of dashboards, alert consoles, and runbooks, operators manage their entire fleet of user OpenClaw gateways by talking to an agent team.

The foundation is **Clawbernetes** — an open-source, AI-native orchestration system (~74K lines, MIT) that turns OpenClaw into an intelligent infrastructure manager. ClawOps adapts this pattern for the GatewayForge use case: managing a fleet of user VPS instances rather than GPU compute nodes.

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
    ↓
Commander Agent  ←──── Senior SRE persona, orchestrates all ops
    ├── Guardian  ─────── Always-on watchdog, silent unless needed
    ├── Forge     ─────── Provisioning specialist (5 providers)
    ├── Ledger    ─────── Finance-minded ops analyst
    ├── Triage    ─────── On-call incident responder (spawned on-demand)
    └── Briefer   ─────── Scheduled reports + voice briefings

ClawOps Plugin (TypeScript — 12 fleet tools)
    └──→ GatewayForge REST API (/v1/gateways, /v1/instances, ...)
              └──→ Provider APIs (Hetzner, Vultr, Contabo, Hostinger, DO)

gf-clawnode (Rust binary — one per VPS instance)
    └──→ OpenClaw Gateway (Tailscale WebSocket)
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

## Supported Providers

| Provider      | Primary | Standby | Notes                                    |
|---------------|---------|---------|------------------------------------------|
| Hetzner       | ✅      | ✅      | EU primary preference; fastest provision |
| Vultr         | ✅      | ✅      | US + EU + APAC; reliable failover        |
| Contabo       | ✅      | ✅      | EU + US; best cost/GB for storage tiers  |
| Hostinger     | ✅      | ✅      | EU + US + APAC; watch provision latency  |
| DigitalOcean  | ✅      | ✅      | US + EU + APAC; trusted standby option   |

---

## Example Operations

**On-Demand Provisioning**
```
You:   Provision 20 new standard-tier accounts for COMPANION beta users. EU regions, Hetzner primary.
Agent: [CMD] Roger. Spawning Forge. 20 pairs × standard tier. Hetzner EU primary + Vultr EU standby.
       Estimated cost: $240/month added. Confirm?
You:   Go.
Agent: [CMD] Forge running. Progress: 4/20 ACTIVE, 11/20 BOOTSTRAPPING, 5/20 CREATING. ETA 6min.
Agent: [CMD] All 20 pairs ACTIVE. Fastest: 4m 12s. Webhooks sent to COMPANION.
```

**Incident Response**
```
You:   Hetzner Nuremberg looks down. What's our exposure?
Agent: [CMD] Spawning Triage + querying Guardian. Stand by.
Agent: [CMD] 94 primary instances in Nuremberg. 87 failover complete (standbys on Vultr/DO active).
       7 pairs in STANDBY_PROMOTING (ETA 2min). 0 users without an active gateway.
       Nuremberg health alerts silenced 30min. Reprovisioning queue armed for recovery.
```

**Cost Optimization**
```
You:   What are we wasting money on this month?
Agent: [Ledger] (1) 31 idle accounts — $341/month. Recommend teardown + S3 archive.
                (2) 18 accounts on standard tier with nano usage — $108/month if downsized.
                (3) 6 Contabo where hourly Hetzner would be cheaper — $43/month.
                Total recoverable: ~$492/month.
You:   Downsize the 18, teardown the idle ones, leave Contabo.
Agent: [CMD] Queuing: 18 tier downsizes, 31 teardowns with archive. ~$449/month reduction.
```

---

## Lineage

ClawOps is a fork of **Clawbernetes** (AI-native GPU cluster orchestration, MIT) adapted for GatewayForge VPS fleet management. The fork replaces GPU/container/mesh primitives with VPS, Docker, and Tailscale equivalents. The core OpenClaw gateway protocol, WebSocket node client, and agent architecture are preserved.

## Workspace Structure

```
clawops/
├── Cargo.toml                    # Rust workspace (11 crates, Edition 2024, rust-version 1.93)
├── README.md
├── .gitignore
│
├── crates/
│   ├── clawnode/                 # VPS node agent binary (WebSocket → OpenClaw gateway)
│   ├── claw-proto/               # Protocol types (HealthReport, InstanceState, VpsProvider, etc.)
│   ├── claw-persist/             # JSON file-backed key-value persistence (JsonStore)
│   ├── claw-identity/            # Ed25519 device identity + challenge-response signing
│   ├── claw-secrets/             # Encrypted secret store with key rotation
│   ├── claw-auth/                # ApiKeyStore + AuditLogStore (SHA-256 hashed keys)
│   ├── claw-config/              # ConfigStore with immutable flag support
│   ├── claw-metrics/             # Push-based metrics store with retention policy
│   ├── claw-health/              # Health scoring (0–100), auto-heal decisions, failover
│   ├── claw-provision/           # Multi-provider VPS provisioning (Hetzner, Vultr, etc.)
│   └── claw-audit/               # Immutable append-only SHA-256 chain audit trail
│
├── plugin/
│   └── openclaw-clawops/         # TypeScript OpenClaw plugin (12 tools + health monitor)
│       ├── src/
│       │   ├── index.ts          # Plugin entry — createPlugin() factory, tool registration
│       │   ├── tools/            # 12 fleet-level tool implementations
│       │   └── services/
│       │       └── health-monitor.ts  # Background fleet health monitor (5-min poll)
│       ├── openclaw.plugin.json  # Plugin manifest with safety rules
│       ├── package.json
│       └── tsconfig.json
│
├── agents/                       # Agent configurations (SOUL.md + AGENTS.md each)
│   ├── commander/                # Senior SRE orchestrator — always-on, claude-opus-4-6
│   ├── guardian/                 # Health watchdog — always-on, claude-sonnet-4-6
│   ├── forge/                    # Provisioner — ephemeral, claude-sonnet-4-6
│   ├── ledger/                   # Cost analyst — always-on, claude-sonnet-4-6
│   ├── triage/                   # Incident responder — ephemeral, claude-opus-4-6
│   └── briefer/                  # Reporter — cron-triggered, claude-haiku-4-5
│
├── skills/                       # Skill files (SKILL.md each, loaded by agents)
│   ├── clawops/                  # Master skill — full plugin tool reference table
│   ├── vps-fleet/                # Fleet topology, instance states, health thresholds
│   ├── gateway-manager/          # OpenClaw lifecycle + config push process
│   ├── auto-heal/                # 6-step heal decision tree with plugin tool calls
│   ├── instance-diagnose/        # Diagnostic sequences for common failures
│   ├── provision/                # Provisioning workflows with exact tool call sequences
│   ├── failover/                 # Failover orchestration with standby verification
│   ├── config-push/              # Config deployment with rolling validation
│   ├── cost-analysis/            # Cost framework + waste identification
│   ├── incident-response/        # Incident management playbooks
│   ├── provider-health/          # Provider status monitoring
│   ├── capacity-planning/        # Growth projections and headroom
│   └── security-audit/           # Security constraints and audit requirements
│
└── .github/
    └── workflows/
        └── ci.yml                # CI: Rust (stable+nightly) + TypeScript type check
```

---

## Getting Started

### Prerequisites

- Rust 1.93+
- Node.js 20+ / npm 10+
- OpenClaw gateway configured and running
- GatewayForge API access (service API key)
- Tailscale network configured across all VPS instances

### Build Rust Workspace

```bash
cargo build --workspace
```

### Build Plugin

```bash
cd plugin/openclaw-clawops
npm install
npm run build
# Type check only (no output):
npx tsc --noEmit
```

### Deploy clawnode to a VPS

```bash
# Build the release binary first:
cargo build --release --package clawnode

# Copy binary to VPS, configure with a JSON config file:
cat > /etc/clawnode/config.json << 'EOF'
{
  "gateway": "wss://gateway.example.com",
  "token": "<gateway-api-token>",
  "hostname": "vps-acct-001-primary",
  "provider": "hetzner",
  "region": "eu-hetzner-nbg1",
  "tier": "standard",
  "role": "primary",
  "account_id": "acct-001",
  "state_path": "/var/lib/clawnode"
}
EOF

# Run the agent:
clawnode run --config /etc/clawnode/config.json
```

### Configure Agents

Place each agent's `SOUL.md` and `AGENTS.md` in the corresponding OpenClaw workspace directory, then start the agent session.

---

## Agent Team

| Agent     | Persona                    | Autonomy                                  | Cadence            |
|-----------|----------------------------|-------------------------------------------|--------------------|
| Commander | Senior SRE orchestrator    | Heal, restart, upgrade; confirm for rest  | Always-on          |
| Guardian  | Always-on health watchdog  | Heal scripts, restart Docker, failover    | Sweep every 5 min  |
| Forge     | Provisioning specialist    | Provision and teardown (with confirm)     | On-demand          |
| Ledger    | Finance-minded analyst     | Recommend tier changes (no exec alone)   | Analysis every 6h  |
| Triage    | On-call incident responder | Read-only diagnostics only               | Spawned on-demand  |
| Briefer   | Scheduled reporter         | Comms only; no infrastructure ops        | Daily + weekly     |

---

## Safety Rules

These are hard constraints encoded in SOUL.md and skills — not advisory guidelines:

- **Never** teardown an ACTIVE primary without confirming standby is ACTIVE
- **Never** push config changes to > 100 instances without rolling batch validation
- **Never** execute provider API deletes without logging to audit trail first
- **Always** check provider status page before declaring a widespread incident
- **Require** explicit operator confirmation for any action affecting > 10 users
- **Never** touch another user's instance (tenant isolation absolute)
- **Never** delete a VPS in the auto-heal sequence — escalate to Commander

---

## Integration Roadmap

| Phase | Weeks | Milestone |
|-------|-------|-----------|
| A | 1–3   | Clawbernetes adoption, gf-clawnode + gf-node-proto |
| B | 4–7   | Core agent team, 5 skills, plugin + GatewayForge API |
| C | 8–12  | Full 6 agents, 13 skills, 12 tools, background monitor |
| D | 13–16 | Hardening, agent memory, multi-operator, voice briefings |

---

## License

MIT — RedClaw Systems LLC
