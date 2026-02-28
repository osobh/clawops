/**
 * gf_pair_status — Get status of a specific primary/standby gateway pair
 *
 * Used by Forge to monitor provisioning progress, Guardian to check pair
 * health during auto-heal, and Commander to report to operator.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface PairStatusParams {
  /** Account ID whose pair to query */
  accountId: string;
  /** Optional provision request ID (for tracking in-progress provisions) */
  provisionRequestId?: string;
}

export interface PairStatusResult {
  accountId: string;
  pairStatus: 'ACTIVE' | 'DEGRADED' | 'FAILED' | 'BOOTSTRAPPING' | 'CREATING' | 'NO_PAIR';
  primary: InstanceStatusRecord | null;
  standby: InstanceStatusRecord | null;
  lastFailover: FailoverRecord | null;
  slaStatus: {
    uptime30dPct: number;
    breaches30d: number;
    lastBreachAt: string | null;
  };
  // For in-progress provisions
  provision?: {
    requestId: string;
    startedAt: string;
    currentStep: string;
    progressPct: number;
    estimatedCompleteAt: string;
    primaryStatus: string;
    standbyStatus: string;
  };
}

export interface InstanceStatusRecord {
  instanceId: string;
  provider: string;
  region: string;
  tier: string;
  role: 'PRIMARY' | 'STANDBY';
  status: 'ACTIVE' | 'DEGRADED' | 'FAILED' | 'BOOTSTRAPPING' | 'CREATING' | 'STANDBY_PROMOTING';
  healthScore: number;
  ipTailscale: string;
  openclawStatus: string;
  lastHeartbeat: string;
  provisionedAt: string;
  uptimeSeconds: number;
}

export interface FailoverRecord {
  failoverId: string;
  trigger: string;
  oldPrimaryId: string;
  newPrimaryId: string;
  triggeredAt: string;
  completedAt: string;
  durationMs: number;
}

export function pairStatusTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_pair_status',
    description:
      'Get the complete status of a specific primary/standby gateway pair for an account. ' +
      'Returns health scores, current status, last heartbeat, and failover history. ' +
      'Forge calls this to monitor provision progress. ' +
      'Guardian calls this before making failover decisions. ' +
      'Commander calls this when operator asks about a specific account. ' +
      'Example: gf_pair_status({ accountId: "acct-001" }) ' +
      'Example: gf_pair_status({ accountId: "acct-001", provisionRequestId: "req-xyz" })',

    inputSchema: {
      type: 'object',
      properties: {
        accountId: {
          type: 'string',
          description: 'Account ID whose gateway pair to check',
        },
        provisionRequestId: {
          type: 'string',
          description: 'Provision request ID for tracking in-progress provision',
        },
      },
      required: ['accountId'],
    },

    execute: async (params: unknown): Promise<PairStatusResult> => {
      const p = params as PairStatusParams;

      const queryParams = new URLSearchParams();
      if (p.provisionRequestId) {
        queryParams.set('provision_request_id', p.provisionRequestId);
      }

      const response = await axios.get<PairStatusResult>(
        `${config.gfApiBase}/v1/accounts/${encodeURIComponent(p.accountId)}/pair?${queryParams}`,
        {
          headers: { Authorization: `Bearer ${config.gfApiKey}` },
          timeout: 10_000,
        }
      );

      return response.data;
    },
  };
}
