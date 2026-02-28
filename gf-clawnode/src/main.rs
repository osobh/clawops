//! gf-clawnode — GatewayForge instance-side agent
//!
//! Runs on each VPS instance in the fleet. Connects to the ClawOps OpenClaw
//! Gateway over Tailscale WebSocket and handles commands dispatched by the
//! agent team (Guardian, Forge, Commander).
//!
//! This is a fork of Clawbernetes' clawnode, adapted for VPS/OpenClaw
//! management rather than GPU orchestration.

use anyhow::{Context, Result};
use clap::Parser;
use std::sync::Arc;
use tracing::{error, info, warn};

mod agent;
mod commands;
mod config;
mod heartbeat;
mod metrics_collector;
mod websocket;

use config::NodeConfig;

#[derive(Parser, Debug)]
#[command(name = "gf-clawnode", about = "GatewayForge VPS instance agent")]
struct Cli {
    /// Override config file path
    #[arg(long, env = "GF_CONFIG_FILE", default_value = "/etc/gf-clawnode/config.toml")]
    config: String,

    /// Enable verbose debug logging
    #[arg(long, short = 'v')]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(format!("gf_clawnode={log_level}").parse()?)
                .add_directive(format!("gf_health={log_level}").parse()?)
                .add_directive(format!("gf_metrics={log_level}").parse()?)
                .add_directive(format!("gf_audit={log_level}").parse()?),
        )
        .json()
        .init();

    dotenvy::dotenv().ok();

    let config = NodeConfig::load(&cli.config)
        .with_context(|| format!("Failed to load config from {}", cli.config))?;

    info!(
        instance_id = %config.instance_id,
        region = %config.region,
        tier = %config.tier,
        gateway_url = %config.gateway_url,
        "gf-clawnode starting"
    );

    let config = Arc::new(config);

    // Build the node agent — establishes WebSocket to OpenClaw gateway
    let node = agent::ClawNode::new(Arc::clone(&config))
        .await
        .context("Failed to initialize ClawNode")?;
    let node = Arc::new(node);

    // Register all command handlers
    commands::register_all(Arc::clone(&node)).await;
    info!("Command handlers registered");

    // Heartbeat loop: POST /v1/heartbeat every 30s
    let heartbeat_node = Arc::clone(&node);
    let heartbeat_handle = tokio::spawn(async move {
        heartbeat::run(heartbeat_node).await
    });

    // Metrics collector: gather sysinfo + docker stats every 60s
    let metrics_node = Arc::clone(&node);
    let metrics_handle = tokio::spawn(async move {
        metrics_collector::run(metrics_node).await
    });

    info!("gf-clawnode ready — waiting for commands");

    // Main WebSocket event loop
    tokio::select! {
        result = node.run() => {
            match result {
                Ok(()) => info!("Node event loop exited cleanly"),
                Err(e) => error!("Node event loop failed: {e:#}"),
            }
        }
        result = heartbeat_handle => {
            warn!("Heartbeat task exited: {result:?}");
        }
        result = metrics_handle => {
            warn!("Metrics task exited: {result:?}");
        }
    }

    Ok(())
}

// ─── Agent module ─────────────────────────────────────────────────────────────

mod agent {
    use super::config::NodeConfig;
    use anyhow::Result;
    use std::sync::Arc;
    use tracing::{debug, info};

    /// The core agent struct. Holds the WebSocket connection to the OpenClaw
    /// gateway and a registry of command handlers.
    pub struct ClawNode {
        pub config: Arc<NodeConfig>,
        // ws: Arc<tokio::sync::Mutex<WebSocket>>,
        // handlers: Arc<HandlerRegistry>,
    }

    impl ClawNode {
        pub async fn new(config: Arc<NodeConfig>) -> Result<Self> {
            // TODO: establish WebSocket connection to config.gateway_url
            // TODO: authenticate with config.api_key via HMAC handshake
            info!(gateway = %config.gateway_url, "Connecting to OpenClaw gateway");
            Ok(Self { config })
        }

        pub async fn run(&self) -> Result<()> {
            // TODO: main WebSocket read loop — receive commands, dispatch to handlers
            debug!("Event loop running");
            tokio::time::sleep(tokio::time::Duration::from_secs(u64::MAX)).await;
            Ok(())
        }
    }
}

// ─── Commands module ──────────────────────────────────────────────────────────

mod commands {
    use super::agent::ClawNode;
    use std::sync::Arc;
    use tracing::info;

    /// Registers all VPS/OpenClaw command handlers on the node agent.
    ///
    /// GatewayForge command surface (replaces GPU commands from Clawbernetes):
    ///
    /// System commands:
    ///   vps.health       — Return health score, resource utilization
    ///   vps.metrics      — CPU, RAM, disk, network I/O metrics
    ///   vps.reboot       — Graceful OS reboot
    ///   vps.info         — Instance metadata (region, tier, provider)
    ///
    /// OpenClaw lifecycle:
    ///   openclaw.health   — HTTP health check on local OpenClaw process
    ///   openclaw.restart  — docker compose restart openclaw
    ///   openclaw.stop     — Graceful stop
    ///   openclaw.start    — Start if stopped
    ///   openclaw.update   — Pull new image and rolling restart
    ///   openclaw.logs     — Tail recent log lines
    ///
    /// Config management:
    ///   config.push       — Write new config, validate, hot-reload
    ///   config.get        — Return current config (redacted secrets)
    ///   config.diff       — Show diff vs proposed config
    ///   config.rollback   — Revert to last known-good config
    ///
    /// Process/Docker:
    ///   docker.ps         — List running containers
    ///   docker.restart    — Restart named container
    ///   docker.pull       — Pull latest image
    ///   docker.logs       — Container log tail
    ///
    /// Security:
    ///   ssh.audit         — List active SSH sessions
    ///   firewall.status   — UFW/iptables rule summary
    ///   tailscale.status  — Tailscale connection status + latency
    pub async fn register_all(_node: Arc<ClawNode>) {
        info!("Registering all command handlers");
        // TODO: register each handler with node.register(command_name, handler_fn)
    }
}

// ─── Config module ────────────────────────────────────────────────────────────

mod config {
    use anyhow::Result;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct NodeConfig {
        /// Unique instance UUID assigned by GatewayForge at provision time
        pub instance_id: String,
        /// OpenClaw gateway WebSocket URL (Tailscale internal address)
        pub gateway_url: String,
        /// API key for authenticating commands from the gateway
        pub api_key: String,
        /// VPS provider (hetzner | vultr | contabo | hostinger | digitalocean)
        pub provider: String,
        /// Instance region code (e.g. eu-hetzner-nbg1)
        pub region: String,
        /// Service tier (nano | standard | pro | enterprise)
        pub tier: String,
        /// User account ID this instance belongs to
        pub account_id: Option<String>,
        /// Whether this instance is the PRIMARY or STANDBY in a pair
        pub role: InstanceRole,
        /// The paired instance ID (other member of the primary/standby pair)
        pub pair_instance_id: Option<String>,
        /// Heartbeat interval in seconds (default 30)
        pub heartbeat_interval_secs: u64,
        /// Metrics collection interval in seconds (default 60)
        pub metrics_interval_secs: u64,
        /// Allowed command prefixes (allowlist for security)
        pub allowed_command_prefixes: Vec<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum InstanceRole {
        Primary,
        Standby,
    }

    impl NodeConfig {
        pub fn load(path: &str) -> Result<Self> {
            // Load from TOML file with env var overrides
            let _ = path;
            // TODO: use config crate to load from file + env
            Ok(Self {
                instance_id: std::env::var("INSTANCE_ID")
                    .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string()),
                gateway_url: std::env::var("GATEWAY_URL")
                    .unwrap_or_else(|_| "ws://localhost:8443".to_string()),
                api_key: std::env::var("GF_API_KEY").unwrap_or_default(),
                provider: std::env::var("NODE_PROVIDER")
                    .unwrap_or_else(|_| "hetzner".to_string()),
                region: std::env::var("NODE_REGION")
                    .unwrap_or_else(|_| "eu-hetzner-nbg1".to_string()),
                tier: std::env::var("NODE_TIER")
                    .unwrap_or_else(|_| "standard".to_string()),
                account_id: std::env::var("ACCOUNT_ID").ok(),
                role: InstanceRole::Primary,
                pair_instance_id: std::env::var("PAIR_INSTANCE_ID").ok(),
                heartbeat_interval_secs: 30,
                metrics_interval_secs: 60,
                allowed_command_prefixes: vec![
                    "vps.".to_string(),
                    "openclaw.".to_string(),
                    "config.".to_string(),
                    "docker.".to_string(),
                    "ssh.".to_string(),
                    "firewall.".to_string(),
                    "tailscale.".to_string(),
                ],
            })
        }
    }
}

// ─── Heartbeat module ─────────────────────────────────────────────────────────

mod heartbeat {
    use super::agent::ClawNode;
    use std::sync::Arc;
    use tracing::{debug, warn};

    pub async fn run(node: Arc<ClawNode>) {
        let interval = node.config.heartbeat_interval_secs;
        let mut ticker = tokio::time::interval(
            tokio::time::Duration::from_secs(interval),
        );
        loop {
            ticker.tick().await;
            debug!("Sending heartbeat");
            // TODO: POST heartbeat to GatewayForge API with current health snapshot
            // If heartbeat fails 3× consecutive, attempt self-reconnect
            if let Err(e) = send_heartbeat(&node).await {
                warn!("Heartbeat failed: {e}");
            }
        }
    }

    async fn send_heartbeat(_node: &ClawNode) -> anyhow::Result<()> {
        // TODO: collect quick health snapshot and POST to /v1/heartbeat
        Ok(())
    }
}

// ─── Metrics collector module ─────────────────────────────────────────────────

mod metrics_collector {
    use super::agent::ClawNode;
    use std::sync::Arc;
    use tracing::{debug, warn};

    pub async fn run(node: Arc<ClawNode>) {
        let interval = node.config.metrics_interval_secs;
        let mut ticker = tokio::time::interval(
            tokio::time::Duration::from_secs(interval),
        );
        loop {
            ticker.tick().await;
            debug!("Collecting metrics");
            if let Err(e) = collect_and_push(&node).await {
                warn!("Metrics collection failed: {e}");
            }
        }
    }

    async fn collect_and_push(_node: &ClawNode) -> anyhow::Result<()> {
        // TODO: use sysinfo to gather CPU, RAM, disk
        // TODO: use bollard to gather docker container stats
        // TODO: push to gf-metrics aggregator via WebSocket
        Ok(())
    }
}

// ─── WebSocket module ─────────────────────────────────────────────────────────

mod websocket {
    // TODO: implement WebSocket client with:
    // - TLS over Tailscale
    // - HMAC-SHA256 message authentication
    // - Automatic reconnect with exponential backoff
    // - Ping/pong keepalive
    // - Structured JSON message framing (matches gf-node-proto message types)
}
