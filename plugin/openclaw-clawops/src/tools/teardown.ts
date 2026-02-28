/**
 * gf_teardown — Teardown a VPS gateway pair with optional data archiving
 *
 * SAFETY CRITICAL: This permanently deletes VPS instances.
 * - Always verify no active user sessions before teardown.
 * - Never teardown PRIMARY without confirming STANDBY is handled.
 * - Always log to audit trail before calling provider delete API.
 * - Operator confirmation required in all cases.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface TeardownParams {
  /** Account ID whose gateway pair to teardown */
  accountId: string;
  /**
   * Whether to archive OpenClaw config + conversation history to S3 before teardown.
   * Default: true — always archive unless explicitly disabled.
   */
  archiveBeforeTeardown?: boolean;
  /**
   * Operator confirmation token — REQUIRED for all teardowns.
   * Generated when operator provides explicit confirmation.
   */
  confirmationToken: string;
  /** Reason for teardown (logged to audit trail) */
  reason: 'idle' | 'operator_request' | 'billing_failure' | 'account_closed' | 'maintenance';
  /** If true, only teardown standby (e.g. after failover while reprovisioning) */
  standbyOnly?: boolean;
}

export interface TeardownResult {
  requestId: string;
  accountId: string;
  status: 'QUEUED' | 'ARCHIVING' | 'TEARING_DOWN' | 'COMPLETE' | 'FAILED';
  archiveUrl: string | null;
  primaryDeleted: boolean;
  standbyDeleted: boolean;
  estimatedCompletionSecs: number;
  auditRecordId: string;
  message: string;
}

export function teardownTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_teardown',
    description:
      'Teardown (permanently delete) a VPS gateway pair for an account. ' +
      'SAFETY: confirmationToken is ALWAYS required — never call without operator approval. ' +
      'ALWAYS archives data to S3 before deletion unless archiveBeforeTeardown=false. ' +
      'NEVER teardown an account with active user sessions. ' +
      'Typical use: 31 idle accounts after Ledger identifies them as waste. ' +
      'Example: gf_teardown({ accountId: "acct-001", confirmationToken: "tok_xyz", ' +
      'reason: "idle", archiveBeforeTeardown: true })',

    inputSchema: {
      type: 'object',
      properties: {
        accountId: {
          type: 'string',
          description: 'Account ID whose VPS pair to teardown',
        },
        archiveBeforeTeardown: {
          type: 'boolean',
          description: 'Archive config + data to S3 before deletion (default: true)',
        },
        confirmationToken: {
          type: 'string',
          description: 'REQUIRED: Operator confirmation token. Never call without this.',
        },
        reason: {
          type: 'string',
          enum: ['idle', 'operator_request', 'billing_failure', 'account_closed', 'maintenance'],
          description: 'Teardown reason logged to audit trail',
        },
        standbyOnly: {
          type: 'boolean',
          description: 'Only teardown the standby instance (not the primary)',
        },
      },
      required: ['accountId', 'confirmationToken', 'reason'],
    },

    execute: async (params: unknown): Promise<TeardownResult> => {
      const p = params as TeardownParams;

      // Validate confirmation token is present
      if (!p.confirmationToken || p.confirmationToken.trim() === '') {
        throw new Error(
          'SAFETY: confirmationToken is required for teardown operations. ' +
          'Obtain operator confirmation before proceeding.'
        );
      }

      const response = await axios.post<TeardownResult>(
        `${config.gfApiBase}/v1/teardown`,
        {
          account_id: p.accountId,
          archive_before_teardown: p.archiveBeforeTeardown ?? true,
          confirmation_token: p.confirmationToken,
          reason: p.reason,
          standby_only: p.standbyOnly ?? false,
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
