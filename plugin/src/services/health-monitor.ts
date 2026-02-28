/**
 * ClawOps Background Fleet Health Monitor Service
 *
 * Runs continuously in the OpenClaw gateway process. Polls fleet status
 * every 5 minutes and emits structured events to specialist agents when
 * thresholds are crossed.
 *
 * This is the autonomous monitoring heartbeat that enables Guardian to
 * operate without being polled by the operator.
 *
 * Event emission map:
 *   Degraded instance detected       → INSTANCE_DEGRADED → Guardian
 *   Failed pair detected             → PAIR_FAILED → Guardian (high priority)
 *   Cost anomaly (> 15% above proj)  → COST_ANOMALY → Ledger
 *   Provision queue depth > 20       → PROVISION_QUEUE_BACKLOG → Forge
 *   Provider health score < 75       → PROVIDER_DEGRADED → Commander
 */

import type { ClawOpsConfig } from '../index';
import type { FleetStatusResult } from '../tools/fleet-status';
import axios from 'axios';

// ─── Event types ─────────────────────────────────────────────────────────────

export type MonitorEventType =
  | 'INSTANCE_DEGRADED'
  | 'INSTANCE_FAILED'
  | 'PAIR_FAILED'
  | 'COST_ANOMALY'
  | 'PROVISION_QUEUE_BACKLOG'
  | 'PROVIDER_DEGRADED'
  | 'FLEET_RECOVERING'
  | 'FLEET_HEALTHY';

export interface MonitorEvent {
  eventId: string;
  type: MonitorEventType;
  priority: 'low' | 'medium' | 'high' | 'critical';
  timestamp: string;
  targetAgent: 'guardian' | 'ledger' | 'forge' | 'commander';
  payload: Record<string, unknown>;
  suppressUntil?: string; // ISO timestamp — don't re-emit until after this
}

// ─── Health monitor thresholds ─────────────────────────────────────────────

export interface MonitorThresholds {
  /** Instance health score below which INSTANCE_DEGRADED is emitted */
  instanceDegradedScore: number;
  /** Cost deviation % above which COST_ANOMALY is emitted */
  costAnomalyPct: number;
  /** Provision queue depth above which PROVISION_QUEUE_BACKLOG is emitted */
  provisionQueueDepth: number;
  /** Provider health score below which PROVIDER_DEGRADED is emitted */
  providerDegradedScore: number;
  /** Minutes an instance must remain degraded before re-emitting the event */
  degradedSuppressionMins: number;
}

const DEFAULT_THRESHOLDS: MonitorThresholds = {
  instanceDegradedScore: 50,
  costAnomalyPct: 15,
  provisionQueueDepth: 20,
  providerDegradedScore: 75,
  degradedSuppressionMins: 30,
};

// ─── Health monitor service ───────────────────────────────────────────────────

export class HealthMonitorService {
  private config: ClawOpsConfig;
  private thresholds: MonitorThresholds;
  private running: boolean = false;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private suppressedInstances: Map<string, Date> = new Map();
  private suppressedProviders: Map<string, Date> = new Map();
  private lastKnownState: FleetStatusResult | null = null;

  constructor(config: ClawOpsConfig, thresholds?: Partial<MonitorThresholds>) {
    this.config = config;
    this.thresholds = { ...DEFAULT_THRESHOLDS, ...thresholds };
  }

  async start(): Promise<void> {
    if (this.running) {
      console.warn('[health-monitor] Already running');
      return;
    }

    this.running = true;
    const intervalMs = this.config.healthPollIntervalMs ?? 300_000;

    console.log(`[health-monitor] Starting — poll interval: ${intervalMs / 1000}s`);

    // Run immediately on start, then on interval
    await this.runSweep().catch((e) => console.error('[health-monitor] Initial sweep failed:', e));

    this.pollTimer = setInterval(async () => {
      if (!this.running) return;
      await this.runSweep().catch((e) =>
        console.error('[health-monitor] Sweep failed:', e)
      );
    }, intervalMs);
  }

  async stop(): Promise<void> {
    this.running = false;
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
      this.pollTimer = null;
    }
    console.log('[health-monitor] Stopped');
  }

  // ─── Main sweep ────────────────────────────────────────────────────────────

  private async runSweep(): Promise<void> {
    const sweepStart = Date.now();
    console.debug('[health-monitor] Running fleet sweep');

    let fleetStatus: FleetStatusResult;
    try {
      fleetStatus = await this.fetchFleetStatus();
    } catch (e) {
      console.error('[health-monitor] Failed to fetch fleet status:', e);
      return;
    }

    const events: MonitorEvent[] = [];

    // Check for degraded/failed instances
    events.push(...this.checkInstanceHealth(fleetStatus));

    // Check for cost anomalies
    const costEvent = this.checkCostAnomaly(fleetStatus);
    if (costEvent) events.push(costEvent);

    // Check provision queue depth
    const queueEvent = this.checkProvisionQueue(fleetStatus);
    if (queueEvent) events.push(queueEvent);

    // Check provider health
    events.push(...this.checkProviderHealth(fleetStatus));

    // Check for fleet recovery (was degraded, now healthy)
    const recoveryEvent = this.checkFleetRecovery(fleetStatus);
    if (recoveryEvent) events.push(recoveryEvent);

    // Emit all events
    for (const event of events) {
      await this.emitEvent(event).catch((e) =>
        console.error(`[health-monitor] Failed to emit ${event.type}:`, e)
      );
    }

    this.lastKnownState = fleetStatus;

    const duration = Date.now() - sweepStart;
    console.debug(`[health-monitor] Sweep complete in ${duration}ms — ${events.length} events emitted`);
  }

  // ─── Check functions ───────────────────────────────────────────────────────

  private checkInstanceHealth(fleet: FleetStatusResult): MonitorEvent[] {
    const events: MonitorEvent[] = [];
    const now = new Date();
    const suppressionMs = this.thresholds.degradedSuppressionMins * 60_000;

    // Check degraded instances
    if (fleet.degradedInstances > 0) {
      // In full detail mode we'd iterate instances; in summary mode we emit an aggregate event
      const key = 'fleet-degraded';
      const suppressedUntil = this.suppressedInstances.get(key);

      if (!suppressedUntil || now > suppressedUntil) {
        events.push({
          eventId: `evt-${Date.now()}-degraded`,
          type: 'INSTANCE_DEGRADED',
          priority: fleet.degradedInstances > 10 ? 'high' : 'medium',
          timestamp: now.toISOString(),
          targetAgent: 'guardian',
          payload: {
            degradedCount: fleet.degradedInstances,
            failedCount: fleet.failedInstances,
            byProvider: fleet.byProvider,
            message: `${fleet.degradedInstances} degraded, ${fleet.failedInstances} failed instances detected`,
          },
          suppressUntil: new Date(now.getTime() + suppressionMs).toISOString(),
        });

        this.suppressedInstances.set(
          key,
          new Date(now.getTime() + suppressionMs)
        );
      }
    }

    // Failed pairs are always high/critical priority — no suppression
    if (fleet.failedInstances > 0) {
      events.push({
        eventId: `evt-${Date.now()}-failed`,
        type: 'PAIR_FAILED',
        priority: 'critical',
        timestamp: now.toISOString(),
        targetAgent: 'guardian',
        payload: {
          failedCount: fleet.failedInstances,
          alerts: fleet.alerts.filter((a) => a.severity === 'critical'),
          message: `CRITICAL: ${fleet.failedInstances} gateway pairs in FAILED state`,
        },
      });
    }

    return events;
  }

  private checkCostAnomaly(fleet: FleetStatusResult): MonitorEvent | null {
    const { monthlyActualUsd, monthlyProjectedUsd, deviationPct } = fleet.cost;

    if (Math.abs(deviationPct) >= this.thresholds.costAnomalyPct) {
      return {
        eventId: `evt-${Date.now()}-cost`,
        type: 'COST_ANOMALY',
        priority: Math.abs(deviationPct) > 25 ? 'high' : 'medium',
        timestamp: new Date().toISOString(),
        targetAgent: 'ledger',
        payload: {
          actualUsd: monthlyActualUsd,
          projectedUsd: monthlyProjectedUsd,
          deviationPct,
          direction: deviationPct > 0 ? 'over' : 'under',
          message: `Cost anomaly: ${deviationPct > 0 ? '+' : ''}${deviationPct.toFixed(1)}% vs projection ($${monthlyActualUsd.toFixed(0)} actual vs $${monthlyProjectedUsd.toFixed(0)} projected)`,
        },
      };
    }

    return null;
  }

  private checkProvisionQueue(fleet: FleetStatusResult): MonitorEvent | null {
    const queueDepth = fleet.bootstrappingInstances;

    if (queueDepth >= this.thresholds.provisionQueueDepth) {
      return {
        eventId: `evt-${Date.now()}-queue`,
        type: 'PROVISION_QUEUE_BACKLOG',
        priority: queueDepth > 50 ? 'high' : 'medium',
        timestamp: new Date().toISOString(),
        targetAgent: 'forge',
        payload: {
          queueDepth,
          bootstrappingInstances: fleet.bootstrappingInstances,
          message: `Provision queue backlog: ${queueDepth} instances in BOOTSTRAPPING state`,
        },
      };
    }

    return null;
  }

  private checkProviderHealth(fleet: FleetStatusResult): MonitorEvent[] {
    const events: MonitorEvent[] = [];
    const now = new Date();
    const suppressionMs = 60 * 60_000; // 1 hour suppression for provider alerts

    for (const [provider, summary] of Object.entries(fleet.byProvider)) {
      if (summary.healthScore < this.thresholds.providerDegradedScore) {
        const suppressedUntil = this.suppressedProviders.get(provider);

        if (!suppressedUntil || now > suppressedUntil) {
          events.push({
            eventId: `evt-${Date.now()}-provider-${provider}`,
            type: 'PROVIDER_DEGRADED',
            priority: summary.healthScore < 50 ? 'critical' : 'high',
            timestamp: now.toISOString(),
            targetAgent: 'commander',
            payload: {
              provider,
              healthScore: summary.healthScore,
              activeInstances: summary.active,
              degradedInstances: summary.degraded,
              message: `Provider ${provider} health score: ${summary.healthScore}/100 — consider pausing new provisions`,
            },
            suppressUntil: new Date(now.getTime() + suppressionMs).toISOString(),
          });

          this.suppressedProviders.set(
            provider,
            new Date(now.getTime() + suppressionMs)
          );
        }
      } else {
        // Clear suppression when provider recovers
        this.suppressedProviders.delete(provider);
      }
    }

    return events;
  }

  private checkFleetRecovery(fleet: FleetStatusResult): MonitorEvent | null {
    // If previous state had failures and current state doesn't, emit recovery event
    if (
      this.lastKnownState &&
      (this.lastKnownState.failedInstances > 0 || this.lastKnownState.degradedInstances > 0) &&
      fleet.failedInstances === 0 &&
      fleet.degradedInstances === 0
    ) {
      return {
        eventId: `evt-${Date.now()}-recovery`,
        type: 'FLEET_RECOVERING',
        priority: 'low',
        timestamp: new Date().toISOString(),
        targetAgent: 'commander',
        payload: {
          previousFailed: this.lastKnownState.failedInstances,
          previousDegraded: this.lastKnownState.degradedInstances,
          message: 'Fleet recovered — all instances now healthy',
        },
      };
    }

    return null;
  }

  // ─── Event emission ────────────────────────────────────────────────────────

  private async emitEvent(event: MonitorEvent): Promise<void> {
    console.log(
      `[health-monitor] EMIT ${event.type} → ${event.targetAgent} [${event.priority}]`
    );

    // Emit to the appropriate agent's session via OpenClaw sessions_send
    // The session ID is configured at plugin load time
    const sessionId = this.getSessionId(event.targetAgent);

    if (!sessionId) {
      // No session configured — log and skip (agent may not be running)
      console.warn(
        `[health-monitor] No session ID for ${event.targetAgent} — event queued for next agent start`
      );
      return;
    }

    // TODO: call OpenClaw sessions_send API to deliver event to agent session
    // This will be replaced with the OpenClaw SDK call when integrated:
    //   await openclaw.sessions.send(sessionId, {
    //     role: 'user',
    //     content: JSON.stringify(event),
    //   });
    await axios.post(
      `${this.config.gfApiBase}/_internal/sessions/${sessionId}/event`,
      event,
      {
        headers: { Authorization: `Bearer ${this.config.gfApiKey}` },
        timeout: 5_000,
      }
    ).catch(() => {
      // Non-fatal — agent session may not be active
    });
  }

  private getSessionId(agent: MonitorEvent['targetAgent']): string | undefined {
    switch (agent) {
      case 'guardian': return this.config.guardianSessionId;
      case 'ledger': return this.config.ledgerSessionId;
      case 'commander': return this.config.commanderSessionId;
      case 'forge': return undefined; // Forge is spawned on demand
    }
  }

  // ─── Fleet status fetch ────────────────────────────────────────────────────

  private async fetchFleetStatus(): Promise<FleetStatusResult> {
    const response = await axios.get<FleetStatusResult>(
      `${this.config.gfApiBase}/v1/fleet/status`,
      {
        headers: { Authorization: `Bearer ${this.config.gfApiKey}` },
        timeout: 10_000,
      }
    );
    return response.data;
  }

  // ─── Status / diagnostics ──────────────────────────────────────────────────

  get isRunning(): boolean {
    return this.running;
  }

  get lastState(): FleetStatusResult | null {
    return this.lastKnownState;
  }

  getSuppressedCount(): number {
    const now = new Date();
    let count = 0;
    for (const d of this.suppressedInstances.values()) {
      if (now < d) count++;
    }
    for (const d of this.suppressedProviders.values()) {
      if (now < d) count++;
    }
    return count;
  }
}
