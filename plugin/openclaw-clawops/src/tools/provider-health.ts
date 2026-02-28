/**
 * gf_provider_health — Get health scores and performance metrics for all providers
 *
 * Used by Commander and Ledger to make provider selection decisions.
 * Also called before new provisions to verify the selected provider is healthy.
 * Checks provider status pages, recent provision success rates, and API latency.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface ProviderHealthParams {
  /** Specific provider to check (omit for all providers) */
  provider?: 'hetzner' | 'vultr' | 'contabo' | 'hostinger' | 'digitalocean';
  /** Include 7-day historical performance data */
  includeHistory?: boolean;
}

export interface ProviderHealthRecord {
  provider: string;
  displayName: string;
  // Current status
  apiReachable: boolean;
  healthScore: number; // 0–100
  recommendation: 'primary' | 'primary_ok' | 'standby_only' | 'pause' | 'emergency';
  activeIncident: boolean;
  incidentDescription: string | null;
  incidentUrl: string | null;
  // Performance (rolling 7 days)
  avgProvisionTimeSecs: number;
  provisionSuccessRate7d: number; // 0.0–1.0
  avgUptimePct7d: number;
  incidentCount7d: number;
  apiLatencyMs: number;
  // Quota
  quotaUsedPct: number;
  quotaLimit: number;
  quotaUsed: number;
  // Regions
  regions: Array<{
    id: string;
    displayName: string;
    available: boolean;
    activeInstances: number;
    incident: boolean;
  }>;
  // Historical
  history?: Array<{
    date: string;
    healthScore: number;
    avgProvisionTimeSecs: number;
    successRate: number;
    incidents: number;
  }>;
  // Cost efficiency
  avgCostPerInstanceUsd: number;
  costEfficiencyScore: number; // performance per dollar, 0–100
  checkedAt: string;
}

export interface ProviderHealthResult {
  providers: ProviderHealthRecord[];
  overallFleetHealthy: boolean;
  recommendedPrimaryProvider: string;
  recommendedStandbyProvider: string;
  providerAlerts: Array<{
    provider: string;
    severity: 'info' | 'warning' | 'critical';
    message: string;
  }>;
  generatedAt: string;
}

export function providerHealthTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_provider_health',
    description:
      'Get health scores and performance metrics for all VPS providers (or a specific one). ' +
      'Returns health score (0–100), provision success rate, average provision time, ' +
      'active incidents, and a recommendation: primary/standby_only/pause/emergency. ' +
      'Commander calls this when operator asks about a provider. ' +
      'Forge calls this before every provision to verify provider is healthy (score >= 75). ' +
      'Example: gf_provider_health({}) → all providers ranked. ' +
      'Example: gf_provider_health({ provider: "hostinger" }) → Hostinger 7-day report.',

    inputSchema: {
      type: 'object',
      properties: {
        provider: {
          type: 'string',
          enum: ['hetzner', 'vultr', 'contabo', 'hostinger', 'digitalocean'],
          description: 'Specific provider to check (omit for all)',
        },
        includeHistory: {
          type: 'boolean',
          description: 'Include 7-day daily breakdown (default: false)',
        },
      },
      required: [],
    },

    execute: async (params: unknown): Promise<ProviderHealthResult> => {
      const p = (params as ProviderHealthParams) ?? {};

      const queryParams = new URLSearchParams();
      if (p.provider) queryParams.set('provider', p.provider);
      if (p.includeHistory) queryParams.set('include_history', 'true');

      const response = await axios.get<ProviderHealthResult>(
        `${config.gfApiBase}/v1/providers/health?${queryParams}`,
        {
          headers: { Authorization: `Bearer ${config.gfApiKey}` },
          timeout: 15_000,
        }
      );

      return response.data;
    },
  };
}
