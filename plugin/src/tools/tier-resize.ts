/**
 * gf_tier_resize — Resize an account's VPS instances to a different tier
 *
 * Used by Ledger's recommendations and Commander's approval flow.
 * Supports zero-downtime resize on providers that allow live upgrade (Hetzner).
 * Requires brief (~60s) downtime on providers requiring stop/start (Vultr, Contabo).
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export type InstanceTier = 'nano' | 'standard' | 'pro' | 'enterprise';

export interface TierResizeParams {
  /** Account ID to resize */
  accountId: string;
  /** Target tier */
  newTier: InstanceTier;
  /**
   * Resize both primary and standby (default: true — always keep pair consistent).
   */
  resizeBoth?: boolean;
  /** Operator confirmation token — required for any tier change */
  confirmationToken: string;
  /** If true, only do a dry-run and return cost/downtime estimate */
  dryRun?: boolean;
}

export interface TierResizeResult {
  requestId: string;
  accountId: string;
  previousTier: InstanceTier;
  newTier: InstanceTier;
  primaryResized: boolean;
  standbyResized: boolean;
  downtimeSeconds: number;  // 0 if live resize (Hetzner upgrade)
  monthlySavingsUsd: number; // negative if upgrading
  status: 'COMPLETE' | 'IN_PROGRESS' | 'FAILED' | 'DRY_RUN';
  dryRunEstimate?: {
    downtimeSeconds: number;
    monthlyCostChangUsd: number;
    liveMigration: boolean;
    notes: string;
  };
}

export function tierResizeTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_tier_resize',
    description:
      'Resize a user account\'s VPS instances to a different service tier. ' +
      'Use for cost optimization (downgrade idle accounts) or capacity upgrades. ' +
      'Hetzner supports live resize (zero downtime for upgrades). ' +
      'Vultr/Contabo require ~60s downtime for any resize. ' +
      'Use dryRun=true to estimate cost/downtime before committing. ' +
      'SAFETY: confirmationToken required. Ledger recommends, Commander approves. ' +
      'Example: gf_tier_resize({ accountId: "acct-001", newTier: "nano", ' +
      'confirmationToken: "tok_xyz" })',

    inputSchema: {
      type: 'object',
      properties: {
        accountId: {
          type: 'string',
          description: 'Account ID to resize',
        },
        newTier: {
          type: 'string',
          enum: ['nano', 'standard', 'pro', 'enterprise'],
          description: 'Target service tier',
        },
        resizeBoth: {
          type: 'boolean',
          description: 'Resize both primary and standby (default: true)',
        },
        confirmationToken: {
          type: 'string',
          description: 'Operator confirmation token — required for tier changes',
        },
        dryRun: {
          type: 'boolean',
          description: 'If true, return estimate without executing resize',
        },
      },
      required: ['accountId', 'newTier', 'confirmationToken'],
    },

    execute: async (params: unknown): Promise<TierResizeResult> => {
      const p = params as TierResizeParams;

      const response = await axios.post<TierResizeResult>(
        `${config.gfApiBase}/v1/accounts/${encodeURIComponent(p.accountId)}/resize`,
        {
          new_tier: p.newTier,
          resize_both: p.resizeBoth ?? true,
          confirmation_token: p.confirmationToken,
          dry_run: p.dryRun ?? false,
        },
        {
          headers: {
            Authorization: `Bearer ${config.gfApiKey}`,
            'Content-Type': 'application/json',
          },
          timeout: 15_000,
        }
      );

      return response.data;
    },
  };
}
