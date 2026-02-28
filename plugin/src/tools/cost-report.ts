/**
 * gf_cost_report — Generate fleet cost analysis and waste identification
 *
 * Primary tool for the Ledger agent. Returns itemized cost breakdown,
 * identifies waste (idle accounts, over-provisioned tiers, suboptimal providers),
 * and produces actionable recommendations with dollar amounts.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface CostReportParams {
  /** Time period for the report */
  period?: 'current_month' | 'last_month' | 'last_7d' | 'last_30d';
  /** Include idle account analysis (14+ days no activity) */
  includeIdleAnalysis?: boolean;
  /** Include tier optimization recommendations */
  includeTierOptimization?: boolean;
  /** Include provider cost comparison */
  includeProviderComparison?: boolean;
  /** Minimum monthly savings threshold to include a recommendation (USD) */
  minSavingsUsd?: number;
}

export interface WasteItem {
  category: 'idle_accounts' | 'overprovisioned' | 'suboptimal_provider' | 'duplicate_resources';
  description: string;
  monthlyCostUsd: number;
  recommendation: string;
  affectedAccounts: string[];
  confidence: 'low' | 'medium' | 'high';
  actionRequired: string;
}

export interface ProviderCostBreakdown {
  provider: string;
  instanceCount: number;
  monthlyCostUsd: number;
  costPerInstanceUsd: number;
  percentOfTotal: number;
  vsAlternative?: {
    provider: string;
    monthlySavingsUsd: number;
    notes: string;
  };
}

export interface CostReportResult {
  reportId: string;
  period: string;
  generatedAt: string;

  // Summary
  totalMonthlyActualUsd: number;
  totalMonthlyProjectedUsd: number;
  deviationPct: number;
  costPerActiveAccountUsd: number;
  totalActiveAccounts: number;

  // Weekly breakdown
  weeklyActualUsd: number;
  weeklyProjectedUsd: number;

  // Waste analysis
  totalRecoverableUsd: number;
  wasteItems: WasteItem[];

  // Provider breakdown
  byProvider: ProviderCostBreakdown[];

  // Tier breakdown
  byTier: Array<{
    tier: string;
    count: number;
    monthlyCostUsd: number;
    avgUtilizationCpu: number;
    avgUtilizationMem: number;
    downsizeCandidates: number;
  }>;

  // Top recommendations
  recommendations: Array<{
    priority: 'high' | 'medium' | 'low';
    action: string;
    monthlySavingsUsd: number;
    effort: 'automated' | 'manual_review' | 'operator_approval';
    details: string;
  }>;

  // Month-over-month trend
  trend: {
    previousMonthUsd: number;
    currentMonthUsd: number;
    changeUsd: number;
    changePct: number;
    drivers: string[];
  };
}

export function costReportTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_cost_report',
    description:
      'Generate a fleet cost analysis report. Identifies waste across three categories: ' +
      '(1) idle accounts (14+ days inactive), (2) over-provisioned tiers (< 20% avg utilization), ' +
      '(3) suboptimal provider choices. Returns actionable recommendations with dollar amounts. ' +
      'Ledger calls this every 6h autonomously; Commander calls it on operator request. ' +
      'Example: gf_cost_report({ period: "current_month", includeIdleAnalysis: true }) ' +
      'Example response: "31 idle accounts → $341/month. 18 overprovisioned → $108/month savings."',

    inputSchema: {
      type: 'object',
      properties: {
        period: {
          type: 'string',
          enum: ['current_month', 'last_month', 'last_7d', 'last_30d'],
          description: 'Reporting period (default: current_month)',
        },
        includeIdleAnalysis: {
          type: 'boolean',
          description: 'Include idle account analysis (default: true)',
        },
        includeTierOptimization: {
          type: 'boolean',
          description: 'Include tier resize recommendations (default: true)',
        },
        includeProviderComparison: {
          type: 'boolean',
          description: 'Include provider cost comparison (default: true)',
        },
        minSavingsUsd: {
          type: 'number',
          description: 'Minimum monthly savings to include a recommendation (default: 10)',
        },
      },
      required: [],
    },

    execute: async (params: unknown): Promise<CostReportResult> => {
      const p = (params as CostReportParams) ?? {};

      const response = await axios.post<CostReportResult>(
        `${config.gfApiBase}/v1/reports/cost`,
        {
          period: p.period ?? 'current_month',
          include_idle_analysis: p.includeIdleAnalysis ?? true,
          include_tier_optimization: p.includeTierOptimization ?? true,
          include_provider_comparison: p.includeProviderComparison ?? true,
          min_savings_usd: p.minSavingsUsd ?? 10,
        },
        {
          headers: {
            Authorization: `Bearer ${config.gfApiKey}`,
            'Content-Type': 'application/json',
          },
          timeout: 30_000,
        }
      );

      return response.data;
    },
  };
}
