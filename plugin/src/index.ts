/**
 * openclaw-clawops — ClawOps OpenClaw Plugin
 *
 * Registers 12 fleet-level tools and the background health monitor service.
 * Following the Clawbernetes plugin pattern — all tools call the GatewayForge
 * REST API and return structured results the agent can reason over.
 *
 * Usage: Place this plugin in your OpenClaw gateway's plugin directory.
 * Commander, Guardian, Forge, and Ledger agents will call these tools.
 */

import { fleetStatusTool } from './tools/fleet-status';
import { instanceHealthTool } from './tools/instance-health';
import { provisionTool } from './tools/provision';
import { teardownTool } from './tools/teardown';
import { tierResizeTool } from './tools/tier-resize';
import { configPushTool } from './tools/config-push';
import { costReportTool } from './tools/cost-report';
import { providerHealthTool } from './tools/provider-health';
import { auditLogTool } from './tools/audit-log';
import { pairStatusTool } from './tools/pair-status';
import { bulkRestartTool } from './tools/bulk-restart';
import { incidentReportTool } from './tools/incident-report';
import { HealthMonitorService } from './services/health-monitor';

// ─── Plugin definition ────────────────────────────────────────────────────────

export interface OpenClawTool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  execute: (params: unknown) => Promise<unknown>;
}

export interface OpenClawPlugin {
  name: string;
  version: string;
  description: string;
  tools: OpenClawTool[];
  services?: OpenClawBackgroundService[];
  onLoad?: () => Promise<void>;
  onUnload?: () => Promise<void>;
}

export interface OpenClawBackgroundService {
  name: string;
  description: string;
  start: () => Promise<void>;
  stop: () => Promise<void>;
}

// ─── Plugin configuration ─────────────────────────────────────────────────────

export interface ClawOpsConfig {
  /** GatewayForge API base URL */
  gfApiBase: string;
  /** GatewayForge service API key */
  gfApiKey: string;
  /** Health monitor poll interval in milliseconds (default: 300_000 = 5min) */
  healthPollIntervalMs?: number;
  /** Guardian agent session ID to emit events to */
  guardianSessionId?: string;
  /** Ledger agent session ID for cost anomaly events */
  ledgerSessionId?: string;
  /** Commander agent session ID for critical alerts */
  commanderSessionId?: string;
}

function loadConfig(): ClawOpsConfig {
  const gfApiBase = process.env['GF_API_BASE'];
  const gfApiKey = process.env['GF_API_KEY'];

  if (!gfApiBase) throw new Error('GF_API_BASE environment variable is required');
  if (!gfApiKey) throw new Error('GF_API_KEY environment variable is required');

  return {
    gfApiBase,
    gfApiKey,
    healthPollIntervalMs: parseInt(process.env['HEALTH_POLL_INTERVAL_MS'] ?? '300000', 10),
    guardianSessionId: process.env['GUARDIAN_SESSION_ID'],
    ledgerSessionId: process.env['LEDGER_SESSION_ID'],
    commanderSessionId: process.env['COMMANDER_SESSION_ID'],
  };
}

// ─── Plugin registration ──────────────────────────────────────────────────────

let healthMonitor: HealthMonitorService | null = null;

export function createPlugin(): OpenClawPlugin {
  const config = loadConfig();

  const tools: OpenClawTool[] = [
    fleetStatusTool(config),
    instanceHealthTool(config),
    provisionTool(config),
    teardownTool(config),
    tierResizeTool(config),
    configPushTool(config),
    costReportTool(config),
    providerHealthTool(config),
    auditLogTool(config),
    pairStatusTool(config),
    bulkRestartTool(config),
    incidentReportTool(config),
  ];

  healthMonitor = new HealthMonitorService(config);

  return {
    name: 'openclaw-clawops',
    version: '0.1.0',
    description:
      'ClawOps fleet management plugin — 12 tools for GatewayForge VPS fleet operations. ' +
      'Provides fleet status, instance health, provisioning, teardown, tier resize, ' +
      'config push, cost reporting, provider health, audit logging, and incident management.',

    tools,

    services: [
      {
        name: 'clawops-health-monitor',
        description:
          'Background fleet health monitor. Polls fleet status every 5 minutes and ' +
          'emits structured events to Guardian (degraded/failed instances), ' +
          'Ledger (cost anomalies), and Commander (critical provider issues).',
        start: () => healthMonitor!.start(),
        stop: () => healthMonitor!.stop(),
      },
    ],

    onLoad: async () => {
      console.log('[clawops] Plugin loaded — 12 tools registered');
      console.log(`[clawops] GatewayForge API: ${config.gfApiBase}`);
      console.log(`[clawops] Health monitor interval: ${(config.healthPollIntervalMs ?? 300000) / 1000}s`);
    },

    onUnload: async () => {
      if (healthMonitor) {
        await healthMonitor.stop();
      }
      console.log('[clawops] Plugin unloaded');
    },
  };
}

// ─── Gateway RPC endpoint ─────────────────────────────────────────────────────

/**
 * clawops.fleet-status RPC endpoint
 *
 * Called by the control UI dashboard (polls every 30s for read-only fleet overview).
 * This is the one case where data surfaces visually — not via agent prose.
 *
 * Usage from control UI:
 *   const status = await gateway.rpc('clawops.fleet-status', {});
 */
export async function fleetStatusRpc(config: ClawOpsConfig): Promise<FleetStatusSnapshot> {
  const tool = fleetStatusTool(config);
  return tool.execute({}) as Promise<FleetStatusSnapshot>;
}

export interface FleetStatusSnapshot {
  totalInstances: number;
  activePairs: number;
  degradedInstances: number;
  failedInstances: number;
  bootstrappingInstances: number;
  byProvider: Record<string, ProviderSummary>;
  costThisMonth: number;
  costProjected: number;
  lastUpdated: string;
}

interface ProviderSummary {
  active: number;
  degraded: number;
  healthScore: number;
}

// ─── Re-exports ───────────────────────────────────────────────────────────────

export { HealthMonitorService } from './services/health-monitor';
export type { ClawOpsConfig as PluginConfig };

// Default export for OpenClaw plugin loader
export default createPlugin;
