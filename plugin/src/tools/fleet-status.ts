/**
 * gf_fleet_status — Get a complete fleet status snapshot
 *
 * Called by Commander on every operator interaction that requires fleet context.
 * Also polled by Guardian's health sweep to identify degraded instances.
 * Returns structured data the agent uses to form natural language responses.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface FleetStatusParams {
  /** Filter by provider (optional) */
  provider?: 'hetzner' | 'vultr' | 'contabo' | 'hostinger' | 'digitalocean';
  /** Filter by status (optional) */
  status?: 'ACTIVE' | 'DEGRADED' | 'FAILED' | 'BOOTSTRAPPING' | 'CREATING';
  /** Include cost metrics (default: true) */
  includeCost?: boolean;
  /** Include per-instance detail or summary only (default: summary) */
  detail?: 'summary' | 'full';
}

export interface InstanceRecord {
  instanceId: string;
  accountId: string;
  provider: string;
  region: string;
  tier: string;
  role: 'PRIMARY' | 'STANDBY';
  status: string;
  healthScore: number;
  pairInstanceId: string | null;
  lastHeartbeat: string;
  ipTailscale: string;
  provisionedAt: string;
  monthlyCostUsd: number;
}

export interface FleetStatusResult {
  snapshotId: string;
  capturedAt: string;
  // Counts
  totalInstances: number;
  activePairs: number;
  degradedInstances: number;
  failedInstances: number;
  bootstrappingInstances: number;
  unpaired: number;
  // By provider
  byProvider: Record<string, {
    total: number;
    active: number;
    degraded: number;
    failed: number;
    healthScore: number;
    avgProvisionTimeSecs: number;
    monthlyCostUsd: number;
  }>;
  // By tier
  byTier: Record<string, {
    count: number;
    monthlyCostUsd: number;
    avgCpuUsagePct: number;
    avgMemUsagePct: number;
  }>;
  // Cost
  cost: {
    monthlyActualUsd: number;
    monthlyProjectedUsd: number;
    deviationPct: number;
    weeklyActualUsd: number;
    perActiveAccountUsd: number;
  };
  // Instances (only in full detail mode)
  instances?: InstanceRecord[];
  // Alerts requiring attention
  alerts: Array<{
    severity: 'info' | 'warning' | 'critical';
    instanceId?: string;
    message: string;
  }>;
}

export function fleetStatusTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_fleet_status',
    description:
      'Get a complete GatewayForge fleet status snapshot. Returns active/degraded/failed ' +
      'instance counts, provider breakdown, tier distribution, cost metrics, and any ' +
      'active alerts. Use this at the start of every operator interaction. ' +
      'Example: gf_fleet_status({}) → fleet summary. ' +
      'gf_fleet_status({ provider: "hetzner", detail: "full" }) → all Hetzner instances.',

    inputSchema: {
      type: 'object',
      properties: {
        provider: {
          type: 'string',
          enum: ['hetzner', 'vultr', 'contabo', 'hostinger', 'digitalocean'],
          description: 'Filter by specific provider',
        },
        status: {
          type: 'string',
          enum: ['ACTIVE', 'DEGRADED', 'FAILED', 'BOOTSTRAPPING', 'CREATING'],
          description: 'Filter by instance status',
        },
        includeCost: {
          type: 'boolean',
          description: 'Include cost metrics (default: true)',
        },
        detail: {
          type: 'string',
          enum: ['summary', 'full'],
          description: 'Return summary counts or full instance list (default: summary)',
        },
      },
      required: [],
    },

    execute: async (params: unknown): Promise<FleetStatusResult> => {
      const p = (params as FleetStatusParams) ?? {};

      const queryParams = new URLSearchParams();
      if (p.provider) queryParams.set('provider', p.provider);
      if (p.status) queryParams.set('status', p.status);
      if (p.includeCost === false) queryParams.set('include_cost', 'false');
      if (p.detail === 'full') queryParams.set('detail', 'full');

      const response = await axios.get<FleetStatusResult>(
        `${config.gfApiBase}/v1/fleet/status?${queryParams}`,
        {
          headers: {
            Authorization: `Bearer ${config.gfApiKey}`,
            'Content-Type': 'application/json',
          },
          timeout: 10_000,
        }
      );

      return response.data;
    },
  };
}
