//! Health check command handlers
//!
//! Implements: health.check, health.score, node.health, node.capabilities

use crate::SharedState;
use crate::commands::CommandError;
use chrono::Utc;
use claw_health::{HealthThresholds, compute_health_score, evaluate_alerts, recommend_action};
use claw_proto::{HealthReport, InstanceRole, InstanceState, ServiceStatus};
use serde_json::{Value, json};
use sysinfo::System;

// ─── Build a HealthReport from live system state ──────────────────────────────

async fn gather_health_report(state: &SharedState) -> HealthReport {
    let s = state.read().await;

    let mut sys = System::new_all();
    sys.refresh_all();

    let cpu_usage = sys.global_cpu_usage();
    let mem_total = sys.total_memory();
    let mem_used = sys.used_memory();
    let mem_pct = if mem_total > 0 {
        (mem_used as f64 / mem_total as f64) * 100.0
    } else {
        0.0
    };

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk_pct = disks
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .map(|d| {
            let used = d.total_space() - d.available_space();
            if d.total_space() > 0 {
                (used as f64 / d.total_space() as f64) * 100.0
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);

    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();
    let swap_pct = if swap_total > 0 {
        (swap_used as f64 / swap_total as f64) * 100.0
    } else {
        0.0
    };

    let load = System::load_average();

    let networks = sysinfo::Networks::new_with_refreshed_list();
    let (bytes_sent, bytes_recv): (u64, u64) = networks.iter().fold((0, 0), |(s, r), (_, n)| {
        (s + n.transmitted(), r + n.received())
    });

    let docker_running = std::process::Command::new("docker")
        .args(["info"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let tailscale_connected = std::process::Command::new("tailscale")
        .args(["status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let openclaw_status = if docker_running {
        ServiceStatus::Healthy
    } else {
        ServiceStatus::Degraded
    };

    let tier = match s.config.tier.as_str() {
        "nano" => claw_proto::InstanceTier::Nano,
        "pro" => claw_proto::InstanceTier::Pro,
        "enterprise" => claw_proto::InstanceTier::Enterprise,
        _ => claw_proto::InstanceTier::Standard,
    };

    let role = match s.config.role.as_str() {
        "standby" => InstanceRole::Standby,
        _ => InstanceRole::Primary,
    };

    let provider = match s.config.provider.as_str() {
        "vultr" => claw_proto::VpsProvider::Vultr,
        "contabo" => claw_proto::VpsProvider::Contabo,
        "hostinger" => claw_proto::VpsProvider::Hostinger,
        "digitalocean" => claw_proto::VpsProvider::DigitalOcean,
        _ => claw_proto::VpsProvider::Hetzner,
    };

    HealthReport {
        instance_id: s.config.hostname.clone(),
        account_id: s.config.account_id.clone(),
        provider,
        region: s.config.region.clone(),
        tier,
        role,
        state: InstanceState::Active,
        health_score: 0,
        openclaw_status,
        openclaw_http_status: None,
        docker_running,
        tailscale_connected,
        tailscale_latency_ms: None,
        cpu_usage_1m: cpu_usage,
        mem_usage_pct: mem_pct as f32,
        disk_usage_pct: disk_pct as f32,
        swap_usage_pct: swap_pct as f32,
        load_avg_1m: load.one as f32,
        load_avg_5m: load.five as f32,
        load_avg_15m: load.fifteen as f32,
        uptime_secs: System::uptime(),
        bytes_sent_per_sec: bytes_sent as f64,
        bytes_recv_per_sec: bytes_recv as f64,
        reported_at: Utc::now(),
    }
}

// ─── health.check ────────────────────────────────────────────────────────────

pub async fn handle_health_check(state: &SharedState) -> Result<Value, CommandError> {
    let mut report = gather_health_report(state).await;
    let thresholds = HealthThresholds::default();

    let score = compute_health_score(&report);
    report.health_score = score;

    let alerts = evaluate_alerts(&report, &thresholds);
    let action = recommend_action(score, &thresholds);

    let instance_state = if score >= thresholds.degraded_score {
        InstanceState::Active
    } else if score >= thresholds.critical_score {
        InstanceState::Degraded
    } else {
        InstanceState::Failed
    };

    Ok(json!({
        "ok": true,
        "instance_id": report.instance_id,
        "health_score": score,
        "state": format!("{:?}", instance_state),
        "cpu_usage_pct": report.cpu_usage_1m,
        "mem_usage_pct": report.mem_usage_pct,
        "disk_usage_pct": report.disk_usage_pct,
        "docker_running": report.docker_running,
        "tailscale_connected": report.tailscale_connected,
        "uptime_secs": report.uptime_secs,
        "recommended_action": format!("{:?}", action),
        "alerts": alerts.iter().map(|a| json!({
            "type": format!("{:?}", a.alert_type),
            "severity": format!("{:?}", a.severity),
            "message": a.message,
        })).collect::<Vec<_>>(),
    }))
}

// ─── health.score ────────────────────────────────────────────────────────────

pub async fn handle_health_score(state: &SharedState) -> Result<Value, CommandError> {
    let report = gather_health_report(state).await;
    let score = compute_health_score(&report);

    Ok(json!({
        "ok": true,
        "health_score": score,
    }))
}

// ─── node.health ─────────────────────────────────────────────────────────────

pub async fn handle_node_health(state: &SharedState) -> Result<Value, CommandError> {
    handle_health_check(state).await
}

// ─── node.capabilities ───────────────────────────────────────────────────────

pub async fn handle_node_capabilities(state: &SharedState) -> Result<Value, CommandError> {
    Ok(json!({
        "ok": true,
        "capabilities": state.capabilities,
        "commands": state.commands,
        "agent_version": env!("CARGO_PKG_VERSION"),
    }))
}
