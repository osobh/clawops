//! Protocol types for ClawOps VPS node protocol.
//!
//! Defines the message types exchanged between the OpenClaw gateway
//! and clawnode agents running on VPS instances.

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── VPS Provider & Tier ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VpsProvider {
    Hetzner,
    Vultr,
    Contabo,
    Hostinger,
    DigitalOcean,
}

impl std::fmt::Display for VpsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hetzner => write!(f, "hetzner"),
            Self::Vultr => write!(f, "vultr"),
            Self::Contabo => write!(f, "contabo"),
            Self::Hostinger => write!(f, "hostinger"),
            Self::DigitalOcean => write!(f, "digitalocean"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceTier {
    Nano,
    Standard,
    Pro,
    Enterprise,
}

impl std::fmt::Display for InstanceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nano => write!(f, "nano"),
            Self::Standard => write!(f, "standard"),
            Self::Pro => write!(f, "pro"),
            Self::Enterprise => write!(f, "enterprise"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceRole {
    Primary,
    Standby,
}

impl std::fmt::Display for InstanceRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => write!(f, "primary"),
            Self::Standby => write!(f, "standby"),
        }
    }
}

// ─── Instance State ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum InstanceState {
    #[default]
    Unknown,
    Bootstrapping,
    Active,
    Degraded,
    Failed,
    Maintenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    Healthy,
    Degraded,
    Down,
    Unknown,
}

// ─── Instance Metadata ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceMetadata {
    pub instance_id: String,
    pub account_id: String,
    pub provider: VpsProvider,
    pub region: String,
    pub tier: InstanceTier,
    pub role: InstanceRole,
    pub pair_instance_id: Option<String>,
    pub provisioned_at: DateTime<Utc>,
    pub ip_public: Option<String>,
    pub ip_tailscale: Option<String>,
    pub openclaw_version: Option<String>,
    pub clawnode_version: String,
}

// ─── Health & Metrics ─────────────────────────────────────────────────────────

/// Health report sent periodically from clawnode to gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub instance_id: String,
    pub account_id: String,
    pub provider: VpsProvider,
    pub region: String,
    pub tier: InstanceTier,
    pub role: InstanceRole,
    pub state: InstanceState,
    pub health_score: u8, // 0-100

    // Service statuses
    pub openclaw_status: ServiceStatus,
    pub openclaw_http_status: Option<u16>,
    pub docker_running: bool,
    pub tailscale_connected: bool,
    pub tailscale_latency_ms: Option<f32>,

    // Resource usage
    pub cpu_usage_1m: f32,
    pub mem_usage_pct: f32,
    pub disk_usage_pct: f32,
    pub swap_usage_pct: f32,
    pub load_avg_1m: f32,
    pub load_avg_5m: f32,
    pub load_avg_15m: f32,
    pub uptime_secs: u64,

    // Network
    pub bytes_sent_per_sec: f64,
    pub bytes_recv_per_sec: f64,

    pub reported_at: DateTime<Utc>,
}

/// CPU/memory/disk metrics from a VPS instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsReport {
    pub instance_id: String,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disk: Vec<DiskMetrics>,
    pub network: NetworkMetrics,
    pub openclaw: OpenClawMetrics,
    pub reported_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuMetrics {
    pub usage_pct: f32,
    pub load_1m: f32,
    pub load_5m: f32,
    pub load_15m: f32,
    pub core_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_mb: u64,
    pub used_mb: u64,
    pub available_mb: u64,
    pub swap_total_mb: u64,
    pub swap_used_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMetrics {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetrics {
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub bytes_sent_per_sec: f64,
    pub bytes_recv_per_sec: f64,
    pub tailscale_latency_ms: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawMetrics {
    pub http_status: Option<u16>,
    pub response_time_ms: Option<u32>,
    pub active_connections: Option<u32>,
    pub uptime_secs: Option<u64>,
}

// ─── Heartbeat ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPayload {
    pub instance_id: String,
    pub health_score: u8,
    pub openclaw_status: ServiceStatus,
    pub docker_running: bool,
    pub tailscale_connected: bool,
    pub uptime_secs: u64,
}

// ─── Event types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeEventType {
    OpenClawDown,
    OpenClawRecovered,
    DockerDown,
    DockerRecovered,
    TailscaleDisconnected,
    TailscaleReconnected,
    DiskUsageHigh,
    CpuUsageHigh,
    MemUsageHigh,
    HealthScoreLow,
    HealthScoreRecovered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventNotification {
    pub event_type: NodeEventType,
    pub severity: EventSeverity,
    pub instance_id: String,
    pub description: String,
    pub details: Option<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

// ─── Provisioning types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionRequest {
    pub request_id: Uuid,
    pub account_id: String,
    pub tier: InstanceTier,
    pub role: InstanceRole,
    pub provider: VpsProvider,
    pub region: String,
    pub pair_instance_id: Option<String>,
    pub openclaw_config: Option<serde_json::Value>,
    pub requested_by: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionResult {
    pub request_id: Uuid,
    pub instance_id: Option<String>,
    pub success: bool,
    pub error: Option<String>,
    pub provision_duration_ms: u64,
    pub instance_ip: Option<String>,
    pub tailscale_ip: Option<String>,
    pub provider_instance_id: Option<String>,
}

// ─── Fleet Status ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetStatus {
    pub total_instances: u32,
    pub active_pairs: u32,
    pub degraded_instances: u32,
    pub failed_instances: u32,
    pub bootstrapping_instances: u32,
    pub generated_at: DateTime<Utc>,
}

// ─── Command Protocol ─────────────────────────────────────────────────────────

/// A command request from the gateway to a clawnode instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRequest {
    pub request_id: String,
    pub command: String,
    pub args: serde_json::Value,
    pub issued_by: String,
}

/// Result of a command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub request_id: String,
    pub command: String,
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

// ─── Validation ───────────────────────────────────────────────────────────────

/// Validate an instance ID format.
pub fn validate_instance_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Validate an account ID format.
pub fn validate_account_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_instance_id() {
        assert!(validate_instance_id("i-abc123"));
        assert!(validate_instance_id("gf-account1-12345678"));
        assert!(!validate_instance_id(""));
        assert!(!validate_instance_id("invalid id with spaces"));
    }

    #[test]
    fn test_instance_tier_display() {
        assert_eq!(InstanceTier::Nano.to_string(), "nano");
        assert_eq!(InstanceTier::Enterprise.to_string(), "enterprise");
    }

    #[test]
    fn test_vps_provider_display() {
        assert_eq!(VpsProvider::Hetzner.to_string(), "hetzner");
        assert_eq!(VpsProvider::DigitalOcean.to_string(), "digitalocean");
    }

    #[test]
    fn test_health_report_serialization() {
        let report = HealthReport {
            instance_id: "i-test".to_string(),
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
            cpu_usage_1m: 15.0,
            mem_usage_pct: 42.0,
            disk_usage_pct: 30.0,
            swap_usage_pct: 0.0,
            load_avg_1m: 0.5,
            load_avg_5m: 0.4,
            load_avg_15m: 0.3,
            uptime_secs: 86400,
            bytes_sent_per_sec: 1024.0,
            bytes_recv_per_sec: 2048.0,
            reported_at: Utc::now(),
        };

        let json = serde_json::to_string(&report).expect("serialize");
        let back: HealthReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.instance_id, "i-test");
        assert_eq!(back.health_score, 95);
    }
}
