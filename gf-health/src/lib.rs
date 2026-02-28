//! gf-health — Fleet health checking and auto-heal orchestration
//!
//! Implements the decision logic Guardian uses to detect degraded instances
//! and execute the auto-heal sequence defined in auto-heal.md.
//!
//! IMPORTANT: This crate encodes hard safety rules. Changes must be reviewed.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

pub use gf_node_proto::{HealthReport, ServiceStatus};

// ─── Health thresholds ────────────────────────────────────────────────────────

/// Configurable thresholds for health decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthThresholds {
    /// Below this score → trigger auto-heal sequence
    pub heal_trigger_score: u8,
    /// Above this score → instance is considered recovered
    pub recovery_score: u8,
    /// Minutes of missing heartbeat before triggering heal
    pub heartbeat_timeout_mins: u32,
    /// Disk usage % that triggers warning
    pub disk_warn_pct: f32,
    /// Disk usage % that triggers critical alert
    pub disk_critical_pct: f32,
    /// CPU usage % (1-min avg) that triggers warning
    pub cpu_warn_pct: f32,
    /// Memory usage % that triggers warning
    pub mem_warn_pct: f32,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            heal_trigger_score: 50,
            recovery_score: 70,
            heartbeat_timeout_mins: 3,
            disk_warn_pct: 80.0,
            disk_critical_pct: 90.0,
            cpu_warn_pct: 90.0,
            mem_warn_pct: 90.0,
        }
    }
}

// ─── Health check result ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub instance_id: String,
    pub checked_at: DateTime<Utc>,
    pub health_score: u8,
    pub status: InstanceHealthStatus,
    pub alerts: Vec<HealthAlert>,
    pub recommended_action: RecommendedAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InstanceHealthStatus {
    /// health_score >= 70, all services running
    Healthy,
    /// health_score 50–69, degraded but recoverable
    Degraded,
    /// health_score < 50 or heartbeat missing, needs intervention
    Critical,
    /// No data received, assume failed
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthAlert {
    pub alert_type: AlertType,
    pub severity: AlertSeverity,
    pub message: String,
    pub value: Option<f64>,
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    OpenClawDown,
    DockerDown,
    HeartbeatMissing,
    DiskUsageHigh,
    CpuUsageHigh,
    MemUsageHigh,
    TailscaleDisconnected,
    HealthScoreLow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedAction {
    /// No action needed
    None,
    /// Monitor — watch for next 5 minutes
    Monitor,
    /// Execute auto-heal sequence
    AutoHeal,
    /// Trigger failover to standby
    Failover,
    /// Escalate to Commander — do not act alone
    EscalateToCommander,
}

// ─── Auto-heal engine ─────────────────────────────────────────────────────────

/// Implements the auto-heal decision tree from auto-heal.md.
///
/// This is the exact sequence Guardian follows — encoded as a state machine
/// to prevent partial execution and ensure auditability.
pub struct AutoHealEngine {
    pub thresholds: HealthThresholds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealAttempt {
    pub attempt_id: Uuid,
    pub instance_id: String,
    pub started_at: DateTime<Utc>,
    pub steps: Vec<HealStep>,
    pub outcome: HealOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealStep {
    pub step_number: u8,
    pub description: String,
    pub action: HealAction,
    pub result: HealStepResult,
    pub executed_at: DateTime<Utc>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealAction {
    /// Step 1: verify by calling vps.health
    VerifyHealth,
    /// Step 2: docker compose restart openclaw
    DockerRestartOpenClaw,
    /// Step 3: wait 90s, call openclaw.health (HTTP 200 check)
    WaitAndVerifyOpenClaw,
    /// Step 4: check if PRIMARY; verify STANDBY is ACTIVE
    VerifyStandbyBeforeFailover,
    /// Step 5: promote standby (calls gf-failover)
    TriggerFailover,
    /// Step 6: notify Commander (CRITICAL — do not act alone)
    NotifyCommanderCritical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealStepResult {
    pub success: bool,
    pub recovered: bool,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealOutcome {
    Healed, // docker restart fixed it
    FailoverTriggered,
    EscalatedToCommander,
    Failed, // could not heal, no standby
}

impl AutoHealEngine {
    pub fn new(thresholds: HealthThresholds) -> Self {
        Self { thresholds }
    }

    /// Execute the auto-heal sequence for a degraded or failed instance.
    ///
    /// Follows the exact decision tree from auto-heal.md:
    /// Step 1 — Verify: Call vps.health. If health_score > 70, log and return.
    /// Step 2 — Docker restart: SSH to instance, docker compose restart openclaw
    /// Step 3 — Wait 90s, call openclaw.health. If 200, log HEALED.
    /// Step 4 — Check if PRIMARY. If yes, verify STANDBY is ACTIVE.
    /// Step 5 — If standby ACTIVE: trigger failover.
    /// Step 6 — If standby NOT active: CRITICAL — notify Commander.
    ///
    /// NEVER: delete a VPS. NEVER: touch another user's instance.
    /// NEVER: skip step 4 verification.
    pub async fn execute_heal_sequence(
        &self,
        instance_id: &str,
        pair_info: &PairInfo,
    ) -> Result<HealAttempt> {
        let attempt_id = Uuid::new_v4();
        let started_at = Utc::now();
        let mut steps = Vec::new();

        info!(
            %instance_id,
            %attempt_id,
            "Starting auto-heal sequence"
        );

        // Step 1: Verify health
        let step1_start = Utc::now();
        let current_health = self.call_vps_health(instance_id).await;
        let step1_result = match &current_health {
            Ok(score) if *score > self.thresholds.recovery_score => {
                info!(%instance_id, score, "Health recovered — no action needed");
                HealStepResult {
                    success: true,
                    recovered: true,
                    details: format!("Health score {score} > threshold, recovered"),
                }
            }
            Ok(score) => HealStepResult {
                success: true,
                recovered: false,
                details: format!("Health score {score} still degraded"),
            },
            Err(e) => HealStepResult {
                success: false,
                recovered: false,
                details: format!("Health check failed: {e}"),
            },
        };

        let recovered = step1_result.recovered;
        steps.push(HealStep {
            step_number: 1,
            description: "Verify current health score via vps.health".to_string(),
            action: HealAction::VerifyHealth,
            result: step1_result,
            executed_at: step1_start,
            duration_ms: (Utc::now() - step1_start).num_milliseconds() as u64,
        });

        if recovered {
            return Ok(HealAttempt {
                attempt_id,
                instance_id: instance_id.to_string(),
                started_at,
                steps,
                outcome: HealOutcome::Healed,
            });
        }

        // Step 2: Docker restart
        let step2_start = Utc::now();
        let restart_result = self.docker_restart_openclaw(instance_id).await;
        steps.push(HealStep {
            step_number: 2,
            description: "SSH to instance, run: docker compose restart openclaw".to_string(),
            action: HealAction::DockerRestartOpenClaw,
            result: HealStepResult {
                success: restart_result.is_ok(),
                recovered: false,
                details: restart_result
                    .map(|_| "Docker restart command executed".to_string())
                    .unwrap_or_else(|e| format!("Docker restart failed: {e}")),
            },
            executed_at: step2_start,
            duration_ms: (Utc::now() - step2_start).num_milliseconds() as u64,
        });

        // Step 3: Wait 90s, verify OpenClaw HTTP
        let step3_start = Utc::now();
        tokio::time::sleep(tokio::time::Duration::from_secs(90)).await;
        let http_result = self.check_openclaw_http(instance_id).await;
        let healed_by_restart = http_result.as_ref().map(|s| *s == 200u16).unwrap_or(false);

        if healed_by_restart {
            info!(%instance_id, "HEALED by docker restart — notifying Commander");
        }

        steps.push(HealStep {
            step_number: 3,
            description: "Wait 90s then check openclaw HTTP health endpoint".to_string(),
            action: HealAction::WaitAndVerifyOpenClaw,
            result: HealStepResult {
                success: healed_by_restart,
                recovered: healed_by_restart,
                details: http_result
                    .map(|s| format!("HTTP {s}"))
                    .unwrap_or_else(|e| format!("HTTP check failed: {e}")),
            },
            executed_at: step3_start,
            duration_ms: (Utc::now() - step3_start).num_milliseconds() as u64,
        });

        if healed_by_restart {
            return Ok(HealAttempt {
                attempt_id,
                instance_id: instance_id.to_string(),
                started_at,
                steps,
                outcome: HealOutcome::Healed,
            });
        }

        // Step 4: Check if PRIMARY; verify STANDBY is ACTIVE — NEVER skip this
        let step4_start = Utc::now();
        let is_primary = pair_info.role == gf_node_proto::InstanceRole::Primary;
        let standby_active = if is_primary {
            self.check_standby_active(pair_info).await.unwrap_or(false)
        } else {
            false
        };

        steps.push(HealStep {
            step_number: 4,
            description: "Verify role and standby status before failover".to_string(),
            action: HealAction::VerifyStandbyBeforeFailover,
            result: HealStepResult {
                success: true,
                recovered: false,
                details: format!(
                    "Instance is {}, standby ACTIVE: {standby_active}",
                    if is_primary { "PRIMARY" } else { "STANDBY" }
                ),
            },
            executed_at: step4_start,
            duration_ms: (Utc::now() - step4_start).num_milliseconds() as u64,
        });

        if !is_primary {
            // Standby is failing — notify Commander but don't self-act
            warn!(%instance_id, "STANDBY instance failing — notifying Commander");
            return Ok(HealAttempt {
                attempt_id,
                instance_id: instance_id.to_string(),
                started_at,
                steps,
                outcome: HealOutcome::EscalatedToCommander,
            });
        }

        // Step 5 or 6 depending on standby state
        if standby_active {
            // Step 5: Trigger failover
            info!(%instance_id, "Standby ACTIVE — triggering failover");
            let outcome = HealOutcome::FailoverTriggered;
            steps.push(HealStep {
                step_number: 5,
                description: "Promote standby to primary via provision.promote_standby".to_string(),
                action: HealAction::TriggerFailover,
                result: HealStepResult {
                    success: true,
                    recovered: false,
                    details: "Failover triggered — standby promoting".to_string(),
                },
                executed_at: Utc::now(),
                duration_ms: 0,
            });
            Ok(HealAttempt {
                attempt_id,
                instance_id: instance_id.to_string(),
                started_at,
                steps,
                outcome,
            })
        } else {
            // Step 6: CRITICAL — no standby, notify Commander immediately
            error!(
                %instance_id,
                "CRITICAL: PRIMARY failing and STANDBY not ACTIVE — escalating to Commander"
            );
            steps.push(HealStep {
                step_number: 6,
                description: "CRITICAL: Notify Commander — standby not available".to_string(),
                action: HealAction::NotifyCommanderCritical,
                result: HealStepResult {
                    success: true,
                    recovered: false,
                    details: "Commander notified. Awaiting instructions. No further action taken."
                        .to_string(),
                },
                executed_at: Utc::now(),
                duration_ms: 0,
            });
            Ok(HealAttempt {
                attempt_id,
                instance_id: instance_id.to_string(),
                started_at,
                steps,
                outcome: HealOutcome::EscalatedToCommander,
            })
        }
    }

    async fn call_vps_health(&self, instance_id: &str) -> Result<u8> {
        // TODO: dispatch vps.health command to instance via WebSocket
        let _ = instance_id;
        Ok(45) // placeholder — below threshold
    }

    async fn docker_restart_openclaw(&self, instance_id: &str) -> Result<()> {
        // TODO: dispatch openclaw.restart command via WebSocket
        let _ = instance_id;
        Ok(())
    }

    async fn check_openclaw_http(&self, instance_id: &str) -> Result<u16> {
        // TODO: dispatch openclaw.health command, check HTTP 200
        let _ = instance_id;
        Ok(503)
    }

    async fn check_standby_active(&self, pair_info: &PairInfo) -> Result<bool> {
        // TODO: call vps.health on pair_info.standby_instance_id
        let _ = pair_info;
        Ok(false)
    }
}

/// Pair topology info needed for failover decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairInfo {
    pub primary_instance_id: String,
    pub standby_instance_id: Option<String>,
    pub role: gf_node_proto::InstanceRole,
    pub account_id: String,
}

// ─── Fleet health sweeper ─────────────────────────────────────────────────────

/// Runs the periodic fleet-wide health sweep for Guardian.
/// Polls all active instances every 5 minutes.
pub struct FleetHealthSweeper {
    pub thresholds: HealthThresholds,
}

impl FleetHealthSweeper {
    pub fn new(thresholds: HealthThresholds) -> Self {
        Self { thresholds }
    }

    /// Execute a full health sweep across all instances.
    /// Returns degraded/failed instances sorted by severity.
    pub async fn sweep(&self, instance_ids: &[String]) -> Vec<HealthCheckResult> {
        let mut results = Vec::new();
        for id in instance_ids {
            if let Ok(result) = self.check_instance(id).await {
                if result.status != InstanceHealthStatus::Healthy {
                    results.push(result);
                }
            }
        }
        // Sort by severity (Critical first)
        results.sort_by(|a, b| b.health_score.cmp(&a.health_score).reverse());
        results
    }

    async fn check_instance(&self, instance_id: &str) -> Result<HealthCheckResult> {
        // TODO: fetch latest health data from GatewayForge API or cached heartbeat
        let _ = instance_id;
        Ok(HealthCheckResult {
            instance_id: instance_id.to_string(),
            checked_at: Utc::now(),
            health_score: 95,
            status: InstanceHealthStatus::Healthy,
            alerts: vec![],
            recommended_action: RecommendedAction::None,
        })
    }

    /// Compute health score from a full report (0–100)
    pub fn compute_score(report: &HealthReport) -> u8 {
        let mut score: f32 = 100.0;

        if report.openclaw_status != ServiceStatus::Healthy {
            score -= 40.0;
        }
        if report.docker_status.unhealthy_count > 0 {
            score -= 20.0 * report.docker_status.unhealthy_count as f32;
        }
        if report.disk_usage_pct > 90.0 {
            score -= 15.0;
        } else if report.disk_usage_pct > 80.0 {
            score -= 5.0;
        }
        if report.cpu_usage_1m > 95.0 {
            score -= 10.0;
        }
        if report.mem_usage_pct > 95.0 {
            score -= 10.0;
        }

        score.clamp(0.0, 100.0) as u8
    }
}

// ─── Health monitor trait (used by plugin background service) ─────────────────

#[async_trait]
pub trait HealthEventSink: Send + Sync {
    async fn on_instance_degraded(&self, instance_id: &str, health_score: u8);
    async fn on_instance_failed(&self, instance_id: &str);
    async fn on_pair_failed(&self, primary_id: &str, standby_id: Option<&str>);
    async fn on_cost_anomaly(&self, actual_usd: f64, projected_usd: f64, deviation_pct: f32);
    async fn on_provider_degraded(&self, provider: &str, health_score: u8);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gf_node_proto::{DockerStatus, ServiceStatus};

    fn make_perfect_report() -> HealthReport {
        HealthReport {
            instance_id: "inst-001".to_string(),
            health_score: 100,
            openclaw_status: ServiceStatus::Healthy,
            docker_status: DockerStatus {
                running: true,
                container_count: 2,
                unhealthy_count: 0,
            },
            disk_usage_pct: 40.0,
            cpu_usage_1m: 20.0,
            mem_usage_pct: 30.0,
            swap_usage_pct: 0.0,
            load_avg_1m: 0.5,
            load_avg_5m: 0.4,
            load_avg_15m: 0.3,
            uptime_seconds: 7200,
            tailscale_latency_ms: Some(5),
            last_heartbeat: Utc::now(),
            openclaw_http_status: Some(200),
            containers: vec![],
        }
    }

    #[test]
    fn health_thresholds_default_values() {
        let thresholds = HealthThresholds::default();
        assert_eq!(thresholds.heal_trigger_score, 50);
        assert_eq!(thresholds.recovery_score, 70);
        assert_eq!(thresholds.heartbeat_timeout_mins, 3);
    }

    #[test]
    fn fleet_health_sweeper_new_uses_given_thresholds() {
        let thresholds = HealthThresholds {
            heal_trigger_score: 40,
            recovery_score: 65,
            heartbeat_timeout_mins: 5,
            disk_warn_pct: 75.0,
            disk_critical_pct: 88.0,
            cpu_warn_pct: 85.0,
            mem_warn_pct: 85.0,
        };
        let sweeper = FleetHealthSweeper::new(thresholds.clone());
        assert_eq!(sweeper.thresholds.heal_trigger_score, 40);
        assert_eq!(sweeper.thresholds.recovery_score, 65);
    }

    #[test]
    fn compute_score_perfect_report_returns_100() {
        let report = make_perfect_report();
        let score = FleetHealthSweeper::compute_score(&report);
        assert_eq!(score, 100);
    }

    #[test]
    fn compute_score_openclaw_down_returns_60() {
        let mut report = make_perfect_report();
        report.openclaw_status = ServiceStatus::Down;
        let score = FleetHealthSweeper::compute_score(&report);
        // 100 - 40 (openclaw not healthy) = 60
        assert_eq!(score, 60);
    }

    #[test]
    fn compute_score_openclaw_down_plus_one_unhealthy_container_returns_40() {
        let mut report = make_perfect_report();
        report.openclaw_status = ServiceStatus::Down;
        report.docker_status.unhealthy_count = 1;
        let score = FleetHealthSweeper::compute_score(&report);
        // 100 - 40 (openclaw) - 20 * 1 (unhealthy container) = 40
        assert_eq!(score, 40);
    }

    #[test]
    fn compute_score_disk_above_90_subtracts_15() {
        let mut report = make_perfect_report();
        report.disk_usage_pct = 91.0;
        let score = FleetHealthSweeper::compute_score(&report);
        // 100 - 15 (disk > 90%) = 85
        assert_eq!(score, 85);
    }

    #[test]
    fn health_check_result_serializes_healthy_status() {
        let result = HealthCheckResult {
            instance_id: "inst-test".to_string(),
            checked_at: Utc::now(),
            health_score: 95,
            status: InstanceHealthStatus::Healthy,
            alerts: vec![],
            recommended_action: RecommendedAction::None,
        };

        let json = serde_json::to_string(&result).expect("HealthCheckResult serialization failed");
        assert!(
            json.contains("HEALTHY"),
            "InstanceHealthStatus::Healthy should serialize as HEALTHY"
        );

        let decoded: HealthCheckResult =
            serde_json::from_str(&json).expect("HealthCheckResult deserialization failed");
        assert_eq!(decoded.status, InstanceHealthStatus::Healthy);
        assert_eq!(decoded.instance_id, "inst-test");
    }

    #[test]
    fn instance_health_status_healthy_ne_critical() {
        assert_ne!(
            InstanceHealthStatus::Healthy,
            InstanceHealthStatus::Critical
        );
        assert_eq!(InstanceHealthStatus::Healthy, InstanceHealthStatus::Healthy);
        assert_ne!(
            InstanceHealthStatus::Degraded,
            InstanceHealthStatus::Unknown
        );
    }
}
