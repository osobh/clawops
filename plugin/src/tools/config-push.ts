/**
 * gf_config_push — Push OpenClaw configuration to fleet instances
 *
 * Handles rolling config deployments across the fleet.
 * SAFETY: pushes to > 100 instances require rolling mode with batch validation.
 * Never push all-at-once to more than 100 instances without explicit operator approval.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface ConfigPushParams {
  /** Instance IDs to push to. Use "all" for entire active fleet. */
  targets: string[] | 'all';
  /** The new config to apply. Only include changed fields. */
  configDiff: Record<string, unknown>;
  /**
   * Push strategy:
   * - rolling: 50 instances at a time, validate each batch before next
   * - immediate: all at once (requires targets.length <= 100 OR confirmationToken)
   */
  strategy?: 'rolling' | 'immediate';
  /** Batch size for rolling pushes (default: 50) */
  batchSize?: number;
  /** Auto-rollback if validation fails on a batch */
  autoRollback?: boolean;
  /** Human-readable description of what changed (for audit trail) */
  changeDescription: string;
  /** Operator confirmation token — required for immediate push to > 100 instances */
  confirmationToken?: string;
}

export interface BatchResult {
  batchNumber: number;
  instanceIds: string[];
  applied: number;
  failed: number;
  validationErrors: Array<{ instanceId: string; error: string }>;
  rolledBack: boolean;
}

export interface ConfigPushResult {
  pushId: string;
  strategy: 'rolling' | 'immediate';
  totalTargets: number;
  applied: number;
  failed: number;
  skipped: number;
  batches: BatchResult[];
  failedInstances: Array<{ instanceId: string; error: string }>;
  configVersion: string;
  completedAt: string | null;
  status: 'IN_PROGRESS' | 'COMPLETE' | 'PARTIAL_FAILURE' | 'ROLLED_BACK';
}

export function configPushTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_config_push',
    description:
      'Push OpenClaw configuration changes to fleet instances. ' +
      'For > 100 instances, use strategy="rolling" (validates each batch of 50 before next). ' +
      'Set autoRollback=true to revert a batch if validation fails. ' +
      'Returns per-batch results including any validation errors. ' +
      'SAFETY: Never use strategy="immediate" for > 100 instances without confirmationToken. ' +
      'Example: gf_config_push({ targets: "all", configDiff: { "model.primary": "moonshot/kimi-k2.5" }, ' +
      'strategy: "rolling", batchSize: 50, autoRollback: true, ' +
      'changeDescription: "Switch primary model to Kimi K2.5" })',

    inputSchema: {
      type: 'object',
      properties: {
        targets: {
          oneOf: [
            { type: 'string', enum: ['all'] },
            { type: 'array', items: { type: 'string' } },
          ],
          description: 'Instance IDs to push to, or "all" for entire active fleet',
        },
        configDiff: {
          type: 'object',
          description: 'Config fields to update. Only include changed fields.',
        },
        strategy: {
          type: 'string',
          enum: ['rolling', 'immediate'],
          description: 'rolling: 50 at a time with validation. immediate: all at once.',
        },
        batchSize: {
          type: 'number',
          description: 'Instances per batch for rolling strategy (default: 50)',
        },
        autoRollback: {
          type: 'boolean',
          description: 'Auto-rollback a batch if validation fails (default: true)',
        },
        changeDescription: {
          type: 'string',
          description: 'Human-readable description of the change (for audit log)',
        },
        confirmationToken: {
          type: 'string',
          description: 'Required for immediate push to > 100 instances',
        },
      },
      required: ['targets', 'configDiff', 'changeDescription'],
    },

    execute: async (params: unknown): Promise<ConfigPushResult> => {
      const p = params as ConfigPushParams;

      // Safety check: immediate mode to all requires confirmation
      if (p.strategy === 'immediate' && p.targets === 'all' && !p.confirmationToken) {
        throw new Error(
          'SAFETY: Immediate config push to all instances requires confirmationToken. ' +
          'Use strategy="rolling" or obtain operator confirmation first.'
        );
      }

      const response = await axios.post<ConfigPushResult>(
        `${config.gfApiBase}/v1/config/push`,
        {
          targets: p.targets,
          config_diff: p.configDiff,
          strategy: p.strategy ?? 'rolling',
          batch_size: p.batchSize ?? 50,
          auto_rollback: p.autoRollback ?? true,
          change_description: p.changeDescription,
          confirmation_token: p.confirmationToken,
        },
        {
          headers: {
            Authorization: `Bearer ${config.gfApiKey}`,
            'Content-Type': 'application/json',
          },
          timeout: 30_000, // initial request — actual push runs async
        }
      );

      return response.data;
    },
  };
}
