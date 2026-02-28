//! Fleet health scoring, auto-heal decision engine, and failover orchestration.
//!
//! Implements the 6-step auto-heal sequence from the PRD with embedded safety rules:
//! - Never teardown PRIMARY without confirming STANDBY is ACTIVE
//! - Escalate to Commander for critical failures rather than self-deleting

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use claw_proto::{HealthReport, InstanceRole, InstanceState, ServiceStatus};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// ─── Health Thresholds ────────────────────────────────────────────────────────

/// Configurable thresholds for health decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthThresholds {
    /// Score below this → DEGRADED
    pub degraded_score: u8,
    /// Score below this → CRITICAL (triggers auto-heal)
    pub critical_score: u8,
    /// CPU usage % above this → alert
    pub cpu_alert_pct: f32,
    /// Memory usage % above this → alert
    pub mem_alert_pct: f32,
    /// Disk usage % above this → alert
    pub disk_alert_pct: f32,
    /// Minutes without heartbeat → alert
    pub heartbeat_timeout_mins: u64,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            degraded_score: 70,
            critical_score: 40,
            cpu_alert_pct: 90.0,
            mem_alert_pct: 85.0,
            disk_alert_pct: 85.0,
            heartbeat_timeout_mins: 5,
        }
    }
}

// ─── Alert types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthAlert {
    pub alert_type: AlertType,
    pub severity: AlertSeverity,
    pub message: String,
    pub threshold: Option<f32>,
    pub actual: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

// ─── Recommended Actions ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedAction {
    None,
    Monitor,
    AutoHeal,
    Failover,
    EscalateToCommander,
}

// ─── Health Check Result ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub instance_id: String,
    pub health_score: u8,
    pub status: InstanceState,
    pub alerts: Vec<HealthAlert>,
    pub recommended_action: RecommendedAction,
    pub checked_at: DateTime<Utc>,
}

// ─── Health Score Engine ──────────────────────────────────────────────────────

/// Compute a 0-100 health score from a health report.
/// Deductions:
/// - OpenClaw down: -40
/// - Docker down: -20
/// - Tailscale disconnected: -15
/// - CPU > 90%: -10
/// - Memory > 85%: -10
/// - Disk > 85%: -10
pub fn compute_health_score(report: &HealthReport) -> u8 {
    let mut score: i32 = 100;

    if report.openclaw_status != ServiceStatus::Healthy {
        score -= 40;
    }
    if !report.docker_running {
        score -= 20;
    }
    if !report.tailscale_connected {
        score -= 15;
    }
    if report.cpu_usage_1m > 90.0 {
        score -= 10;
    }
    if report.mem_usage_pct > 85.0 {
        score -= 10;
    }
    if report.disk_usage_pct > 85.0 {
        score -= 10;
    }

    score.clamp(0, 100) as u8
}

/// Evaluate health alerts from a report.
pub fn evaluate_alerts(report: &HealthReport, thresholds: &HealthThresholds) -> Vec<HealthAlert> {
    let mut alerts = Vec::new();

    if report.openclaw_status != ServiceStatus::Healthy {
        alerts.push(HealthAlert {
            alert_type: AlertType::OpenClawDown,
            severity: AlertSeverity::Critical,
            message: "OpenClaw gateway is not healthy".to_string(),
            threshold: None,
            actual: None,
        });
    }

    if !report.docker_running {
        alerts.push(HealthAlert {
            alert_type: AlertType::DockerDown,
            severity: AlertSeverity::Critical,
            message: "Docker daemon is not running".to_string(),
            threshold: None,
            actual: None,
        });
    }

    if !report.tailscale_connected {
        alerts.push(HealthAlert {
            alert_type: AlertType::TailscaleDisconnected,
            severity: AlertSeverity::Warning,
            message: "Tailscale VPN is disconnected".to_string(),
            threshold: None,
            actual: None,
        });
    }

    if report.cpu_usage_1m > thresholds.cpu_alert_pct {
        alerts.push(HealthAlert {
            alert_type: AlertType::CpuUsageHigh,
            severity: AlertSeverity::Warning,
            message: format!("CPU usage {:.1}% exceeds threshold", report.cpu_usage_1m),
            threshold: Some(thresholds.cpu_alert_pct),
            actual: Some(report.cpu_usage_1m),
        });
    }

    if report.mem_usage_pct > thresholds.mem_alert_pct {
        alerts.push(HealthAlert {
            alert_type: AlertType::MemUsageHigh,
            severity: AlertSeverity::Warning,
            message: format!(
                "Memory usage {:.1}% exceeds threshold",
                report.mem_usage_pct
            ),
            threshold: Some(thresholds.mem_alert_pct),
            actual: Some(report.mem_usage_pct),
        });
    }

    if report.disk_usage_pct > thresholds.disk_alert_pct {
        alerts.push(HealthAlert {
            alert_type: AlertType::DiskUsageHigh,
            severity: AlertSeverity::Warning,
            message: format!("Disk usage {:.1}% exceeds threshold", report.disk_usage_pct),
            threshold: Some(thresholds.disk_alert_pct),
            actual: Some(report.disk_usage_pct),
        });
    }

    alerts
}

/// Determine the recommended action based on health score.
pub fn recommend_action(score: u8, thresholds: &HealthThresholds) -> RecommendedAction {
    if score >= thresholds.degraded_score {
        RecommendedAction::None
    } else if score >= thresholds.critical_score {
        RecommendedAction::Monitor
    } else if score >= 20 {
        RecommendedAction::AutoHeal
    } else {
        RecommendedAction::EscalateToCommander
    }
}

// ─── Auto-Heal Engine ─────────────────────────────────────────────────────────

/// Result of an auto-heal attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoHealResult {
    pub instance_id: String,
    pub success: bool,
    pub steps_completed: Vec<AutoHealStep>,
    pub final_health_score: Option<u8>,
    pub action_taken: AutoHealAction,
    pub escalated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoHealStep {
    VerifiedHealth,
    DockerRestartedOpenclaw,
    WaitedForRecovery,
    VerifiedRecovery,
    CheckedPairRole,
    VerifiedStandbyActive,
    TriggeredFailover,
    EscalatedToCommander,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoHealAction {
    Recovered,
    FailoverTriggered,
    EscalatedCritical,
    StandbyNotReady,
}

/// The auto-heal decision — produced without performing I/O.
///
/// The actual execution (docker restart, failover API calls) is done
/// by the caller (Commander agent) which has the necessary credentials.
pub struct AutoHealDecision {
    pub instance_id: String,
    pub role: InstanceRole,
    pub health_score: u8,
    pub openclaw_down: bool,
    pub docker_down: bool,
}

impl AutoHealDecision {
    /// Determine what the auto-heal engine recommends.
    ///
    /// Returns the sequence of steps to execute.
    /// The caller must verify STANDBY is ACTIVE before triggering failover.
    pub fn recommend(&self) -> Vec<AutoHealStep> {
        let mut steps = vec![AutoHealStep::VerifiedHealth];

        if self.openclaw_down || self.docker_down {
            steps.push(AutoHealStep::DockerRestartedOpenclaw);
            steps.push(AutoHealStep::WaitedForRecovery);
            steps.push(AutoHealStep::VerifiedRecovery);
        }

        if self.health_score < 40 {
            steps.push(AutoHealStep::CheckedPairRole);
            if self.role == InstanceRole::Primary {
                // SAFETY: Never failover without verifying standby first
                steps.push(AutoHealStep::VerifiedStandbyActive);
                steps.push(AutoHealStep::TriggeredFailover);
            } else {
                steps.push(AutoHealStep::EscalatedToCommander);
            }
        }

        steps
    }
}

// ─── Failover Engine ──────────────────────────────────────────────────────────

/// A failover request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverRequest {
    pub account_id: String,
    pub failed_instance_id: String,
    pub standby_instance_id: String,
    pub trigger_reason: FailoverTrigger,
    pub triggered_by: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailoverTrigger {
    AutoHeal,
    ProviderOutage,
    ManualByCommander,
    PlannedMaintenance,
}

/// Result of a failover operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverResult {
    pub account_id: String,
    pub success: bool,
    pub old_primary: String,
    pub new_primary: String,
    pub promotion_duration_ms: u64,
    pub steps: Vec<FailoverStepRecord>,
    pub reprovisioning_scheduled: bool,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverStepRecord {
    pub step: FailoverStepType,
    pub success: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailoverStepType {
    VerifyStandby,
    UpdatePairStatus,
    NotifyGateway,
    UpdateRouting,
    ScheduleReprovisioning,
    NotifyCommander,
}

/// Safety invariant checker: verify standby is ACTIVE before failover.
///
/// # Safety
///
/// This check MUST pass before any failover is initiated.
/// A user must ALWAYS have exactly one ACTIVE gateway.
pub fn verify_standby_precondition(standby_state: InstanceState) -> bool {
    standby_state == InstanceState::Active
}

// ─── Fleet Health Sweep ───────────────────────────────────────────────────────

/// Summary of a fleet-wide health sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetHealthSweepResult {
    pub total_instances: u32,
    pub healthy: u32,
    pub degraded: u32,
    pub critical: u32,
    pub auto_heal_triggered: u32,
    pub failovers_triggered: u32,
    pub escalated_to_commander: u32,
    pub swept_at: DateTime<Utc>,
}

impl FleetHealthSweepResult {
    pub fn new() -> Self {
        Self {
            total_instances: 0,
            healthy: 0,
            degraded: 0,
            critical: 0,
            auto_heal_triggered: 0,
            failovers_triggered: 0,
            escalated_to_commander: 0,
            swept_at: Utc::now(),
        }
    }

    pub fn fleet_health_score(&self) -> u8 {
        if self.total_instances == 0 {
            return 100;
        }
        let healthy_pct = self.healthy as f32 / self.total_instances as f32;
        (healthy_pct * 100.0) as u8
    }
}

impl Default for FleetHealthSweepResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Process a batch of health reports and produce sweep results.
pub fn sweep_fleet(
    reports: &[HealthReport],
    thresholds: &HealthThresholds,
) -> FleetHealthSweepResult {
    let mut result = FleetHealthSweepResult::new();
    result.total_instances = reports.len() as u32;

    for report in reports {
        let score = compute_health_score(report);
        let action = recommend_action(score, thresholds);

        match action {
            RecommendedAction::None => result.healthy += 1,
            RecommendedAction::Monitor => {
                result.degraded += 1;
                info!(instance = %report.instance_id, score, "instance degraded - monitoring");
            }
            RecommendedAction::AutoHeal => {
                result.critical += 1;
                result.auto_heal_triggered += 1;
                warn!(instance = %report.instance_id, score, "instance critical - auto-heal triggered");
            }
            RecommendedAction::Failover => {
                result.critical += 1;
                result.failovers_triggered += 1;
                warn!(instance = %report.instance_id, score, "instance critical - failover triggered");
            }
            RecommendedAction::EscalateToCommander => {
                result.critical += 1;
                result.escalated_to_commander += 1;
                warn!(instance = %report.instance_id, score, "instance critical - escalating to Commander");
            }
        }
    }

    result
}

// ─── Failover State Machine ────────────────────────────────────────────────────

/// Maximum heal attempts before escalating to Commander.
pub const MAX_HEAL_ATTEMPTS: u32 = 3;

/// States of the failover state machine for a single instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailoverState {
    /// Instance is healthy — normal operation.
    Normal,
    /// Score is degraded; watching for further decline.
    Monitoring {
        since: DateTime<Utc>,
        low_score_count: u32,
    },
    /// Attempting to self-heal (docker restart, process restart, etc.).
    Healing {
        attempt: u32,
        started_at: DateTime<Utc>,
    },
    /// Actively failing over to standby.
    FailingOver { started_at: DateTime<Utc> },
    /// Failover/heal exhausted — human intervention required.
    Failed {
        since: DateTime<Utc>,
        reason: String,
    },
    /// Successfully recovered after heal or failover.
    Recovered { at: DateTime<Utc> },
}

impl FailoverState {
    /// Construct the initial state.
    pub fn new() -> Self {
        Self::Normal
    }

    /// True if the machine is in a terminal/stable good state.
    pub fn is_stable(&self) -> bool {
        matches!(self, Self::Normal | Self::Recovered { .. })
    }

    /// True if human attention is needed.
    pub fn needs_escalation(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

impl Default for FailoverState {
    fn default() -> Self {
        Self::new()
    }
}

/// Drives the failover state machine based on incoming health scores.
///
/// Callers should call `transition` on each health check cycle. The returned
/// `FailoverTransition` tells the caller what action to perform.
pub struct FailoverStateMachine {
    pub instance_id: String,
    pub role: InstanceRole,
    pub state: FailoverState,
    thresholds: HealthThresholds,
}

/// What the state machine asks the caller to do after a transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailoverTransition {
    /// No action needed.
    NoAction,
    /// Continue monitoring — score is low but within tolerable range.
    Monitor,
    /// Perform a Docker / OpenClaw restart.
    AttemptDockerRestart { attempt: u32 },
    /// Verify standby is ACTIVE, then initiate failover.
    InitiateFailover,
    /// Escalate to Commander — heal attempts exhausted or standby not ready.
    EscalateToCommander { reason: String },
    /// Instance has recovered — log and resume normal monitoring.
    LogRecovered,
}

impl FailoverStateMachine {
    pub fn new(instance_id: String, role: InstanceRole, thresholds: HealthThresholds) -> Self {
        Self {
            instance_id,
            role,
            state: FailoverState::Normal,
            thresholds,
        }
    }

    /// Process a new health score and determine the next transition.
    ///
    /// PRD §5.3 six-step sequence:
    /// 1. Verify: if score recovers >degraded → back to Normal.
    /// 2. Docker restart: attempt restart.
    /// 3. Wait + recheck (caller handles the wait).
    /// 4. Check if PRIMARY of a pair.
    /// 5. If standby active → trigger failover.
    /// 6. If standby NOT active → CRITICAL alert to Commander.
    pub fn transition(&mut self, health_score: u8, standby_active: bool) -> FailoverTransition {
        let now = Utc::now();

        match &self.state.clone() {
            // ── Normal ──────────────────────────────────────────────────────
            FailoverState::Normal => {
                if health_score < self.thresholds.critical_score {
                    self.state = FailoverState::Healing {
                        attempt: 1,
                        started_at: now,
                    };
                    info!(instance = %self.instance_id, score = health_score, "entering healing state (attempt 1)");
                    return FailoverTransition::AttemptDockerRestart { attempt: 1 };
                }
                if health_score < self.thresholds.degraded_score {
                    self.state = FailoverState::Monitoring {
                        since: now,
                        low_score_count: 1,
                    };
                    return FailoverTransition::Monitor;
                }
                FailoverTransition::NoAction
            }

            // ── Monitoring ──────────────────────────────────────────────────
            FailoverState::Monitoring {
                since,
                low_score_count,
            } => {
                if health_score >= self.thresholds.degraded_score {
                    info!(instance = %self.instance_id, "health recovered from monitoring");
                    self.state = FailoverState::Recovered { at: now };
                    return FailoverTransition::LogRecovered;
                }
                if health_score < self.thresholds.critical_score {
                    self.state = FailoverState::Healing {
                        attempt: 1,
                        started_at: now,
                    };
                    warn!(instance = %self.instance_id, score = health_score, since = %since, "health dropped to critical from monitoring");
                    return FailoverTransition::AttemptDockerRestart { attempt: 1 };
                }
                self.state = FailoverState::Monitoring {
                    since: *since,
                    low_score_count: low_score_count + 1,
                };
                FailoverTransition::Monitor
            }

            // ── Healing ─────────────────────────────────────────────────────
            FailoverState::Healing { attempt, .. } => {
                if health_score >= self.thresholds.degraded_score {
                    info!(instance = %self.instance_id, attempt, "healed successfully");
                    self.state = FailoverState::Recovered { at: now };
                    return FailoverTransition::LogRecovered;
                }

                let next_attempt = attempt + 1;

                if next_attempt > MAX_HEAL_ATTEMPTS {
                    // Exhausted heal attempts — decide failover or escalate
                    if self.role == InstanceRole::Primary {
                        if standby_active {
                            warn!(instance = %self.instance_id, "heal exhausted, primary — initiating failover");
                            self.state = FailoverState::FailingOver { started_at: now };
                            return FailoverTransition::InitiateFailover;
                        } else {
                            warn!(instance = %self.instance_id, "heal exhausted, standby NOT active — escalating");
                            self.state = FailoverState::Failed {
                                since: now,
                                reason: "heal exhausted and standby not active".to_string(),
                            };
                            return FailoverTransition::EscalateToCommander {
                                reason: "heal exhausted and standby not active".to_string(),
                            };
                        }
                    } else {
                        // Standby instance failed — escalate, don't self-failover
                        warn!(instance = %self.instance_id, "standby heal exhausted — escalating to Commander");
                        self.state = FailoverState::Failed {
                            since: now,
                            reason: "standby heal exhausted".to_string(),
                        };
                        return FailoverTransition::EscalateToCommander {
                            reason: "standby heal exhausted".to_string(),
                        };
                    }
                }

                warn!(instance = %self.instance_id, attempt = next_attempt, "re-attempting docker restart");
                self.state = FailoverState::Healing {
                    attempt: next_attempt,
                    started_at: now,
                };
                FailoverTransition::AttemptDockerRestart {
                    attempt: next_attempt,
                }
            }

            // ── FailingOver ─────────────────────────────────────────────────
            FailoverState::FailingOver { .. } => {
                if health_score >= self.thresholds.degraded_score {
                    self.state = FailoverState::Recovered { at: now };
                    return FailoverTransition::LogRecovered;
                }
                // Still in failover — caller is handling it
                FailoverTransition::NoAction
            }

            // ── Failed ──────────────────────────────────────────────────────
            FailoverState::Failed { reason, .. } => {
                // Re-escalate on each cycle until operator clears
                FailoverTransition::EscalateToCommander {
                    reason: reason.clone(),
                }
            }

            // ── Recovered ───────────────────────────────────────────────────
            FailoverState::Recovered { .. } => {
                if health_score >= self.thresholds.degraded_score {
                    self.state = FailoverState::Normal;
                    return FailoverTransition::NoAction;
                }
                // Relapsed — start fresh monitoring cycle
                self.state = FailoverState::Monitoring {
                    since: now,
                    low_score_count: 1,
                };
                FailoverTransition::Monitor
            }
        }
    }

    /// Force-reset the machine to Normal (operator action after clearing failure).
    pub fn reset(&mut self) {
        self.state = FailoverState::Normal;
        info!(instance = %self.instance_id, "failover state machine reset by operator");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_proto::{InstanceTier, ServiceStatus, VpsProvider};

    fn make_healthy_report(instance_id: &str) -> HealthReport {
        HealthReport {
            instance_id: instance_id.to_string(),
            account_id: "acc-1".to_string(),
            provider: VpsProvider::Hetzner,
            region: "eu-hetzner-nbg1".to_string(),
            tier: InstanceTier::Standard,
            role: InstanceRole::Primary,
            state: InstanceState::Active,
            health_score: 95,
            openclaw_status: ServiceStatus::Healthy,
            openclaw_http_status: Some(200),
            docker_running: true,
            tailscale_connected: true,
            tailscale_latency_ms: Some(2.5),
            cpu_usage_1m: 20.0,
            mem_usage_pct: 40.0,
            disk_usage_pct: 30.0,
            swap_usage_pct: 0.0,
            load_avg_1m: 0.5,
            load_avg_5m: 0.4,
            load_avg_15m: 0.3,
            uptime_secs: 86400,
            bytes_sent_per_sec: 1024.0,
            bytes_recv_per_sec: 2048.0,
            reported_at: Utc::now(),
        }
    }

    #[test]
    fn test_compute_health_score_healthy() {
        let report = make_healthy_report("i-1");
        let score = compute_health_score(&report);
        assert_eq!(score, 100);
    }

    #[test]
    fn test_compute_health_score_openclaw_down() {
        let mut report = make_healthy_report("i-2");
        report.openclaw_status = ServiceStatus::Down;
        let score = compute_health_score(&report);
        assert_eq!(score, 60); // 100 - 40
    }

    #[test]
    fn test_compute_health_score_multiple_issues() {
        let mut report = make_healthy_report("i-3");
        report.openclaw_status = ServiceStatus::Down;
        report.docker_running = false;
        report.tailscale_connected = false;
        let score = compute_health_score(&report);
        assert_eq!(score, 25); // 100 - 40 - 20 - 15
    }

    #[test]
    fn test_health_score_minimum_zero() {
        let mut report = make_healthy_report("i-4");
        report.openclaw_status = ServiceStatus::Down;
        report.docker_running = false;
        report.tailscale_connected = false;
        report.cpu_usage_1m = 95.0;
        report.mem_usage_pct = 90.0;
        report.disk_usage_pct = 90.0;
        let score = compute_health_score(&report);
        assert_eq!(score, 0); // would be -5, clamped to 0
    }

    #[test]
    fn test_recommend_action_healthy() {
        let thresholds = HealthThresholds::default();
        assert_eq!(recommend_action(100, &thresholds), RecommendedAction::None);
        assert_eq!(recommend_action(80, &thresholds), RecommendedAction::None);
        assert_eq!(recommend_action(70, &thresholds), RecommendedAction::None);
    }

    #[test]
    fn test_recommend_action_degraded() {
        let thresholds = HealthThresholds::default();
        assert_eq!(
            recommend_action(60, &thresholds),
            RecommendedAction::Monitor
        );
        assert_eq!(
            recommend_action(41, &thresholds),
            RecommendedAction::Monitor
        );
    }

    #[test]
    fn test_recommend_action_critical() {
        let thresholds = HealthThresholds::default();
        assert_eq!(
            recommend_action(35, &thresholds),
            RecommendedAction::AutoHeal
        );
        assert_eq!(
            recommend_action(20, &thresholds),
            RecommendedAction::AutoHeal
        );
    }

    #[test]
    fn test_recommend_action_escalate() {
        let thresholds = HealthThresholds::default();
        assert_eq!(
            recommend_action(0, &thresholds),
            RecommendedAction::EscalateToCommander
        );
        assert_eq!(
            recommend_action(10, &thresholds),
            RecommendedAction::EscalateToCommander
        );
    }

    #[test]
    fn test_verify_standby_precondition() {
        // SAFETY: Standby must be ACTIVE before failover
        assert!(verify_standby_precondition(InstanceState::Active));
        assert!(!verify_standby_precondition(InstanceState::Degraded));
        assert!(!verify_standby_precondition(InstanceState::Failed));
        assert!(!verify_standby_precondition(InstanceState::Unknown));
    }

    #[test]
    fn test_auto_heal_decision_openclaw_down() {
        let decision = AutoHealDecision {
            instance_id: "i-test".to_string(),
            role: InstanceRole::Primary,
            health_score: 60,
            openclaw_down: true,
            docker_down: false,
        };

        let steps = decision.recommend();
        assert!(steps.contains(&AutoHealStep::DockerRestartedOpenclaw));
        assert!(steps.contains(&AutoHealStep::VerifiedRecovery));
    }

    #[test]
    fn test_auto_heal_decision_critical_primary() {
        let decision = AutoHealDecision {
            instance_id: "i-test".to_string(),
            role: InstanceRole::Primary,
            health_score: 10,
            openclaw_down: true,
            docker_down: false,
        };

        let steps = decision.recommend();
        // SAFETY: Must verify standby before failover
        assert!(steps.contains(&AutoHealStep::VerifiedStandbyActive));
        assert!(steps.contains(&AutoHealStep::TriggeredFailover));
    }

    #[test]
    fn test_auto_heal_decision_critical_standby() {
        let decision = AutoHealDecision {
            instance_id: "i-standby".to_string(),
            role: InstanceRole::Standby,
            health_score: 10,
            openclaw_down: true,
            docker_down: false,
        };

        let steps = decision.recommend();
        // Standby can't failover to itself — escalate
        assert!(steps.contains(&AutoHealStep::EscalatedToCommander));
        assert!(!steps.contains(&AutoHealStep::TriggeredFailover));
    }

    #[test]
    fn test_fleet_health_sweep() {
        let thresholds = HealthThresholds::default();
        let reports = vec![make_healthy_report("i-1"), make_healthy_report("i-2"), {
            let mut r = make_healthy_report("i-3");
            r.openclaw_status = ServiceStatus::Down;
            r
        }];

        let result = sweep_fleet(&reports, &thresholds);
        assert_eq!(result.total_instances, 3);
        assert_eq!(result.healthy, 2);
        assert!(result.degraded + result.critical > 0);
    }

    #[test]
    fn test_fleet_health_score() {
        let mut result = FleetHealthSweepResult::new();
        result.total_instances = 10;
        result.healthy = 8;

        assert_eq!(result.fleet_health_score(), 80);
    }

    #[test]
    fn test_evaluate_alerts() {
        let thresholds = HealthThresholds::default();
        let mut report = make_healthy_report("i-alert");
        report.openclaw_status = ServiceStatus::Down;
        report.disk_usage_pct = 90.0;

        let alerts = evaluate_alerts(&report, &thresholds);
        assert!(
            alerts
                .iter()
                .any(|a| a.alert_type == AlertType::OpenClawDown)
        );
        assert!(
            alerts
                .iter()
                .any(|a| a.alert_type == AlertType::DiskUsageHigh)
        );
    }

    // ─── FailoverStateMachine tests ───────────────────────────────────────────

    fn make_fsm(role: InstanceRole) -> FailoverStateMachine {
        FailoverStateMachine::new("i-test".to_string(), role, HealthThresholds::default())
    }

    #[test]
    fn test_fsm_healthy_stays_normal() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        let t = fsm.transition(95, true);
        assert_eq!(t, FailoverTransition::NoAction);
        assert_eq!(fsm.state, FailoverState::Normal);
    }

    #[test]
    fn test_fsm_degraded_enters_monitoring() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        // score 60 < degraded_score (70) but >= critical_score (40)
        let t = fsm.transition(60, true);
        assert_eq!(t, FailoverTransition::Monitor);
        assert!(matches!(fsm.state, FailoverState::Monitoring { .. }));
    }

    #[test]
    fn test_fsm_critical_enters_healing() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        // score 30 < critical_score (40)
        let t = fsm.transition(30, true);
        assert_eq!(t, FailoverTransition::AttemptDockerRestart { attempt: 1 });
        assert!(matches!(
            fsm.state,
            FailoverState::Healing { attempt: 1, .. }
        ));
    }

    #[test]
    fn test_fsm_heals_on_recovery() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        fsm.transition(30, true); // → Healing attempt 1
        let t = fsm.transition(85, true); // recovered
        assert_eq!(t, FailoverTransition::LogRecovered);
        assert!(matches!(fsm.state, FailoverState::Recovered { .. }));
    }

    #[test]
    fn test_fsm_heal_exhausted_with_active_standby_triggers_failover() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        // Drive through 3 heal attempts without recovery
        fsm.transition(30, true); // attempt 1
        fsm.transition(25, true); // attempt 2
        fsm.transition(20, true); // attempt 3
        let t = fsm.transition(15, true); // exhausted → failover
        assert_eq!(t, FailoverTransition::InitiateFailover);
        assert!(matches!(fsm.state, FailoverState::FailingOver { .. }));
    }

    #[test]
    fn test_fsm_heal_exhausted_standby_not_active_escalates() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        fsm.transition(30, false); // attempt 1
        fsm.transition(25, false); // attempt 2
        fsm.transition(20, false); // attempt 3
        let t = fsm.transition(15, false); // exhausted → escalate
        assert!(matches!(t, FailoverTransition::EscalateToCommander { .. }));
        assert!(matches!(fsm.state, FailoverState::Failed { .. }));
    }

    #[test]
    fn test_fsm_standby_exhausted_escalates() {
        let mut fsm = make_fsm(InstanceRole::Standby);
        fsm.transition(30, false); // attempt 1
        fsm.transition(25, false); // attempt 2
        fsm.transition(20, false); // attempt 3
        let t = fsm.transition(15, false); // exhausted → escalate, no failover
        assert!(matches!(t, FailoverTransition::EscalateToCommander { .. }));
        // Standby should never trigger failover to itself
        assert!(!matches!(fsm.state, FailoverState::FailingOver { .. }));
    }

    #[test]
    fn test_fsm_reset() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        fsm.transition(30, false); // enter healing
        fsm.transition(25, false);
        fsm.transition(20, false);
        fsm.transition(15, false); // enter failed
        assert!(fsm.state.needs_escalation());
        fsm.reset();
        assert_eq!(fsm.state, FailoverState::Normal);
    }

    #[test]
    fn test_fsm_monitoring_recovers() {
        let mut fsm = make_fsm(InstanceRole::Primary);
        fsm.transition(60, true); // → Monitoring
        let t = fsm.transition(90, true); // recovered
        assert_eq!(t, FailoverTransition::LogRecovered);
    }

    #[test]
    fn test_fsm_max_heal_attempts_constant() {
        assert_eq!(MAX_HEAL_ATTEMPTS, 3);
    }
}
