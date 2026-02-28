/**
 * gf_instance_health — Get detailed health for a specific instance
 *
 * Called by Guardian during health sweeps and auto-heal sequences.
 * Returns the full HealthReport from gf-clawnode, including OpenClaw status,
 * docker container states, resource utilization, and computed health score.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface InstanceHealthParams {
  /** Instance UUID */
  instanceId: string;
  /** Force a live check (bypass cache). Default: false — returns cached heartbeat */
  live?: boolean;
}

export interface ContainerStatus {
  name: string;
  image: string;
  state: 'running' | 'exited' | 'restarting' | 'paused' | 'dead';
  uptimeSeconds: number;
  restartCount: number;
}

export interface InstanceHealthResult {
  instanceId: string;
  accountId: string;
  provider: string;
  region: string;
  tier: string;
  role: 'PRIMARY' | 'STANDBY';
  pairInstanceId: string | null;

  // Health
  healthScore: number; // 0–100
  status: 'HEALTHY' | 'DEGRADED' | 'CRITICAL' | 'UNKNOWN';
  openclawStatus: 'HEALTHY' | 'DEGRADED' | 'DOWN' | 'UNKNOWN';
  openclawHttpStatus: number | null; // last HTTP health check result
  dockerRunning: boolean;
  tailscaleConnected: boolean;
  tailscaleLatencyMs: number | null;

  // Resources
  cpuUsage1m: number;
  memUsagePct: number;
  diskUsagePct: number;
  swapUsagePct: number;
  loadAvg1m: number;
  loadAvg5m: number;
  loadAvg15m: number;
  uptimeSeconds: number;

  // Containers
  containers: ContainerStatus[];

  // Timestamps
  lastHeartbeat: string;
  checkedAt: string;

  // Alerts active on this instance
  alerts: Array<{
    type: string;
    severity: 'info' | 'warning' | 'critical';
    message: string;
    value?: number;
    threshold?: number;
  }>;

  // Recommended action from auto-heal decision tree
  recommendedAction: 'none' | 'monitor' | 'auto_heal' | 'failover' | 'escalate';
}

export function instanceHealthTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_instance_health',
    description:
      'Get detailed health report for a specific VPS instance. Returns health score (0–100), ' +
      'OpenClaw status, Docker container states, resource utilization (CPU/RAM/disk), ' +
      'Tailscale connectivity, and recommended action. ' +
      'Guardian calls this for every degraded instance detected in fleet sweep. ' +
      'Example: gf_instance_health({ instanceId: "abc-123" }) ' +
      'Example: gf_instance_health({ instanceId: "abc-123", live: true }) — force fresh check',

    inputSchema: {
      type: 'object',
      properties: {
        instanceId: {
          type: 'string',
          description: 'Instance UUID to check',
        },
        live: {
          type: 'boolean',
          description: 'Force a live health check (bypass 30s cache). Default: false.',
        },
      },
      required: ['instanceId'],
    },

    execute: async (params: unknown): Promise<InstanceHealthResult> => {
      const { instanceId, live = false } = params as InstanceHealthParams;

      const url = `${config.gfApiBase}/v1/instances/${encodeURIComponent(instanceId)}/health`;
      const response = await axios.get<InstanceHealthResult>(url, {
        headers: { Authorization: `Bearer ${config.gfApiKey}` },
        params: { live: live ? 'true' : 'false' },
        timeout: live ? 30_000 : 5_000, // live checks take longer
      });

      return response.data;
    },
  };
}
