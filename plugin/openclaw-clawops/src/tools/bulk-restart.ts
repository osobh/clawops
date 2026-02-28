/**
 * gf_bulk_restart — Restart OpenClaw on multiple instances simultaneously
 *
 * Used by Commander/Guardian to recover a set of degraded instances.
 * Executes docker compose restart openclaw in parallel (configurable concurrency).
 * Returns per-instance results with health score before/after.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface BulkRestartParams {
  /** Instance IDs to restart. Max 200 per call. */
  instanceIds: string[];
  /** Number of concurrent restarts (default: 20) */
  concurrency?: number;
  /** Wait N seconds after restart before checking health (default: 90) */
  postRestartWaitSecs?: number;
  /** If health score doesn't recover, trigger failover for that instance */
  failoverOnUnrecovered?: boolean;
  /** Human-readable reason for audit trail */
  reason: string;
}

export interface InstanceRestartResult {
  instanceId: string;
  accountId: string;
  healthScoreBefore: number;
  healthScoreAfter: number | null;
  status: 'HEALED' | 'STILL_DEGRADED' | 'FAILED' | 'SKIPPED' | 'FAILOVER_TRIGGERED';
  restartDurationMs: number;
  error: string | null;
}

export interface BulkRestartResult {
  batchId: string;
  totalRequested: number;
  healed: number;
  stillDegraded: number;
  failed: number;
  failoverTriggered: number;
  results: InstanceRestartResult[];
  totalDurationMs: number;
  completedAt: string;
}

export function bulkRestartTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_bulk_restart',
    description:
      'Restart OpenClaw on multiple degraded instances simultaneously. ' +
      'Runs docker compose restart openclaw on each instance, waits 90s, ' +
      'then checks health score. Returns healed/still_degraded/failed per instance. ' +
      'Guardian calls this during fleet sweep when multiple instances are degraded. ' +
      'SAFETY: Will not restart healthy instances (health_score >= 70). ' +
      'Set failoverOnUnrecovered=true to automatically failover instances that don\'t recover. ' +
      'Example: gf_bulk_restart({ instanceIds: ["inst-001", "inst-002"], ' +
      'reason: "Scheduled maintenance restart" })',

    inputSchema: {
      type: 'object',
      properties: {
        instanceIds: {
          type: 'array',
          items: { type: 'string' },
          description: 'Instance IDs to restart (max 200)',
          maxItems: 200,
        },
        concurrency: {
          type: 'number',
          description: 'Concurrent restarts (default: 20, max: 50)',
          minimum: 1,
          maximum: 50,
        },
        postRestartWaitSecs: {
          type: 'number',
          description: 'Seconds to wait after restart before health check (default: 90)',
        },
        failoverOnUnrecovered: {
          type: 'boolean',
          description: 'Trigger failover for instances that don\'t recover (default: false)',
        },
        reason: {
          type: 'string',
          description: 'Reason for bulk restart (logged to audit trail)',
        },
      },
      required: ['instanceIds', 'reason'],
    },

    execute: async (params: unknown): Promise<BulkRestartResult> => {
      const p = params as BulkRestartParams;

      if (p.instanceIds.length > 200) {
        throw new Error('Maximum 200 instances per bulk restart call');
      }

      const response = await axios.post<BulkRestartResult>(
        `${config.gfApiBase}/v1/instances/bulk-restart`,
        {
          instance_ids: p.instanceIds,
          concurrency: Math.min(p.concurrency ?? 20, 50),
          post_restart_wait_secs: p.postRestartWaitSecs ?? 90,
          failover_on_unrecovered: p.failoverOnUnrecovered ?? false,
          reason: p.reason,
        },
        {
          headers: {
            Authorization: `Bearer ${config.gfApiKey}`,
            'Content-Type': 'application/json',
          },
          timeout: 15_000, // async operation — check progress via fleet status
        }
      );

      return response.data;
    },
  };
}
