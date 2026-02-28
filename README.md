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

## Workspace Structure

```
clawops/
├── Cargo.toml                    # Rust workspace (7 crates)
├── README.md
├── .gitignore
│
├── gf-clawnode/                  # Instance-side agent binary (VPS-level)
├── gf-node-proto/                # Protobuf definitions for node protocol
├── gf-provision/                 # Multi-provider provisioning logic
├── gf-health/                    # Health check and auto-heal logic
├── gf-failover/                  # Primary/standby failover orchestration
├── gf-metrics/                   # Fleet metrics collection + aggregation
├── gf-audit/                     # Audit trail logging for all agent actions
│
├── plugin/                       # TypeScript OpenClaw plugin
│   └── src/
│       ├── index.ts              # Plugin entry, registers all tools
│       ├── tools/                # 12 fleet-level tool implementations
│       └── services/
│           └── health-monitor.ts # Background fleet health service
│
├── agents/                       # Agent configurations
│   ├── commander/                # SOUL.md + AGENTS.md
│   ├── guardian/
│   ├── forge/
│   ├── ledger/
│   ├── triage/
│   └── briefer/
│
└── skills/                       # Skill files loaded by agents
    ├── clawops/                  # Master skill for Commander
    ├── vps-fleet/
    ├── gateway-manager/
    ├── auto-heal/
    ├── instance-diagnose/
    ├── provision/
    ├── failover/
    ├── config-push/
    ├── cost-analysis/
    ├── incident-response/
    ├── provider-health/
    ├── capacity-planning/
    └── security-audit/
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
cd plugin
npm install
npm run build
```

### Deploy gf-clawnode to a VPS

```bash
# Copy binary to VPS, then:
export INSTANCE_ID="<uuid>"
export GATEWAY_URL="https://<tailscale-hostname>:8443"
export GF_API_KEY="<service-key>"
export NODE_REGION="eu-hetzner-nbg1"
export NODE_TIER="standard"
./gf-clawnode
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
