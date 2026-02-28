//! gf-node-proto — Protobuf definitions for the GatewayForge VPS node protocol
//!
//! Defines all message types exchanged between gf-clawnode (on each VPS) and
//! the ClawOps OpenClaw gateway. These types mirror the .proto definitions;
//! until the build.rs pipeline is wired up, they are maintained here as
//! idiomatic Rust structs with serde support.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Envelope ─────────────────────────────────────────────────────────────────

/// Wire envelope for all messages between gateway and clawnode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMessage {
    pub message_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub instance_id: String,
    #[serde(flatten)]
    pub payload: NodePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodePayload {
    Command(CommandRequest),
    CommandResult(CommandResult),
    Heartbeat(HeartbeatPayload),
    HealthReport(HealthReport),
    MetricsReport(MetricsReport),
    EventNotification(EventNotification),
}

// ─── Commands ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRequest {
    pub request_id: Uuid,
    pub command: String,
    #[serde(default)]
    pub args: serde_json::Value,
    /// HMAC-SHA256 signature of (request_id + command + args)
    pub signature: String,
    /// Agent identity that issued this command (e.g. "guardian", "forge")
    pub issued_by: AgentIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub request_id: Uuid,
    pub command: String,
    pub success: bool,
    pub output: serde_json::Value,
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentIdentity {
    Commander,
    Guardian,
    Forge,
    Ledger,
    Triage,
    Briefer,
    System,
}

// ─── Heartbeat ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPayload {
    pub health_score: u8, // 0–100
    pub openclaw_status: ServiceStatus,
    pub docker_running: bool,
    pub tailscale_connected: bool,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServiceStatus {
    Healthy,
    Degraded,
    Down,
    Unknown,
}

// ─── Health Report ────────────────────────────────────────────────────────────

/// Full health report emitted on demand (vps.health command) or periodically
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub instance_id: String,
    pub health_score: u8,
    pub openclaw_status: ServiceStatus,
    pub docker_status: DockerStatus,
    pub disk_usage_pct: f32,
    pub cpu_usage_1m: f32,
    pub mem_usage_pct: f32,
    pub swap_usage_pct: f32,
    pub load_avg_1m: f32,
    pub load_avg_5m: f32,
    pub load_avg_15m: f32,
    pub uptime_seconds: u64,
    pub tailscale_latency_ms: Option<u32>,
    pub last_heartbeat: DateTime<Utc>,
    pub openclaw_http_status: Option<u16>,
    pub containers: Vec<ContainerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerStatus {
    pub running: bool,
    pub container_count: u32,
    pub unhealthy_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStatus {
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    pub uptime_seconds: u64,
    pub restart_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerState {
    Running,
    Exited,
    Restarting,
    Paused,
    Dead,
    Created,
}

// ─── Metrics Report ──────────────────────────────────────────────────────────

/// Detailed metrics snapshot (emitted every 60s by metrics_collector)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsReport {
    pub instance_id: String,
    pub collected_at: DateTime<Utc>,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disk: Vec<DiskMetrics>,
    pub network: NetworkMetrics,
    pub openclaw: OpenClawMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuMetrics {
    pub usage_pct: f32,
    pub steal_pct: f32, // Relevant for VPS — hypervisor steal time
    pub core_count: u32,
    pub load_avg_1m: f32,
    pub load_avg_5m: f32,
    pub load_avg_15m: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_mb: u64,
    pub used_mb: u64,
    pub free_mb: u64,
    pub cached_mb: u64,
    pub swap_total_mb: u64,
    pub swap_used_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMetrics {
    pub mount_point: String,
    pub total_gb: f32,
    pub used_gb: f32,
    pub free_gb: f32,
    pub usage_pct: f32,
    pub iops_read: u64,
    pub iops_write: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetrics {
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub packets_sent: u64,
    pub packets_recv: u64,
    pub errors_in: u64,
    pub errors_out: u64,
    pub interface: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawMetrics {
    pub http_requests_total: u64,
    pub http_latency_p50_ms: f32,
    pub http_latency_p95_ms: f32,
    pub http_latency_p99_ms: f32,
    pub active_sessions: u32,
    pub websocket_connections: u32,
    pub error_rate_pct: f32,
}

// ─── Events ───────────────────────────────────────────────────────────────────

/// Structured events emitted by clawnode to notify the gateway of notable state changes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventNotification {
    pub event_type: NodeEventType,
    pub severity: EventSeverity,
    pub message: String,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeEventType {
    OpenClawDown,
    OpenClawRecovered,
    DockerRestarted,
    DockerDown,
    DiskUsageHigh,
    CpuUsageHigh,
    MemUsageHigh,
    TailscaleDisconnected,
    TailscaleReconnected,
    ConfigUpdated,
    ConfigRollback,
    BootstrapComplete,
    HeartbeatMissed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum EventSeverity {
    Info,
    Warning,
    Critical,
}

// ─── Instance metadata ────────────────────────────────────────────────────────

/// Static instance metadata registered at bootstrap time
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
    pub ip_public: String,
    pub ip_tailscale: String,
    pub openclaw_version: String,
    pub clawnode_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VpsProvider {
    Hetzner,
    Vultr,
    Contabo,
    Hostinger,
    DigitalOcean,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstanceTier {
    Nano,
    Standard,
    Pro,
    Enterprise,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstanceRole {
    Primary,
    Standby,
}

// ─── Provision types ─────────────────────────────────────────────────────────

/// Sent by Forge when requesting a new instance provision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionRequest {
    pub request_id: Uuid,
    pub account_id: String,
    pub tier: InstanceTier,
    pub role: InstanceRole,
    pub provider: VpsProvider,
    pub region: String,
    pub pair_instance_id: Option<String>, // Set for standby provisions
    pub openclaw_config: serde_json::Value,
    pub requested_by: AgentIdentity,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionResult {
    pub request_id: Uuid,
    pub instance_id: String,
    pub success: bool,
    pub error: Option<String>,
    pub provision_duration_ms: u64,
    pub instance_ip: Option<String>,
    pub tailscale_ip: Option<String>,
    pub provider_instance_id: Option<String>,
}
