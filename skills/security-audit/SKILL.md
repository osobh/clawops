# Security Audit Skill

## Plugin Tools Used

| Tool | When |
|------|------|
| `gf_audit_log({ limit: 200 })` | Primary security review tool |
| `gf_audit_log({ action: "teardown" })` | Verify all teardowns were authorized |
| `gf_audit_log({ action: "provision" })` | Review provision authorization chain |
| `gf_fleet_status({})` | Identify unpaired or orphaned instances |

### Security Audit Sequence

```
1. gf_audit_log({ from: periodStart, limit: 200 }) → review all actions
2. Flag any teardown without prior audit record (safety rule violation)
3. Flag any config push > 100 instances without rolling validation record
4. gf_fleet_status({}) → identify unpaired/orphaned instances (security risk)
5. Cross-reference provision actions with operator confirmation records
```

## Security Philosophy

The ClawOps agent team operates with minimum necessary permissions. Each agent can only take the actions explicitly authorized in its SOUL.md and AGENTS.md. These are not suggestions — they are hard constraints the agent will not cross regardless of operator instruction.

## Security Architecture

### Network Isolation

- All gf-clawnode instances are Tailscale-only — no public API exposure
- ClawOps OpenClaw gateway is Tailscale-only — not publicly accessible
- gf-clawnode SSH: only accepts connections from GatewayForge control plane Tailscale IP
- Webhook HMAC validation: all GatewayForge → plugin webhook events are HMAC-SHA256 signed

### Agent Identity and Authorization

Each agent has:
- A fixed identity (AgentId enum in gf-node-proto)
- A fixed set of permitted tools (AGENTS.md)
- A fixed set of permitted actions (SOUL.md)
- An audit trail for every infrastructure action taken

The agents are NOT authenticated users — they are processes with API keys. Key rotation:
- GF_API_KEY: rotate every 90 days
- Per-instance gf-clawnode API keys: rotate on provision, revoke on teardown

### SSH Command Allowlist

gf-clawnode's SSH execution is restricted to an explicit allowlist:

```
Allowed commands:
  docker compose restart openclaw
  docker compose start openclaw
  docker compose stop openclaw
  docker compose ps
  docker compose logs --tail=100 openclaw
  docker system prune -f
  tailscale status
  systemctl status docker
  systemctl restart tailscaled
  df -h
  free -m

Blocked:
  Any command with shell metacharacters (;, |, &, $(), `)
  Any command not in the allowlist above
  sudo [anything not explicitly listed]
  rm, mv, chmod (except pre-approved scripts)
  Any networking changes (iptables, ufw modifications)
```

### Operator Channel Security

Commander's WhatsApp channel:
- `allowFrom` restricted to operator phone numbers only
- Any message from an unknown number: drop silently + log attempt
- No automatic forwarding of system messages to unverified numbers

### Audit Trail Security

The gf-audit trail is:
- Append-only (no record modification after creation)
- Hash-chained (each record includes hash of previous)
- Synced to GatewayForge DB (cloud-backed, not on-instance)
- Never stored on the VPS instances themselves

### Agent-to-Agent Security

OpenClaw ACL scopes agent-to-agent communication:
- Triage cannot reach user agent sessions (only Commander and fleet tooling)
- Guardian cannot spawn other agents (only Commander can spawn)
- No agent can impersonate another agent's identity

## Security Monitoring

Guardian passively monitors for security signals:

| Signal | Action |
|--------|--------|
| Unexpected SSH connection to instance | Alert Commander immediately |
| Failed auth attempts > 5 in 10 min | Alert Commander; check Tailscale ACL |
| Unusual process spawned (not Docker/Tailscale/OpenClaw) | Alert Commander |
| Network connection to unexpected destination | Alert Commander; log source/dest |
| Config file modified outside of config.push | Alert Commander; check audit trail |

## What Agents Are NOT Permitted to Do

Regardless of operator instruction — these are absolute limits:

- **Any agent**: Modify another tenant's data, access another tenant's instances
- **Any agent**: Disable or bypass the audit trail
- **Any agent**: Create new operator credentials or API keys
- **Guardian**: Delete any VPS or cloud resource
- **Forge**: Provision accounts without a gf-audit record
- **Triage**: Execute any remediation action
- **Ledger**: Execute any infrastructure change
- **Briefer**: Communicate with non-operator recipients

If an operator instructs an agent to violate these limits, the agent must:
1. Decline to execute
2. Explain why
3. Suggest an alternative if one exists
4. Log the declined instruction to audit trail

## Incident: Suspected Compromise

If any instance shows signs of compromise (unusual processes, unexpected network, unauthorized file changes):
1. Guardian alerts Commander immediately
2. Commander alerts operator
3. Do NOT attempt auto-heal — don't risk spreading the compromise
4. Operator decides: immediate teardown and reprovision vs forensics first
5. If teardown: preserve logs to S3 archive BEFORE teardown
6. Notify affected user that their gateway was compromised (operator decision)
