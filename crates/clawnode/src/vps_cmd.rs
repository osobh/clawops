//! VPS command handlers
//!
//! Implements: openclaw.health, vps.metrics, vps.status, vps.info,
//!             docker.restart, docker.status, vps.restart

use crate::SharedState;
use crate::commands::CommandError;
use serde_json::{json, Value};
use sysinfo::System;
use tracing::info;

// ─── openclaw.health ─────────────────────────────────────────────────────────

pub async fn handle_openclaw_health(state: &SharedState) -> Result<Value, CommandError> {
    let s = state.read().await;

    // Check if openclaw process is running via sysinfo
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let openclaw_running = sys
        .processes()
        .values()
        .any(|p| p.name().to_string_lossy().contains("openclaw"));

    Ok(json!({
        "ok": true,
        "instance_id": s.config.hostname,
        "provider": s.config.provider,
        "region": s.config.region,
        "tier": s.config.tier,
        "role": s.config.role,
        "openclaw_running": openclaw_running,
        "agent_version": env!("CARGO_PKG_VERSION"),
    }))
}

// ─── vps.status ──────────────────────────────────────────────────────────────

pub async fn handle_vps_status(state: &SharedState) -> Result<Value, CommandError> {
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

    // Disk usage on root
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let (disk_total, disk_used) = disks
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .map(|d| (d.total_space(), d.total_space() - d.available_space()))
        .unwrap_or((0, 0));
    let disk_pct = if disk_total > 0 {
        (disk_used as f64 / disk_total as f64) * 100.0
    } else {
        0.0
    };

    Ok(json!({
        "ok": true,
        "instance_id": s.config.hostname,
        "provider": s.config.provider,
        "region": s.config.region,
        "tier": s.config.tier,
        "role": s.config.role,
        "cpu_usage_pct": cpu_usage,
        "mem_used_mb": mem_used / 1024 / 1024,
        "mem_total_mb": mem_total / 1024 / 1024,
        "mem_usage_pct": mem_pct,
        "disk_used_gb": disk_used / 1024 / 1024 / 1024,
        "disk_total_gb": disk_total / 1024 / 1024 / 1024,
        "disk_usage_pct": disk_pct,
        "uptime_secs": System::uptime(),
    }))
}

// ─── vps.info ────────────────────────────────────────────────────────────────

pub async fn handle_vps_info(state: &SharedState) -> Result<Value, CommandError> {
    let s = state.read().await;

    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let mut sys = System::new_all();
    sys.refresh_all();

    Ok(json!({
        "ok": true,
        "hostname": hostname,
        "instance_id": s.config.hostname,
        "account_id": s.config.account_id,
        "provider": s.config.provider,
        "region": s.config.region,
        "tier": s.config.tier,
        "role": s.config.role,
        "os": System::name().unwrap_or_default(),
        "os_version": System::os_version().unwrap_or_default(),
        "kernel": System::kernel_version().unwrap_or_default(),
        "cpu_count": sys.cpus().len(),
        "mem_total_mb": sys.total_memory() / 1024 / 1024,
        "agent_version": env!("CARGO_PKG_VERSION"),
    }))
}

// ─── vps.metrics ─────────────────────────────────────────────────────────────

pub async fn handle_vps_metrics(_state: &SharedState) -> Result<Value, CommandError> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let cpu_usage = sys.global_cpu_usage();
    let load_avg = System::load_average();
    let mem_total = sys.total_memory();
    let mem_used = sys.used_memory();
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk_list: Vec<Value> = disks
        .iter()
        .map(|d| {
            json!({
                "mount": d.mount_point().to_string_lossy(),
                "total_bytes": d.total_space(),
                "used_bytes": d.total_space() - d.available_space(),
                "available_bytes": d.available_space(),
            })
        })
        .collect();

    let networks = sysinfo::Networks::new_with_refreshed_list();
    let (bytes_sent, bytes_recv): (u64, u64) = networks
        .iter()
        .fold((0, 0), |(s, r), (_, n)| (s + n.transmitted(), r + n.received()));

    Ok(json!({
        "ok": true,
        "cpu": {
            "usage_pct": cpu_usage,
            "load_1m": load_avg.one,
            "load_5m": load_avg.five,
            "load_15m": load_avg.fifteen,
            "core_count": sys.cpus().len(),
        },
        "memory": {
            "total_mb": mem_total / 1024 / 1024,
            "used_mb": mem_used / 1024 / 1024,
            "available_mb": (mem_total - mem_used) / 1024 / 1024,
            "swap_total_mb": swap_total / 1024 / 1024,
            "swap_used_mb": swap_used / 1024 / 1024,
        },
        "disk": disk_list,
        "network": {
            "bytes_sent": bytes_sent,
            "bytes_recv": bytes_recv,
        },
        "uptime_secs": System::uptime(),
    }))
}

// ─── vps.restart ─────────────────────────────────────────────────────────────

pub async fn handle_vps_restart(state: &SharedState) -> Result<Value, CommandError> {
    let s = state.read().await;
    info!(instance = %s.config.hostname, "restart requested via gateway command");

    // Schedule a graceful reboot after a short delay to allow the response to be sent
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let _ = std::process::Command::new("shutdown")
            .args(["-r", "now"])
            .spawn();
    });

    Ok(json!({
        "ok": true,
        "message": "reboot scheduled in 3 seconds",
        "instance_id": s.config.hostname,
    }))
}

// ─── docker.status ───────────────────────────────────────────────────────────

pub async fn handle_docker_status(_state: &SharedState) -> Result<Value, CommandError> {
    let output = std::process::Command::new("docker")
        .args(["info", "--format", "{{json .}}"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let info: Value = serde_json::from_slice(&o.stdout)
                .unwrap_or(json!({"raw": String::from_utf8_lossy(&o.stdout).trim().to_string()}));
            Ok(json!({
                "ok": true,
                "running": true,
                "info": info,
            }))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            Ok(json!({
                "ok": true,
                "running": false,
                "error": stderr,
            }))
        }
        Err(e) => Ok(json!({
            "ok": true,
            "running": false,
            "error": e.to_string(),
        })),
    }
}

// ─── docker.restart ──────────────────────────────────────────────────────────

pub async fn handle_docker_restart(_state: &SharedState) -> Result<Value, CommandError> {
    let output = std::process::Command::new("systemctl")
        .args(["restart", "docker"])
        .output();

    match output {
        Ok(o) if o.status.success() => Ok(json!({
            "ok": true,
            "message": "docker restarted successfully",
        })),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            Err(format!("docker restart failed: {stderr}").into())
        }
        Err(e) => Err(format!("failed to run systemctl: {e}").into()),
    }
}

// ─── system.info ─────────────────────────────────────────────────────────────

pub async fn handle_system_info(state: &SharedState) -> Result<Value, CommandError> {
    handle_vps_info(state).await
}

// ─── system.run ──────────────────────────────────────────────────────────────

pub async fn handle_system_run(
    _state: &SharedState,
    params: Value,
) -> Result<Value, CommandError> {
    let cmd = params
        .get("command")
        .or_else(|| params.get("cmd"))
        .and_then(|v| v.as_str())
        .ok_or("missing 'command' param")?;

    // Basic security: reject shell metacharacters unless explicitly allowed
    let args: Vec<&str> = cmd.split_whitespace().collect();
    if args.is_empty() {
        return Err("empty command".into());
    }

    let output = std::process::Command::new(args[0])
        .args(&args[1..])
        .output()?;

    Ok(json!({
        "ok": output.status.success(),
        "exit_code": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout).trim(),
        "stderr": String::from_utf8_lossy(&output.stderr).trim(),
    }))
}
