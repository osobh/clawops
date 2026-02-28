//! ClawOps performance benchmarks using Criterion.
//!
//! Run with: `cargo bench -p claw-bench`

use chrono::Utc;
use claw_health::{HealthThresholds, compute_health_score, evaluate_alerts};
use claw_metrics::{FleetMetrics, InstanceSnapshot};
use claw_proto::{HealthReport, InstanceRole, InstanceState, InstanceTier, ServiceStatus, VpsProvider};
use claw_provision::{LatencyClass, RetryPolicy, score_provider};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn make_report(id: &str, cpu: f32, mem: f32) -> HealthReport {
    HealthReport {
        instance_id: id.to_string(),
        account_id: "bench-acct".to_string(),
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
        cpu_usage_1m: cpu,
        mem_usage_pct: mem,
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

fn make_snapshot(id: &str, cost: f64) -> InstanceSnapshot {
    InstanceSnapshot {
        instance_id: id.to_string(),
        provider: "hetzner".to_string(),
        cpu_pct: 25.0,
        mem_pct: 40.0,
        disk_pct: 30.0,
        health_score: 95.0,
        monthly_cost_usd: cost,
        recorded_at: Utc::now(),
    }
}

// ─── bench_health_score_computation ──────────────────────────────────────────

/// Measure health scoring speed.
///
/// Guardian calls this every 5 minutes per instance. With 1000 instances the
/// total budget is still well under 1ms at realistic iteration counts.
fn bench_health_score_computation(c: &mut Criterion) {
    let report = make_report("i-bench-1", 45.0, 60.0);
    let thresholds = HealthThresholds::default();

    c.bench_function("health_score_computation", |b| {
        b.iter(|| {
            let score = compute_health_score(black_box(&report));
            let alerts = evaluate_alerts(black_box(&report), black_box(&thresholds));
            black_box((score, alerts))
        });
    });
}

// ─── bench_fleet_metrics_aggregation ─────────────────────────────────────────

/// Aggregate FleetMetrics across 1000 instances.
///
/// Called by Ledger and Commander on cost report requests.
fn bench_fleet_metrics_aggregation(c: &mut Criterion) {
    let instances: Vec<InstanceSnapshot> = (0..1000)
        .map(|i| make_snapshot(&format!("i-{i}"), 12.0 + (i as f64 * 0.01)))
        .collect();

    c.bench_function("fleet_metrics_aggregation_1000", |b| {
        b.iter(|| {
            let metrics = FleetMetrics::compute(black_box(&instances));
            black_box(metrics.total_monthly_cost_usd)
        });
    });
}

// ─── bench_provider_selection ─────────────────────────────────────────────────

/// Score and select the best provider from 5 candidates.
///
/// Called by ProviderRegistry::select_provider() on each provision request.
fn bench_provider_selection(c: &mut Criterion) {
    let providers = [
        (100u8, 1.0f32, LatencyClass::Low, 0.99f32),
        (85, 0.95, LatencyClass::Medium, 0.97),
        (70, 0.80, LatencyClass::High, 0.95),
        (90, 1.0, LatencyClass::Low, 0.98),
        (60, 0.70, LatencyClass::Medium, 0.92),
    ];

    c.bench_function("provider_selection_5", |b| {
        b.iter(|| {
            let best = providers
                .iter()
                .enumerate()
                .map(|(i, (health, cost_factor, latency, reliability))| {
                    (
                        i,
                        score_provider(*health, *cost_factor, latency, *reliability),
                    )
                })
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            black_box(best)
        });
    });
}

// ─── bench_retry_policy_delay ─────────────────────────────────────────────────

/// Measure RetryPolicy delay calculation.
///
/// Called before each retry sleep — must be near-zero cost.
fn bench_retry_policy_delay(c: &mut Criterion) {
    let policy = RetryPolicy::default();

    c.bench_function("retry_policy_delay", |b| {
        let mut n: u32 = 0;
        b.iter(|| {
            let delay = policy.delay_for_attempt(black_box(n % 4));
            n = n.wrapping_add(1);
            black_box(delay)
        });
    });
}

// ─── bench_audit_chain_hash ───────────────────────────────────────────────────

/// Hash 1000 audit entries using SHA-256 chaining.
///
/// Called on each append to claw-audit's AuditLogger.
fn bench_audit_chain_hash(c: &mut Criterion) {
    use sha2::{Digest, Sha256};

    c.bench_function("audit_chain_hash_1000", |b| {
        b.iter(|| {
            let mut prev_hash = "genesis".to_string();
            for i in 0u32..1000 {
                let data = format!("{}:{}:provision:i-{i}:success:1735000000", prev_hash, i);
                prev_hash = format!("{:x}", Sha256::digest(data.as_bytes()));
            }
            black_box(prev_hash)
        });
    });
}

// ─── Criterion groups ─────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_health_score_computation,
    bench_fleet_metrics_aggregation,
    bench_provider_selection,
    bench_retry_policy_delay,
    bench_audit_chain_hash,
);
criterion_main!(benches);
