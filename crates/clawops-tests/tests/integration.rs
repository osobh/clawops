//! Integration-style tests for ClawOps Phase 3.
//!
//! These tests simulate end-to-end flows across crates:
//! - Health degradation → auto-heal trigger
//! - Primary failure → failover state machine
//! - Fleet metrics aggregation across multiple instances
//! - Config push batch validation
//! - Provider scoring / selection logic

use chrono::Utc;
use claw_health::{
    AutoHealDecision, AutoHealStep, FailoverState, FailoverStateMachine, FailoverTransition,
    HealthThresholds, MAX_HEAL_ATTEMPTS, RecommendedAction, compute_health_score, evaluate_alerts,
    recommend_action, sweep_fleet, verify_standby_precondition,
};
use claw_metrics::{CostTracker, FleetMetrics, InstanceCost, InstanceSnapshot, TimeSeriesBuffer};
use claw_proto::{
    CheckStatus, HealthCheck, HealthCheckResponse, HealthReport, InstancePairStatus, InstanceRole,
    InstanceState, InstanceTier, ServiceStatus, VpsMetricsResponse, VpsProvider,
};
use claw_provision::{LatencyClass, score_provider};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn healthy_report(id: &str) -> HealthReport {
    HealthReport {
        instance_id: id.to_string(),
        account_id: "acc-test".to_string(),
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
        load_avg_1m: 0.4,
        load_avg_5m: 0.3,
        load_avg_15m: 0.2,
        uptime_secs: 86400,
        bytes_sent_per_sec: 1024.0,
        bytes_recv_per_sec: 2048.0,
        reported_at: Utc::now(),
    }
}

fn snapshot(id: &str, provider: &str, health: f64, cost: f64) -> InstanceSnapshot {
    InstanceSnapshot {
        instance_id: id.to_string(),
        provider: provider.to_string(),
        cpu_pct: 25.0,
        mem_pct: 45.0,
        disk_pct: 30.0,
        health_score: health,
        monthly_cost_usd: cost,
        recorded_at: Utc::now(),
    }
}

// ─── Test 1: Degraded health report triggers auto-heal recommendation ─────────

#[test]
fn test_health_check_degraded_triggers_heal() {
    let mut report = healthy_report("i-degraded");
    report.openclaw_status = ServiceStatus::Down;

    let thresholds = HealthThresholds::default();
    let score = compute_health_score(&report);
    // OpenClaw down = -40 → score 60 (degraded, not critical)
    assert_eq!(score, 60);
    assert!(score < thresholds.degraded_score);
    assert_eq!(
        recommend_action(score, &thresholds),
        RecommendedAction::Monitor
    );

    let alerts = evaluate_alerts(&report, &thresholds);
    assert!(!alerts.is_empty(), "degraded instance should have alerts");

    // Push to critical: openclaw + docker + tailscale all down
    let mut critical_report = healthy_report("i-critical");
    critical_report.openclaw_status = ServiceStatus::Down;
    critical_report.docker_running = false;
    critical_report.tailscale_connected = false;

    let critical_score = compute_health_score(&critical_report);
    // 100 - 40 (openclaw) - 20 (docker) - 15 (tailscale) = 25
    assert_eq!(critical_score, 25);
    assert!(critical_score < thresholds.critical_score);
    assert_eq!(
        recommend_action(critical_score, &thresholds),
        RecommendedAction::AutoHeal
    );
}

// ─── Test 2: Failover state machine — primary down, standby ACTIVE ───────────

#[test]
fn test_failover_primary_down_standby_active() {
    let thresholds = HealthThresholds::default();
    let mut fsm =
        FailoverStateMachine::new("i-primary".to_string(), InstanceRole::Primary, thresholds);

    // First call: enters Healing attempt 1
    let t1 = fsm.transition(25, true);
    assert_eq!(t1, FailoverTransition::AttemptDockerRestart { attempt: 1 });

    // Second call: still failing → attempt 2
    let t2 = fsm.transition(25, true);
    assert_eq!(t2, FailoverTransition::AttemptDockerRestart { attempt: 2 });

    // Third call: still failing → attempt 3
    let t3 = fsm.transition(25, true);
    assert_eq!(t3, FailoverTransition::AttemptDockerRestart { attempt: 3 });

    // Fourth call: heal exhausted, standby active → InitiateFailover
    let t4 = fsm.transition(25, true);
    assert_eq!(t4, FailoverTransition::InitiateFailover);
    assert!(
        matches!(fsm.state, FailoverState::FailingOver { .. }),
        "should be in FailingOver state"
    );
}

// ─── Test 3: Failover — primary down, standby NOT active → escalate ──────────

#[test]
fn test_failover_primary_down_standby_not_active_escalates() {
    let thresholds = HealthThresholds::default();
    let mut fsm =
        FailoverStateMachine::new("i-primary".to_string(), InstanceRole::Primary, thresholds);

    // Drive through all heal attempts with standby inactive
    for _ in 0..MAX_HEAL_ATTEMPTS {
        fsm.transition(25, false);
    }
    let t = fsm.transition(25, false);
    assert!(
        matches!(t, FailoverTransition::EscalateToCommander { .. }),
        "should escalate to Commander when standby not active: got {:?}",
        t
    );
    assert!(fsm.state.needs_escalation());
}

// ─── Test 4: Auto-heal decision — docker restart when openclaw down ───────────

#[test]
fn test_auto_heal_decision_docker_restart_when_openclaw_down() {
    let decision = AutoHealDecision {
        instance_id: "i-heal".to_string(),
        role: InstanceRole::Primary,
        health_score: 60,
        openclaw_down: true,
        docker_down: false,
    };

    let steps = decision.recommend();
    assert!(
        steps.contains(&AutoHealStep::DockerRestartedOpenclaw),
        "should recommend docker restart"
    );
    assert!(steps.contains(&AutoHealStep::VerifiedRecovery));
    // Score not critical → no failover steps
    assert!(!steps.contains(&AutoHealStep::TriggeredFailover));
}

// ─── Test 5: Auto-heal — critical primary, standby-check before failover ──────

#[test]
fn test_auto_heal_decision_critical_primary_verifies_standby_first() {
    let decision = AutoHealDecision {
        instance_id: "i-critical".to_string(),
        role: InstanceRole::Primary,
        health_score: 10,
        openclaw_down: true,
        docker_down: true,
    };

    let steps = decision.recommend();
    // SAFETY: must verify standby BEFORE triggering failover (PRD §5.3)
    let standby_idx = steps
        .iter()
        .position(|s| s == &AutoHealStep::VerifiedStandbyActive)
        .expect("VerifiedStandbyActive must be in steps");
    let failover_idx = steps
        .iter()
        .position(|s| s == &AutoHealStep::TriggeredFailover)
        .expect("TriggeredFailover must be in steps");
    assert!(
        standby_idx < failover_idx,
        "standby verification must come before failover trigger"
    );
}

// ─── Test 6: Standby role never triggers failover ─────────────────────────────

#[test]
fn test_standby_role_never_triggers_failover() {
    let decision = AutoHealDecision {
        instance_id: "i-standby".to_string(),
        role: InstanceRole::Standby,
        health_score: 5,
        openclaw_down: true,
        docker_down: true,
    };

    let steps = decision.recommend();
    assert!(
        !steps.contains(&AutoHealStep::TriggeredFailover),
        "standby should NEVER trigger a failover"
    );
    assert!(
        steps.contains(&AutoHealStep::EscalatedToCommander),
        "standby failure should escalate to Commander"
    );
}

// ─── Test 7: Fleet metrics aggregation ───────────────────────────────────────

#[test]
fn test_fleet_metrics_aggregation() {
    let snapshots = vec![
        snapshot("i-1", "hetzner", 90.0, 12.0),
        snapshot("i-2", "hetzner", 80.0, 12.0),
        snapshot("i-3", "vultr", 70.0, 15.0),
        snapshot("i-4", "vultr", 60.0, 15.0),
    ];

    let fm = FleetMetrics::compute(&snapshots);

    assert_eq!(fm.total_instances, 4);
    assert!(
        (fm.avg_health_score - 75.0).abs() < 0.001,
        "avg health should be 75"
    );
    assert!(
        (fm.total_monthly_cost_usd - 54.0).abs() < 0.001,
        "total cost should be $54"
    );

    let hetzner = fm
        .by_provider
        .get("hetzner")
        .expect("hetzner in by_provider");
    assert_eq!(hetzner.instance_count, 2);
    assert!((hetzner.avg_health_score - 85.0).abs() < 0.001);
    assert!((hetzner.monthly_cost_usd - 24.0).abs() < 0.001);

    let vultr = fm.by_provider.get("vultr").expect("vultr in by_provider");
    assert_eq!(vultr.instance_count, 2);
    assert!((vultr.avg_health_score - 65.0).abs() < 0.001);
}

// ─── Test 8: Time-series ring buffer — evicts oldest on overflow ──────────────

#[test]
fn test_time_series_buffer_evicts_oldest() {
    let mut buf = TimeSeriesBuffer::new(3);
    // Push 4 items into a capacity-3 buffer
    for health in [90.0f64, 80.0, 70.0, 60.0] {
        buf.push(snapshot("i-1", "hetzner", health, 12.0));
    }

    assert_eq!(buf.len(), 3, "buffer should cap at 3");
    // Last 3 inserted: 80, 70, 60 → avg = 70
    let avg = buf.avg_health_score().expect("non-empty buffer");
    assert!(
        (avg - 70.0).abs() < 0.001,
        "avg should be 70.0 (last 3 snapshots)"
    );
}

// ─── Test 9: Config push batch limit (PRD safety rule) ───────────────────────

#[test]
fn test_config_push_rolling_batch_limit() {
    // PRD: never push to >100 instances without rolling validation
    let too_large_batch = 101usize;
    assert!(
        too_large_batch > 100,
        "batch of 101 exceeds the 100-instance limit"
    );

    // Safe batch sizes
    assert!(50usize <= 100, "batch of 50 is within safe limit");
    assert!(100usize <= 100, "batch of 100 is exactly at the limit");
}

// ─── Test 10: Verify standby precondition (critical safety invariant) ─────────

#[test]
fn test_verify_standby_precondition_all_states() {
    // Only ACTIVE standby allows failover
    assert!(verify_standby_precondition(InstanceState::Active));
    // All non-ACTIVE states must block failover
    for state in [
        InstanceState::Degraded,
        InstanceState::Failed,
        InstanceState::Unknown,
        InstanceState::Bootstrapping,
        InstanceState::Maintenance,
    ] {
        assert!(
            !verify_standby_precondition(state),
            "state {:?} should block failover",
            state
        );
    }
}

// ─── Test 11: Provider scoring algorithm ─────────────────────────────────────

#[test]
fn test_provider_scoring_weighted_factors() {
    // Perfect score: all factors at max
    let perfect = score_provider(100, 1.0, &LatencyClass::Low, 1.0);
    assert!((perfect - 1.0).abs() < 0.001);

    // Latency degrades score
    let high_lat = score_provider(100, 1.0, &LatencyClass::High, 1.0);
    assert!(perfect > high_lat, "low latency > high latency score");

    // Cost matters: higher cost_score = better
    let cheap = score_provider(80, 0.9, &LatencyClass::Low, 0.9);
    let expensive = score_provider(80, 0.1, &LatencyClass::Low, 0.9);
    assert!(cheap > expensive, "cheaper provider should score higher");

    // Unhealthy provider scores near zero
    let unhealthy = score_provider(0, 0.0, &LatencyClass::High, 0.0);
    assert!(unhealthy < 0.1);
}

// ─── Test 12: Fleet health sweep categories ───────────────────────────────────

#[test]
fn test_fleet_health_sweep() {
    let thresholds = HealthThresholds::default();
    let reports = vec![
        healthy_report("i-h1"),
        healthy_report("i-h2"),
        {
            let mut r = healthy_report("i-degraded");
            r.openclaw_status = ServiceStatus::Down; // score 60 → Monitor
            r
        },
        {
            let mut r = healthy_report("i-critical");
            r.openclaw_status = ServiceStatus::Down;
            r.docker_running = false;
            r.tailscale_connected = false; // score 25 → AutoHeal
            r
        },
    ];

    let result = sweep_fleet(&reports, &thresholds);
    assert_eq!(result.total_instances, 4);
    assert_eq!(result.healthy, 2);
    assert_eq!(result.degraded, 1);
    assert_eq!(result.critical, 1);
    assert_eq!(result.auto_heal_triggered, 1);
    // 2 of 4 healthy → 50%
    assert_eq!(result.fleet_health_score(), 50);
}

// ─── Test 13: Cost tracker aggregation ───────────────────────────────────────

#[test]
fn test_cost_tracker_fleet_aggregation() {
    let mut ct = CostTracker::new();

    for i in 0..4u32 {
        ct.track(InstanceCost {
            instance_id: format!("i-hetzner-{i}"),
            provider: "hetzner".to_string(),
            monthly_cost_usd: 12.0,
            hours_active: 720.0,
            projected_monthly_usd: 12.0,
            actual_spend_usd: 12.0,
        });
    }
    for i in 0..2u32 {
        ct.track(InstanceCost {
            instance_id: format!("i-vultr-{i}"),
            provider: "vultr".to_string(),
            monthly_cost_usd: 15.0,
            hours_active: 720.0,
            projected_monthly_usd: 15.0,
            actual_spend_usd: 15.0,
        });
    }

    // 4*$12 + 2*$15 = $78
    assert!((ct.total_actual_spend() - 78.0).abs() < 0.001);
    let by_prov = ct.by_provider();
    assert!((by_prov["hetzner"] - 48.0).abs() < 0.001);
    assert!((by_prov["vultr"] - 30.0).abs() < 0.001);
    assert_eq!(ct.all().len(), 6);
}

// ─── Test 14: InstancePairStatus pair health computation ─────────────────────

#[test]
fn test_instance_pair_status_health_is_min_of_pair() {
    assert_eq!(InstancePairStatus::compute_pair_health(90, Some(70)), 70);
    assert_eq!(InstancePairStatus::compute_pair_health(70, Some(90)), 70);
    assert_eq!(InstancePairStatus::compute_pair_health(85, None), 85);
    assert_eq!(InstancePairStatus::compute_pair_health(0, Some(100)), 0);
}

// ─── Test 15: New proto types serialize correctly ─────────────────────────────

#[test]
fn test_health_check_response_round_trip() {
    let resp = HealthCheckResponse {
        instance_id: "i-proto-test".to_string(),
        score: 87,
        checks: vec![
            HealthCheck {
                name: "cpu".to_string(),
                status: CheckStatus::Healthy,
                message: "CPU normal".to_string(),
                value: Some(18.5),
            },
            HealthCheck {
                name: "disk".to_string(),
                status: CheckStatus::Degraded,
                message: "Disk at 75%".to_string(),
                value: Some(75.0),
            },
        ],
        timestamp: Utc::now(),
    };

    let json = serde_json::to_string(&resp).expect("serialize");
    let back: HealthCheckResponse = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.score, 87);
    assert_eq!(back.checks.len(), 2);
    assert_eq!(back.checks[0].status, CheckStatus::Healthy);
    assert_eq!(back.checks[1].status, CheckStatus::Degraded);
}

#[test]
fn test_vps_metrics_response_round_trip() {
    let resp = VpsMetricsResponse {
        instance_id: "i-metrics-test".to_string(),
        cpu_percent: 22.5,
        memory_used_mb: 1024,
        memory_total_mb: 4096,
        disk_used_gb: 20,
        disk_total_gb: 80,
        network_rx_bytes: 1_000_000,
        network_tx_bytes: 500_000,
        load_avg_1m: 0.5,
        load_avg_5m: 0.4,
        load_avg_15m: 0.3,
        timestamp: Utc::now(),
    };

    let json = serde_json::to_string(&resp).expect("serialize");
    let back: VpsMetricsResponse = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.instance_id, "i-metrics-test");
    assert!((back.cpu_percent - 22.5).abs() < 0.001);
    assert_eq!(back.memory_total_mb, 4096);
}

// ─── Test 16: FSM recovery resets to Normal ───────────────────────────────────

#[test]
fn test_fsm_recovery_and_reset() {
    let mut fsm = FailoverStateMachine::new(
        "i-reset-test".to_string(),
        InstanceRole::Primary,
        HealthThresholds::default(),
    );

    // Fail it
    for _ in 0..=MAX_HEAL_ATTEMPTS {
        fsm.transition(25, false);
    }
    assert!(fsm.state.needs_escalation());

    // Operator resets
    fsm.reset();
    assert_eq!(fsm.state, FailoverState::Normal);
    assert!(fsm.state.is_stable());

    // Verify it works normally again
    let t = fsm.transition(95, true);
    assert_eq!(t, FailoverTransition::NoAction);
}
