

CLAWOPS
Conversational Infrastructure Operations Platform
GatewayForge  ×  Clawbernetes  ×  OpenClaw
Architecture & Integration PRD  ·  v1.0
RedClaw Systems LLC  ·  Rust 1.93+  ·  Zero Operator Dashboards

1. Executive Summary
ClawOps is the conversational operations layer that sits on top of GatewayForge and the COMPANION platform. Instead of dashboards, alert consoles, and runbooks, operators manage their entire fleet of user OpenClaw gateways by talking to an agent team.
The foundation is Clawbernetes — an open-source, AI-native orchestration system (23 Rust crates, ~74K lines, MIT licensed) that turns OpenClaw into an intelligent infrastructure manager. Clawbernetes already solves the hard problem: connecting OpenClaw nodes to machines, translating natural language into 80+ infrastructure commands across GPU clusters, container orchestration, secrets, autoscaling, networking, and incident response. ClawOps adapts this pattern for the GatewayForge use case — managing a fleet of user VPS instances rather than GPU compute nodes.
🎯 North Star:  You message your ops agent "Provision 50 new accounts, optimize costs, restart anything degraded, and brief me in the morning." You wake up to a voice message summary. Zero dashboards. Zero on-call pages. A team of scalable AI operators handles the fleet 24/7.
This document defines: (1) how Clawbernetes is adopted and adapted for GatewayForge, (2) the agent team architecture with specialized operator roles, (3) the custom skills and plugin design, (4) the conversational interface patterns, and (5) the integration roadmap.

2. Clawbernetes Deep Dive — What We're Building On
2.1 What Clawbernetes Is
Clawbernetes describes itself as "Kubernetes was built for web apps. Clawbernetes is AI-native infrastructure management you talk to." It replaces kubectl + YAML + dashboards with a single conversational interface backed by an OpenClaw agent that reads Skills (SKILL.md files) and calls infrastructure commands via the OpenClaw node protocol.
The architecture is elegantly simple:
OpenClaw Gateway = the control plane (one per operator)
clawnode binary = the agent running on each managed machine
Skills = SKILL.md files the agent reads to understand infrastructure operations
Plugin = TypeScript fleet-level tools (claw_fleet_status, claw_deploy, claw_multi_invoke)
MOLT Network = optional P2P GPU marketplace (Solana SPL tokens, hardware attestation)

2.2 Clawbernetes Crate Inventory — Reuse Analysis

💡 Key Insight:  Roughly 10 of Clawbernetes' 23 crates are directly reusable with minimal changes. The GPU/MOLT/WireGuard crates are irrelevant. This saves ~3–4 weeks of infrastructure plumbing that Clawbernetes has already battle-tested.
2.3 The 80+ Node Commands — What Maps to GatewayForge
Clawbernetes exposes 80+ commands via the OpenClaw WebSocket node protocol. For ClawOps, we replace the GPU/container domain commands with VPS/OpenClaw lifecycle commands while keeping the framework identical:

2.4 The Skills Pattern — How the Agent Knows What to Do
Clawbernetes' Skills are SKILL.md files placed in the OpenClaw workspace. The agent reads them on session start and they teach it: what commands exist, when to use them, diagnostic sequences, escalation criteria, and safety rules. ClawOps uses the same pattern with GatewayForge-specific skills replacing the GPU/container skills.
The 14 Clawbernetes skills map to our needs as follows:


3. ClawOps Agent Team Architecture
3.1 The Operator Agent Team
ClawOps runs a team of specialized OpenClaw agents, each with a distinct role, persona, tool set, and escalation path. They communicate via OpenClaw's sessions_spawn and sessions_send (agent-to-agent ping-pong). The operator interacts primarily with the Commander agent — the others operate autonomously unless surfacing decisions.


3.2 Agent Communication Topology
All agents share a single OpenClaw Gateway but have isolated sessions and workspace directories. The Commander orchestrates via OpenClaw's native agent-to-agent protocol:
Operator → Commander: WhatsApp/Telegram/web chat message
Commander → Forge: sessions_spawn when provisioning work is needed
Commander → Guardian: sessions_send "run health sweep on degraded instances"
Commander → Ledger: sessions_send "generate weekly cost report"
Guardian → Commander: sessions_send "instance {id} unrecoverable, failover triggered"
Briefer → Operator: cron-triggered daily briefing via preferred channel
Triage → Commander: incident report ready for synthesis and delivery

🔁 Agent-to-Agent Protocol:  OpenClaw's sessions_send runs a reply-back ping-pong (reply REPLY_SKIP to stop; max turns via session.agentToAgent.maxPingPongTurns). Agents exchange structured JSON payloads, not prose. Commander then synthesizes into human-readable output for the operator.
3.3 SOUL.md Profiles per Agent
Each agent has a distinct SOUL.md that shapes its behavior:
Commander (CMD)
Identity: Senior SRE with ops authority. Pragmatic, decisive, concise.
Communication: Bullet-point summaries. No fluff. Surface decisions not status.
Autonomy: Can authorize heal, restart, upgrade. Cannot authorize tier change or teardown without confirmation.
Escalation trigger: Any action affecting > 10 users, any data-loss risk, any cost spike > 20%.
Guardian
Identity: Always-on watchdog. Silent unless something needs attention.
Communication: Structured JSON to Commander. Terse one-liners on Slack.
Autonomy: Can execute heal scripts, restart Docker, trigger failover. Cannot delete VPS.
Cadence: Health sweep every 5 minutes. Full report to Commander every hour.
Ledger
Identity: Finance-minded ops analyst. Obsessed with waste elimination.
Communication: Numbers-first. Always includes projected vs actual spend.
Autonomy: Can recommend tier changes. Cannot execute without Commander approval.
Cadence: Cost analysis every 6 hours. Weekly report every Monday 07:00 UTC.
Triage
Identity: On-call incident responder. Methodical, systematic, documents everything.
Communication: Structured incident reports with timeline, root cause, action items.
Autonomy: Read-only diagnostics. All remediation goes through Commander.
Activation: Spawned on-demand only. Not always running.


4. Conversational Interface Patterns
4.1 Fleet Operations — Example Dialogues
Morning Briefing (Autonomous)
Agent:  [Dispatch — 07:00 UTC] Good morning Omar. Fleet status: 847 active pairs across 3 providers. 2 degraded (auto-healing in progress), 1 failed (failover complete, reprovisioning standby). Cost this week: $1,247 vs $1,190 projected (+4.8%). 14 idle accounts flagged for review. No SLA breaches. Full report in your Drive.

On-Demand Provisioning
You:  Provision 20 new standard-tier accounts for COMPANION beta users. Use EU regions, Hetzner primary.
Agent:  [CMD] Roger. Spawning Forge. 20 pairs × standard tier. Hetzner EU primary + Vultr EU standby. Estimated cost: $240/month added. Confirm?
You:  Go.
Agent:  [CMD] Forge is running. Progress: 4/20 ACTIVE, 11/20 BOOTSTRAPPING, 5/20 CREATING. ETA 6 minutes. I'll ping you when all 20 are live.
Agent:  [CMD] All 20 pairs ACTIVE. Fastest provision: 4m 12s. Slowest: 7m 48s (Hetzner Helsinki had a 90s delay). Webhook payloads sent to COMPANION. Forge logged all 20 instance IDs.

Incident Response
You:  Hetzner Nuremberg looks down. What's our exposure?
Agent:  [CMD] Spawning Triage + querying Guardian. Stand by.
Agent:  [CMD] Triage report: 94 primary instances in Nuremberg. Guardian confirms 87 have failover complete (standbys on Vultr/DO now active). 7 pairs in STANDBY_PROMOTING state (ETA 2 min). 0 users without an active gateway. Hetzner status page: "Network issues in Nuremberg — investigating." I've silenced Nuremberg health alerts for 30 min and set reprovisioning queue for affected primaries to fire when Hetzner recovers. You'll get a summary then.

Cost Optimization
You:  What are we wasting money on this month?
Agent:  [Ledger] Three categories: (1) 31 idle accounts (14+ days no activity) — $341/month. Recommend teardown with 30-day S3 archive. (2) 18 accounts on standard tier with nano-level usage (< 20% CPU/RAM all week) — $108/month savings if downsized. (3) 6 Contabo instances where hourly-billed Hetzner would be cheaper at current tenure — $43/month. Total recoverable: ~$492/month.
You:  Downsize the 18, teardown the idle ones, leave Contabo for now.
Agent:  [CMD] Queuing Ledger to execute: 18 tier downsizes (zero-downtime resize ops), 31 teardowns with S3 archive. Will complete within 25 minutes. Expect $449/month reduction on next billing cycle.

Bulk Operations
You:  Push the new OpenClaw config with the Kimi K2.5 model routing to all active instances.
Agent:  [CMD] 847 active instances. Config diff: model.primary changed to moonshot/kimi-k2.5, fallbacks updated. This touches every user's gateway. Rolling push (50 at a time, validate each batch before next)? Or immediate all-at-once?
You:  Rolling. Validate each batch.
Agent:  [CMD] Rolling push started. Batch 1/17 (50 instances): 49 applied, 1 config validation error (instance af3c-uuid — malformed existing config, flagged for manual review). Continuing. Will summarize when complete.

Provider Health / Routing Decision
You:  Hostinger has been slow this week. Should we stop provisioning there?
Agent:  [Ledger] Hostinger 7-day health score: 71/100 (avg provision time 9m 42s vs 4m 12s Hetzner, 2 failed provisions requiring retry). I'd recommend pausing Hostinger as primary for new US accounts — route to Vultr instead. Hostinger stays viable as standby only. Cost impact: +$0.80/user/month on average for Vultr primary vs Hostinger primary. Want me to update the provider selection algorithm config?
You:  Yes, do it.
Agent:  [CMD] Done. Hostinger removed from primary selection pool. Existing Hostinger primaries unaffected. New US accounts will provision Vultr primary + Contabo standby. Config version bumped to v47, applied to GatewayForge control plane.


5. ClawOps Skills Design
Skills are SKILL.md files in the OpenClaw workspace. Each agent loads its assigned skills on session start. Skills are the mechanism by which agents know what's possible, what's safe, and how to sequence operations. They are plain English instructions, not code.
5.1 Master Skill — clawops.md
Purpose: Architecture overview for CMD. Maps user intent to specialist agents.
Contents: Fleet topology description, agent roster + capabilities, escalation matrix, safety rules
Key safety rules encoded in skill:
  → Never teardown a ACTIVE primary without confirming standby is ACTIVE
  → Never push config changes to > 100 instances without rolling validation
  → Never execute provider API deletes without logging to audit trail first
  → Always check provider status page before declaring widespread incident
  → Require explicit confirmation for any action affecting > 10 users
5.2 Skills Inventory

5.3 Sample Skill Excerpt — auto-heal.md
This gives the flavor of skill content — plain English, precise, safety-first:
## Auto-Heal Decision Tree

When Guardian detects an instance with health_score < 50 or missing heartbeat > 3min:

Step 1 — Verify: Call vps.health on the instance. If health_score recovers > 70, log and continue.
Step 2 — Docker restart: SSH to instance via Tailscale. Run: docker compose restart openclaw
Step 3 — Wait 90s. Call openclaw.health. If HTTP 200 received, log HEALED and notify Commander.
Step 4 — If still failing: check if this is the PRIMARY of a pair. If yes, verify STANDBY is ACTIVE.
Step 5 — If standby ACTIVE: trigger failover. Call provision.promote_standby. Notify Commander.
Step 6 — If standby NOT active: CRITICAL — notify Commander immediately. Do not act alone.

NEVER: delete a VPS. NEVER: touch another user's instance. NEVER: skip step 4 verification.


6. OpenClaw Plugin — openclaw-clawops
The ClawOps plugin is a TypeScript OpenClaw plugin (following the Clawbernetes plugin pattern) that registers fleet-level tools the agents call. These tools aggregate data across the GatewayForge API and present it as structured results the agent can reason over.
6.1 Plugin Tools

6.2 Background Service — Fleet Health Monitor
The plugin registers a background service (clawops-health-monitor) that runs continuously in the OpenClaw gateway process. It polls gf_fleet_status every 5 minutes and emits structured events to Guardian when thresholds are crossed. This is the autonomous monitoring heartbeat that enables Guardian to operate without being polled by the operator.
Degraded instance detected → emit INSTANCE_DEGRADED event to Guardian session
Failed pair detected → emit PAIR_FAILED event to Guardian (high priority)
Cost anomaly (> 15% above projection) → emit COST_ANOMALY to Ledger
Provision queue depth > 20 → emit PROVISION_QUEUE_BACKLOG to Provisioner
Provider health score drops below 75 → emit PROVIDER_DEGRADED to Commander

6.3 Plugin Gateway RPC
The plugin exposes a clawops.fleet-status RPC endpoint (following the Clawbernetes pattern of clawbernetes.fleet-status) that the Control UI dashboard can call to get a live fleet snapshot without going through the agent. This is the one case where data is surfaced visually — the operator-facing admin dashboard polls this every 30 seconds for a read-only fleet overview.

7. Full System Integration Architecture
7.1 Component Stack

7.2 Data Flow: Operator Command to Fleet Action
Step 1: Operator sends WhatsApp message to Commander agent via OpenClaw
Step 2: Commander reads clawops.md skill, determines intent and required specialist
Step 3: Commander calls gf_fleet_status plugin tool to get current fleet state
Step 4: Commander spawns/sends to specialist agent (Forge, Guardian, Ledger, etc.)
Step 5: Specialist calls plugin tools (gf_provision, gf_tier_resize, etc.)
Step 6: Plugin tools call GatewayForge REST API (/v1/gateways, /v1/instances, etc.)
Step 7: GatewayForge executes against provider APIs or sends config to instance via Tailscale
Step 8: GatewayForge emits webhook → OpenClaw plugin background service picks up
Step 9: Background service emits event to specialist agent session
Step 10: Specialist reports outcome to Commander via sessions_send
Step 11: Commander synthesizes and delivers result to operator via original channel

⚡ Latency Profile:  Simple queries (fleet status, cost report): ~3–5 seconds. Single instance operations (restart, config push): ~15–30 seconds. Provisioning a pair: ~5–8 minutes (async — agent confirms start immediately, follows up when ACTIVE). Bulk operations (50+ instances): async with progress updates every batch.
7.3 gf-clawnode — The Instance-Side Agent
GatewayForge's per-instance agent is a fork of Clawbernetes' clawnode, adapted for VPS management rather than GPU orchestration. It connects to the ClawOps OpenClaw Gateway over Tailscale WebSocket and handles commands dispatched by the agent team:


8. GatewayForge Workspace — New Crates for ClawOps
The GatewayForge Cargo workspace gains the following crates to support ClawOps:


9. Operator Channel Strategy
9.1 Primary Operator Interfaces
The operator (Omar / RedClaw team) interacts with the Commander agent through multiple channels simultaneously. OpenClaw's multi-channel gateway makes this native:

9.2 Proactive Operator Communications
ClawOps is not reactive-only. The Briefer agent runs scheduled proactive communications so the operator stays informed without polling:
07:00 UTC daily: Voice note via WhatsApp — overnight summary, cost, incidents, actions taken
Monday 08:00 UTC: Weekly cost report via Telegram — provider breakdown, savings executed, recommendations
Real-time: Incident alerts within 60s of unrecoverable failure detection — WhatsApp high-priority
Real-time: Provision failures after 2 retries — Telegram with full error context
Real-time: Cost anomaly > 15% above projection — Telegram with drill-down
Monthly: Capacity planning report — projected user growth vs infrastructure headroom

10. Security Considerations
10.1 Agent Autonomy Boundaries
The agent team has explicit autonomy limits encoded in SOUL.md and skills. These are not advisory — they are hard constraints the agents are trained to never violate regardless of operator instruction:

10.2 OpenClaw Security for Ops Channel
Commander agent's WhatsApp channel: allowFrom restricted to operator phone numbers only
ClawOps gateway has no public exposure — Tailscale-only, same as user gateways
gf-clawnode ssh_exec: strict allowlist of permitted commands; no shell metacharacters
All agent actions logged to audit trail in GatewayForge DB with timestamp and agent identity
Webhook HMAC validation on all GatewayForge → ClawOps plugin events
Agent-to-agent sessions scoped by OpenClaw ACL — Triage cannot reach user agent sessions

11. Integration Roadmap
Phase A — Clawbernetes Adoption (Weeks 1–3)
Fork clawbernetes repo into RedClaw org; audit all 23 crates for reuse vs replace
Add claw-metrics, claw-persist, claw-secrets, claw-identity, claw-auth as workspace dependencies
Scaffold gf-clawnode: replace GPU command handlers with OpenClaw/VPS command set
Write gf-node-proto: Protobuf definitions for VPS-specific node protocol messages
Test gf-clawnode connecting to OpenClaw Gateway, executing openclaw.health and vps.metrics
Phase B — Agent Team Foundation (Weeks 4–7)
Write 5 core skills: clawops.md, vps-fleet.md, gateway-manager.md, auto-heal.md, instance-diagnose.md
Configure Commander agent (SOUL.md, AGENTS.md, skill loading)
Configure Guardian agent with auto-heal skill + background health sweep
Build openclaw-clawops plugin: gf_fleet_status, gf_instance_health, gf_provision, gf_teardown tools
Connect plugin to GatewayForge API (service API key auth)
Test: operator WhatsApp → Commander → gf_fleet_status → fleet report delivered
Phase C — Full Operator Capability (Weeks 8–12)
All 6 specialist agents configured (Forge, Guardian, Ledger, Triage, Briefer, Commander)
All 13 skills written and tested
Full plugin tool set (12 tools) connected to GatewayForge API
Background health monitor service running; Guardian receiving events
Scheduled briefings: daily voice note, weekly Telegram cost report
Incident response tested: kill primary VPS, verify Guardian detects and heals within 3 min
Config push rolling test: push to 100 instances, validate batch-by-batch
Phase D — Hardening & Scale (Weeks 13–16)
All safety constraints battle-tested with adversarial prompts
Agent memory: Briefer maintains incident log in memory/ for pattern recognition
Ledger learns provider performance patterns from 30-day history
Multi-operator support: Discord channel for engineering team visibility
Voice briefing fully operational (ElevenLabs via sag plugin)
Clawbernetes upstream contributions: submit generic VPS node protocol improvements back to project

12. Open Questions


— End of Document —
