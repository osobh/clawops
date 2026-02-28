/**
 * gf_audit_log — Query and append to the agent action audit trail
 *
 * All agent actions that touch infrastructure are recorded here.
 * This tool lets Triage and Commander inspect the audit trail during incidents,
 * and lets operators verify what the agent team has done.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface AuditLogQueryParams {
  /** Filter by account ID */
  accountId?: string;
  /** Filter by instance ID */
  instanceId?: string;
  /** Filter by agent */
  agent?: 'commander' | 'guardian' | 'forge' | 'ledger' | 'triage' | 'briefer' | 'system';
  /** Filter by action type */
  action?: string;
  /** Start time (ISO 8601) */
  from?: string;
  /** End time (ISO 8601) */
  to?: string;
  /** Max records to return (default: 50, max: 500) */
  limit?: number;
  /** Filter by severity of outcome */
  outcome?: 'success' | 'failure' | 'all';
}

export interface AuditRecord {
  recordId: string;
  correlationId: string;
  timestamp: string;
  agent: string;
  action: string;
  target: {
    type: string;
    id: string;
    accountId: string | null;
    provider: string | null;
    region: string | null;
  };
  parameters: Record<string, unknown>;
  result: {
    success: boolean;
    durationMs: number;
    error: string | null;
    affectedResources: string[];
    completedAt: string;
  } | null;
  operatorConfirmation: {
    confirmedBy: string;
    confirmedAt: string;
    channel: string;
  } | null;
}

export interface AuditLogResult {
  records: AuditRecord[];
  total: number;
  returnedCount: number;
  queryParams: AuditLogQueryParams;
  generatedAt: string;
}

export function auditLogTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_audit_log',
    description:
      'Query the immutable agent action audit trail. Every infrastructure action ' +
      'taken by any agent is logged here with agent identity, parameters, and outcome. ' +
      'Use during incident investigation to reconstruct what happened and when. ' +
      'Triage calls this first when investigating any incident. ' +
      'Example: gf_audit_log({ from: "2024-01-15T00:00:00Z", action: "trigger_failover" }) ' +
      'Example: gf_audit_log({ accountId: "acct-001", limit: 20 }) — last 20 actions on account',

    inputSchema: {
      type: 'object',
      properties: {
        accountId: {
          type: 'string',
          description: 'Filter by account ID',
        },
        instanceId: {
          type: 'string',
          description: 'Filter by instance ID',
        },
        agent: {
          type: 'string',
          enum: ['commander', 'guardian', 'forge', 'ledger', 'triage', 'briefer', 'system'],
          description: 'Filter by agent that performed the action',
        },
        action: {
          type: 'string',
          description: 'Filter by action type (e.g. "trigger_failover", "docker_restart_openclaw")',
        },
        from: {
          type: 'string',
          description: 'Start time filter (ISO 8601)',
        },
        to: {
          type: 'string',
          description: 'End time filter (ISO 8601)',
        },
        limit: {
          type: 'number',
          description: 'Max records to return (default: 50, max: 500)',
        },
        outcome: {
          type: 'string',
          enum: ['success', 'failure', 'all'],
          description: 'Filter by action outcome (default: all)',
        },
      },
      required: [],
    },

    execute: async (params: unknown): Promise<AuditLogResult> => {
      const p = (params as AuditLogQueryParams) ?? {};

      const queryParams = new URLSearchParams();
      if (p.accountId) queryParams.set('account_id', p.accountId);
      if (p.instanceId) queryParams.set('instance_id', p.instanceId);
      if (p.agent) queryParams.set('agent', p.agent);
      if (p.action) queryParams.set('action', p.action);
      if (p.from) queryParams.set('from', p.from);
      if (p.to) queryParams.set('to', p.to);
      if (p.limit) queryParams.set('limit', String(Math.min(p.limit, 500)));
      if (p.outcome && p.outcome !== 'all') queryParams.set('outcome', p.outcome);

      const response = await axios.get<AuditLogResult>(
        `${config.gfApiBase}/v1/audit?${queryParams}`,
        {
          headers: { Authorization: `Bearer ${config.gfApiKey}` },
          timeout: 10_000,
        }
      );

      return response.data;
    },
  };
}
