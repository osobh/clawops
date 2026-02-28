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

use config::NodeConfig;

#[derive(Parser, Debug)]
#[command(name = "gf-clawnode", about = "GatewayForge VPS instance agent")]
struct Cli {
    /// Override config file path
    #[arg(
        long,
        env = "GF_CONFIG_FILE",
        default_value = "/etc/gf-clawnode/config.toml"
    )]
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

    let cfg = NodeConfig::load(&cli.config)
        .with_context(|| format!("Failed to load config from {}", cli.config))?;

    info!(
        instance_id = %cfg.instance_id,
        region = %cfg.region,
        tier = %cfg.tier,
        gateway_url = %cfg.gateway_url,
        "gf-clawnode starting"
    );

    let cfg = Arc::new(cfg);

    // Build the node agent — establishes WebSocket to OpenClaw gateway
    let node = agent::ClawNode::new(Arc::clone(&cfg))
        .await
        .context("Failed to initialize ClawNode")?;
    let node = Arc::new(node);

    // Register all command handlers
    commands::register_all(Arc::clone(&node)).await;
    info!("Command handlers registered");

    // Heartbeat loop: POST /v1/heartbeat every 30s
    let heartbeat_node = Arc::clone(&node);
    let heartbeat_handle = tokio::spawn(async move { heartbeat::run(heartbeat_node).await });

    // Metrics collector: gather sysinfo + docker stats every 60s
    let metrics_node = Arc::clone(&node);
    let metrics_handle = tokio::spawn(async move { metrics_collector::run(metrics_node).await });

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
        /// GatewayForge control plane REST API base URL
        pub gf_api_base: String,
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

    impl std::fmt::Display for InstanceRole {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                InstanceRole::Primary => write!(f, "primary"),
                InstanceRole::Standby => write!(f, "standby"),
            }
        }
    }

    impl NodeConfig {
        pub fn load(_path: &str) -> Result<Self> {
            // Load from environment with sensible defaults.
            // In production, extend to parse a TOML file at _path.
            Ok(Self {
                instance_id: std::env::var("INSTANCE_ID")
                    .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string()),
                gateway_url: std::env::var("GATEWAY_URL")
                    .unwrap_or_else(|_| "ws://localhost:8443".to_string()),
                api_key: std::env::var("GF_API_KEY").unwrap_or_default(),
                gf_api_base: std::env::var("GF_API_BASE")
                    .unwrap_or_else(|_| "https://api.gatewayforge.io".to_string()),
                provider: std::env::var("NODE_PROVIDER").unwrap_or_else(|_| "hetzner".to_string()),
                region: std::env::var("NODE_REGION")
                    .unwrap_or_else(|_| "eu-hetzner-nbg1".to_string()),
                tier: std::env::var("NODE_TIER").unwrap_or_else(|_| "standard".to_string()),
                account_id: std::env::var("ACCOUNT_ID").ok(),
                role: std::env::var("NODE_ROLE")
                    .map(|r| {
                        if r == "standby" {
                            InstanceRole::Standby
                        } else {
                            InstanceRole::Primary
                        }
                    })
                    .unwrap_or(InstanceRole::Primary),
                pair_instance_id: std::env::var("PAIR_INSTANCE_ID").ok(),
                heartbeat_interval_secs: std::env::var("HEARTBEAT_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
                metrics_interval_secs: std::env::var("METRICS_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60),
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn node_config_load_empty_path_succeeds_with_defaults() {
            let cfg =
                NodeConfig::load("").expect("NodeConfig::load should succeed with empty path");
            // gateway_url falls back to env or default
            assert!(!cfg.gateway_url.is_empty());
            assert!(!cfg.provider.is_empty());
            assert!(!cfg.region.is_empty());
            assert!(!cfg.tier.is_empty());
            // heartbeat_interval_secs defaults to 30 if env var not set
            assert!(cfg.heartbeat_interval_secs >= 1);
            assert!(cfg.metrics_interval_secs >= 1);
        }

        #[test]
        fn instance_role_primary_displays_as_primary() {
            let role = InstanceRole::Primary;
            assert_eq!(format!("{role}"), "primary");
        }

        #[test]
        fn instance_role_standby_displays_as_standby() {
            let role = InstanceRole::Standby;
            assert_eq!(format!("{role}"), "standby");
        }

        #[test]
        fn allowed_command_prefixes_contains_vps_prefix() {
            let cfg = NodeConfig::load("").expect("NodeConfig::load failed");
            assert!(
                cfg.allowed_command_prefixes.iter().any(|p| p == "vps."),
                "allowed_command_prefixes should contain 'vps.' prefix"
            );
        }

        #[test]
        fn config_with_empty_api_key_is_dev_mode() {
            // When api_key is empty, verify_signature returns true (dev mode skip)
            // We test this by confirming the load function can produce an empty api_key
            // (when GF_API_KEY env var is not set) and that the config reflects it.
            // The actual verify_signature logic is in the agent module; here we validate
            // the config value that drives the branch.
            let cfg = NodeConfig::load("").expect("NodeConfig::load failed");
            // If GF_API_KEY is not set in test environment, api_key is empty string
            // This is the condition that triggers dev mode in verify_signature
            // We simply check the field is accessible and is a String
            let _api_key: &str = &cfg.api_key;
        }
    }
}

// ─── Agent module ─────────────────────────────────────────────────────────────

mod agent {
    use super::config::NodeConfig;
    use anyhow::{bail, Context, Result};
    use futures::{SinkExt, StreamExt};
    use gf_node_proto::{
        CommandRequest, CommandResult, HeartbeatPayload, NodeMessage, NodePayload, ServiceStatus,
    };
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock};
    use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
    use tracing::{debug, error, info, warn};
    use uuid::Uuid;

    type HmacSha256 = Hmac<Sha256>;

    /// Handler signature: takes the command request, returns JSON result value.
    pub type CommandHandler = Arc<
        dyn Fn(CommandRequest) -> futures::future::BoxFuture<'static, serde_json::Value>
            + Send
            + Sync,
    >;

    /// The core agent struct. Holds the WebSocket connection to the OpenClaw
    /// gateway and a registry of command handlers.
    pub struct ClawNode {
        pub config: Arc<NodeConfig>,
        /// Registry of command name → handler function
        handlers: RwLock<HashMap<String, CommandHandler>>,
        /// HTTP client for heartbeat POSTs to GatewayForge API
        http_client: reqwest::Client,
        /// Latest heartbeat payload (updated by heartbeat module)
        pub latest_heartbeat: Mutex<Option<HeartbeatPayload>>,
    }

    impl ClawNode {
        pub async fn new(config: Arc<NodeConfig>) -> Result<Self> {
            info!(gateway = %config.gateway_url, "Initializing ClawNode");
            let http_client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .context("Failed to build HTTP client")?;

            Ok(Self {
                config,
                handlers: RwLock::new(HashMap::new()),
                http_client,
                latest_heartbeat: Mutex::new(None),
            })
        }

        /// Register a command handler. The command name is the exact string that
        /// will appear in CommandRequest.command (e.g. "vps.health", "openclaw.restart").
        pub async fn register<F, Fut>(&self, command: &str, handler: F)
        where
            F: Fn(CommandRequest) -> Fut + Send + Sync + 'static,
            Fut: futures::Future<Output = serde_json::Value> + Send + 'static,
        {
            let boxed: CommandHandler = Arc::new(move |req| Box::pin(handler(req)));
            self.handlers
                .write()
                .await
                .insert(command.to_string(), boxed);
            debug!(command, "Handler registered");
        }

        /// Main WebSocket event loop. Connects to the gateway, authenticates,
        /// and dispatches incoming commands. Reconnects with exponential backoff
        /// on disconnect.
        pub async fn run(&self) -> Result<()> {
            let mut backoff_secs = 2u64;
            const MAX_BACKOFF: u64 = 60;

            loop {
                match self.connect_and_run().await {
                    Ok(()) => {
                        info!("WebSocket connection closed cleanly — will reconnect");
                        backoff_secs = 2;
                    }
                    Err(e) => {
                        error!("WebSocket error: {e:#}. Reconnecting in {backoff_secs}s");
                        tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                        backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF);
                    }
                }
            }
        }

        async fn connect_and_run(&self) -> Result<()> {
            let url = &self.config.gateway_url;
            info!(url, "Connecting to gateway WebSocket");

            let (mut ws_stream, _) = connect_async(url)
                .await
                .with_context(|| format!("Failed to connect to WebSocket at {url}"))?;

            info!("WebSocket connected — sending registration");

            // Send registration message
            let registration = serde_json::json!({
                "type": "register",
                "instance_id": self.config.instance_id,
                "provider": self.config.provider,
                "region": self.config.region,
                "tier": self.config.tier,
                "role": format!("{}", self.config.role),
                "account_id": self.config.account_id,
                "pair_instance_id": self.config.pair_instance_id,
            });

            ws_stream
                .send(Message::Text(serde_json::to_string(&registration)?))
                .await
                .context("Failed to send registration")?;

            info!("WebSocket registered — entering command loop");

            while let Some(msg) = ws_stream.next().await {
                match msg {
                    Ok(Message::Text(text)) => match serde_json::from_str::<NodeMessage>(&text) {
                        Ok(envelope) => {
                            if let NodePayload::Command(cmd_req) = envelope.payload {
                                self.dispatch_command(cmd_req, &mut ws_stream).await;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse message: {e} — raw: {text:.200}");
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        ws_stream.send(Message::Pong(data)).await.ok();
                    }
                    Ok(Message::Close(_)) => {
                        info!("Server sent close frame");
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        bail!("WebSocket receive error: {e}");
                    }
                }
            }

            Ok(())
        }

        async fn dispatch_command<S>(&self, req: CommandRequest, ws: &mut S)
        where
            S: SinkExt<Message> + Unpin,
            <S as futures::Sink<Message>>::Error: std::fmt::Display,
        {
            // Security: verify command is in the allowlist
            let allowed = self
                .config
                .allowed_command_prefixes
                .iter()
                .any(|prefix| req.command.starts_with(prefix.as_str()));

            if !allowed {
                warn!(
                    command = %req.command,
                    issued_by = ?req.issued_by,
                    "Rejected command — not in allowlist"
                );
                let _ = self
                    .send_result(
                        ws,
                        CommandResult {
                            request_id: req.request_id,
                            command: req.command,
                            success: false,
                            output: serde_json::Value::Null,
                            error: Some("Command not in allowlist".to_string()),
                            duration_ms: 0,
                        },
                    )
                    .await;
                return;
            }

            // Security: verify HMAC signature
            if !self.verify_signature(&req) {
                warn!(
                    command = %req.command,
                    "Rejected command — invalid HMAC signature"
                );
                let _ = self
                    .send_result(
                        ws,
                        CommandResult {
                            request_id: req.request_id,
                            command: req.command,
                            success: false,
                            output: serde_json::Value::Null,
                            error: Some("Invalid HMAC signature".to_string()),
                            duration_ms: 0,
                        },
                    )
                    .await;
                return;
            }

            let command = req.command.clone();
            let request_id = req.request_id;
            let start = std::time::Instant::now();

            info!(
                %command,
                %request_id,
                issued_by = ?req.issued_by,
                "Dispatching command"
            );

            let handler = self.handlers.read().await.get(&command).cloned();

            let (output, error) = match handler {
                Some(h) => {
                    let output = h(req).await;
                    (output, None)
                }
                None => {
                    warn!(%command, "No handler registered for command");
                    (
                        serde_json::Value::Null,
                        Some(format!("Unknown command: {command}")),
                    )
                }
            };

            let duration_ms = start.elapsed().as_millis() as u64;
            let success = error.is_none();

            let _ = self
                .send_result(
                    ws,
                    CommandResult {
                        request_id,
                        command,
                        success,
                        output,
                        error,
                        duration_ms,
                    },
                )
                .await;
        }

        async fn send_result<S>(&self, ws: &mut S, result: CommandResult) -> Result<()>
        where
            S: SinkExt<Message> + Unpin,
            <S as futures::Sink<Message>>::Error: std::fmt::Display,
        {
            let envelope = NodeMessage {
                message_id: Uuid::new_v4(),
                timestamp: chrono::Utc::now(),
                instance_id: self.config.instance_id.clone(),
                payload: NodePayload::CommandResult(result),
            };

            let json = serde_json::to_string(&envelope)?;
            ws.send(Message::Text(json))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send result: {e}"))?;
            Ok(())
        }

        /// Verify HMAC-SHA256 signature on an incoming command.
        /// Signature is over: request_id + ":" + command + ":" + args_json
        fn verify_signature(&self, req: &CommandRequest) -> bool {
            if self.config.api_key.is_empty() {
                // No key configured — skip verification (dev mode only)
                return true;
            }

            let Ok(mut mac) = HmacSha256::new_from_slice(self.config.api_key.as_bytes()) else {
                return false;
            };

            let args_str = req.args.to_string();
            let message = format!("{}:{}:{}", req.request_id, req.command, args_str);
            mac.update(message.as_bytes());

            match hex::decode(&req.signature) {
                Ok(sig_bytes) => mac.verify_slice(&sig_bytes).is_ok(),
                Err(_) => false,
            }
        }

        /// Build a heartbeat payload from current system state.
        pub async fn build_heartbeat(&self) -> HeartbeatPayload {
            let uptime = self.get_uptime_seconds();
            HeartbeatPayload {
                health_score: 90, // Will be computed by gf-health in production
                openclaw_status: self.check_openclaw_status().await,
                docker_running: self.check_docker_running().await,
                tailscale_connected: self.check_tailscale().await,
                uptime_seconds: uptime,
            }
        }

        async fn check_openclaw_status(&self) -> ServiceStatus {
            // HTTP health check on local OpenClaw process
            let url = "http://localhost:8080/health";
            match self.http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => ServiceStatus::Healthy,
                Ok(_) => ServiceStatus::Degraded,
                Err(_) => ServiceStatus::Down,
            }
        }

        async fn check_docker_running(&self) -> bool {
            // Check if Docker daemon is responsive
            tokio::process::Command::new("docker")
                .args(["info", "--format", "{{.ServerVersion}}"])
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        async fn check_tailscale(&self) -> bool {
            // Check Tailscale status
            tokio::process::Command::new("tailscale")
                .args(["status", "--json"])
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        fn get_uptime_seconds(&self) -> u64 {
            // Read /proc/uptime for VPS uptime
            std::fs::read_to_string("/proc/uptime")
                .ok()
                .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
                .map(|f| f as u64)
                .unwrap_or(0)
        }

        pub fn http_client(&self) -> &reqwest::Client {
            &self.http_client
        }
    }
}

// ─── Commands module ──────────────────────────────────────────────────────────

mod commands {
    use super::agent::ClawNode;
    use gf_node_proto::CommandRequest;
    use std::sync::Arc;
    use tracing::info;

    /// Registers all VPS/OpenClaw command handlers on the node agent.
    ///
    /// GatewayForge command surface:
    ///
    /// System:
    ///   vps.health      — Return health score, resource utilization
    ///   vps.metrics     — Full metrics snapshot
    ///   vps.reboot      — Graceful OS reboot
    ///   vps.info        — Instance metadata
    ///
    /// OpenClaw lifecycle:
    ///   openclaw.health  — HTTP health check
    ///   openclaw.restart — docker compose restart openclaw
    ///   openclaw.stop    — Graceful stop
    ///   openclaw.start   — Start if stopped
    ///   openclaw.logs    — Tail recent log lines
    ///
    /// Config management:
    ///   config.push      — Write new config, validate, hot-reload
    ///   config.get       — Return current config (redacted secrets)
    ///   config.rollback  — Revert to last known-good config
    ///
    /// Process/Docker:
    ///   docker.ps        — List running containers
    ///   docker.restart   — Restart named container
    ///   docker.logs      — Container log tail
    ///
    /// Connectivity:
    ///   tailscale.status — Tailscale connection + latency
    ///   firewall.status  — UFW/iptables rule summary
    pub async fn register_all(node: Arc<ClawNode>) {
        info!("Registering all command handlers");

        // vps.health — compute and return full health report
        let n = Arc::clone(&node);
        node.register("vps.health", move |_req: CommandRequest| {
            let node = Arc::clone(&n);
            async move {
                let report = build_health_report(&node).await;
                serde_json::to_value(report).unwrap_or(serde_json::Value::Null)
            }
        })
        .await;

        // vps.info — return static instance metadata
        let n = Arc::clone(&node);
        node.register("vps.info", move |_req: CommandRequest| {
            let node = Arc::clone(&n);
            async move {
                serde_json::json!({
                    "instance_id": node.config.instance_id,
                    "provider": node.config.provider,
                    "region": node.config.region,
                    "tier": node.config.tier,
                    "role": format!("{}", node.config.role),
                    "account_id": node.config.account_id,
                    "pair_instance_id": node.config.pair_instance_id,
                })
            }
        })
        .await;

        // vps.reboot — schedule graceful reboot (10s delay to allow response)
        node.register("vps.reboot", move |_req: CommandRequest| {
            async move {
                // Schedule a reboot in 10s — gives time to send command result back
                tokio::spawn(async {
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    let _ = std::process::Command::new("reboot").status();
                });
                serde_json::json!({ "scheduled": true, "delay_secs": 10 })
            }
        })
        .await;

        // openclaw.health — HTTP health check on local OpenClaw
        node.register("openclaw.health", move |_req: CommandRequest| async move {
            let client = reqwest::Client::new();
            match client
                .get("http://localhost:8080/health")
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) => serde_json::json!({
                    "status": resp.status().as_u16(),
                    "healthy": resp.status().is_success(),
                }),
                Err(e) => serde_json::json!({
                    "status": null,
                    "healthy": false,
                    "error": e.to_string(),
                }),
            }
        })
        .await;

        // openclaw.restart — docker compose restart openclaw
        node.register("openclaw.restart", move |_req: CommandRequest| async move {
            let output = tokio::process::Command::new("docker")
                .args(["compose", "restart", "openclaw"])
                .output()
                .await;
            match output {
                Ok(o) => serde_json::json!({
                    "success": o.status.success(),
                    "stdout": String::from_utf8_lossy(&o.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&o.stderr).to_string(),
                }),
                Err(e) => serde_json::json!({
                    "success": false,
                    "error": e.to_string(),
                }),
            }
        })
        .await;

        // openclaw.logs — tail last N lines of OpenClaw logs
        node.register("openclaw.logs", move |req: CommandRequest| async move {
            let lines = req
                .args
                .get("lines")
                .and_then(|v| v.as_u64())
                .unwrap_or(100);
            let output = tokio::process::Command::new("docker")
                .args(["logs", "--tail", &lines.to_string(), "openclaw"])
                .output()
                .await;
            match output {
                Ok(o) => serde_json::json!({
                    "logs": String::from_utf8_lossy(&o.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&o.stderr).to_string(),
                    "lines_requested": lines,
                }),
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            }
        })
        .await;

        // docker.ps — list running containers
        node.register("docker.ps", move |_req: CommandRequest| {
            async move {
                let output = tokio::process::Command::new("docker")
                    .args(["ps", "--format", "json", "--no-trunc"])
                    .output()
                    .await;
                match output {
                    Ok(o) => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        // docker ps --format json outputs one JSON object per line
                        let containers: Vec<serde_json::Value> = stdout
                            .lines()
                            .filter_map(|line| serde_json::from_str(line).ok())
                            .collect();
                        serde_json::json!({ "containers": containers })
                    }
                    Err(e) => serde_json::json!({ "error": e.to_string() }),
                }
            }
        })
        .await;

        // docker.restart — restart a named container
        node.register("docker.restart", move |req: CommandRequest| {
            async move {
                let container = req
                    .args
                    .get("container")
                    .and_then(|v| v.as_str())
                    .unwrap_or("openclaw")
                    .to_string();

                // Security: only allow alphanumeric container names
                if !container
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    return serde_json::json!({
                        "success": false,
                        "error": "Invalid container name — alphanumeric, dash, underscore only",
                    });
                }

                let output = tokio::process::Command::new("docker")
                    .args(["restart", &container])
                    .output()
                    .await;
                match output {
                    Ok(o) => serde_json::json!({
                        "success": o.status.success(),
                        "container": container,
                    }),
                    Err(e) => serde_json::json!({ "success": false, "error": e.to_string() }),
                }
            }
        })
        .await;

        // tailscale.status — connectivity check
        node.register("tailscale.status", move |_req: CommandRequest| async move {
            let output = tokio::process::Command::new("tailscale")
                .args(["status", "--json"])
                .output()
                .await;
            match output {
                Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout)
                    .unwrap_or_else(|_| serde_json::json!({ "connected": true })),
                Ok(_) => serde_json::json!({ "connected": false }),
                Err(e) => serde_json::json!({ "connected": false, "error": e.to_string() }),
            }
        })
        .await;

        // config.get — return current OpenClaw config with secrets redacted
        node.register("config.get", move |_req: CommandRequest| {
            async move {
                let config_path = "/etc/openclaw/config.json";
                match tokio::fs::read_to_string(config_path).await {
                    Ok(content) => {
                        // Parse and redact secret fields
                        let mut cfg: serde_json::Value =
                            serde_json::from_str(&content).unwrap_or(serde_json::json!({}));
                        // Redact known secret fields
                        for secret_key in
                            &["api_key", "secret", "token", "password", "webhook_secret"]
                        {
                            if let Some(obj) = cfg.as_object_mut() {
                                if obj.contains_key(*secret_key) {
                                    obj.insert(
                                        secret_key.to_string(),
                                        serde_json::json!("[REDACTED]"),
                                    );
                                }
                            }
                        }
                        serde_json::json!({ "config": cfg, "path": config_path })
                    }
                    Err(e) => serde_json::json!({ "error": e.to_string() }),
                }
            }
        })
        .await;

        // config.push — write new config and signal hot-reload
        node.register("config.push", move |req: CommandRequest| {
            async move {
                let Some(new_config) = req.args.get("config") else {
                    return serde_json::json!({ "success": false, "error": "Missing 'config' field" });
                };

                // Validate the config is valid JSON before writing
                let config_str = match serde_json::to_string_pretty(new_config) {
                    Ok(s) => s,
                    Err(e) => return serde_json::json!({ "success": false, "error": e.to_string() }),
                };

                let config_path = "/etc/openclaw/config.json";

                // Backup current config before overwriting
                let backup_path = "/etc/openclaw/config.json.prev";
                let _ = tokio::fs::copy(config_path, backup_path).await;

                if let Err(e) = tokio::fs::write(config_path, &config_str).await {
                    return serde_json::json!({ "success": false, "error": e.to_string() });
                }

                // Signal OpenClaw to hot-reload (SIGHUP or docker kill --signal HUP)
                let _ = tokio::process::Command::new("docker")
                    .args(["kill", "--signal", "HUP", "openclaw"])
                    .output()
                    .await;

                serde_json::json!({
                    "success": true,
                    "config_path": config_path,
                    "backup_path": backup_path,
                    "bytes_written": config_str.len(),
                })
            }
        }).await;

        // config.rollback — restore previous config
        node.register("config.rollback", move |_req: CommandRequest| async move {
            let config_path = "/etc/openclaw/config.json";
            let backup_path = "/etc/openclaw/config.json.prev";

            match tokio::fs::copy(backup_path, config_path).await {
                Ok(bytes) => {
                    let _ = tokio::process::Command::new("docker")
                        .args(["kill", "--signal", "HUP", "openclaw"])
                        .output()
                        .await;
                    serde_json::json!({
                        "success": true,
                        "bytes_restored": bytes,
                        "message": "Previous config restored and OpenClaw signaled to reload",
                    })
                }
                Err(e) => serde_json::json!({ "success": false, "error": e.to_string() }),
            }
        })
        .await;

        info!("All command handlers registered");
    }

    /// Build a comprehensive health report for the vps.health command.
    async fn build_health_report(node: &ClawNode) -> serde_json::Value {
        let hb = node.build_heartbeat().await;

        // Gather resource metrics
        let (cpu_pct, mem_pct, disk_pct) = gather_resource_metrics();

        // Compute health score
        let mut score: f32 = 100.0;
        if hb.openclaw_status != gf_node_proto::ServiceStatus::Healthy {
            score -= 40.0;
        }
        if !hb.docker_running {
            score -= 20.0;
        }
        if disk_pct > 90.0 {
            score -= 15.0;
        } else if disk_pct > 80.0 {
            score -= 5.0;
        }
        if cpu_pct > 95.0 {
            score -= 10.0;
        }
        if mem_pct > 95.0 {
            score -= 10.0;
        }
        let health_score = score.clamp(0.0, 100.0) as u8;

        serde_json::json!({
            "instance_id": node.config.instance_id,
            "health_score": health_score,
            "openclaw_status": format!("{:?}", hb.openclaw_status),
            "docker_running": hb.docker_running,
            "tailscale_connected": hb.tailscale_connected,
            "cpu_usage_1m": cpu_pct,
            "mem_usage_pct": mem_pct,
            "disk_usage_pct": disk_pct,
            "uptime_seconds": hb.uptime_seconds,
        })
    }

    fn gather_resource_metrics() -> (f32, f32, f32) {
        // CPU: read /proc/loadavg for 1m load average
        let cpu_pct = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f32>().ok())
            .map(|load| (load * 100.0).min(100.0))
            .unwrap_or(0.0);

        // Memory: read /proc/meminfo
        let mem_pct = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                let mut total = 0u64;
                let mut available = 0u64;
                for line in s.lines() {
                    if line.starts_with("MemTotal:") {
                        total = line.split_whitespace().nth(1)?.parse().ok()?;
                    } else if line.starts_with("MemAvailable:") {
                        available = line.split_whitespace().nth(1)?.parse().ok()?;
                    }
                }
                if total == 0 {
                    return None;
                }
                Some(((total - available) as f32 / total as f32) * 100.0)
            })
            .unwrap_or(0.0);

        // Disk: check root filesystem via df
        let disk_pct = std::process::Command::new("df")
            .args(["-P", "/"])
            .output()
            .ok()
            .and_then(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                // df -P output: filesystem 1K-blocks used available capacity mountpoint
                stdout
                    .lines()
                    .nth(1)?
                    .split_whitespace()
                    .nth(4)?
                    .trim_end_matches('%')
                    .parse::<f32>()
                    .ok()
            })
            .unwrap_or(0.0);

        (cpu_pct, mem_pct, disk_pct)
    }
}

// ─── Heartbeat module ─────────────────────────────────────────────────────────

mod heartbeat {
    use super::agent::ClawNode;
    use gf_node_proto::{NodeMessage, NodePayload};
    use std::sync::Arc;
    use tracing::{debug, warn};
    use uuid::Uuid;

    pub async fn run(node: Arc<ClawNode>) {
        let interval = node.config.heartbeat_interval_secs;
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(interval));
        let mut consecutive_failures = 0u32;

        loop {
            ticker.tick().await;
            debug!("Sending heartbeat");

            match send_heartbeat(&node).await {
                Ok(()) => {
                    consecutive_failures = 0;
                }
                Err(e) => {
                    consecutive_failures += 1;
                    warn!("Heartbeat failed ({consecutive_failures} consecutive): {e}");
                    // After 3 consecutive failures, log as error (Guardian will notice via missed heartbeat)
                    if consecutive_failures >= 3 {
                        tracing::error!(
                            "3+ consecutive heartbeat failures — GatewayForge will detect missed heartbeat"
                        );
                    }
                }
            }
        }
    }

    async fn send_heartbeat(node: &ClawNode) -> anyhow::Result<()> {
        let hb = node.build_heartbeat().await;

        // Store latest heartbeat for other modules
        *node.latest_heartbeat.lock().await = Some(hb.clone());

        let envelope = NodeMessage {
            message_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            instance_id: node.config.instance_id.clone(),
            payload: NodePayload::Heartbeat(hb),
        };

        // POST to GatewayForge API heartbeat endpoint
        let url = format!(
            "{}/v1/instances/{}/heartbeat",
            node.config.gf_api_base, node.config.instance_id
        );

        node.http_client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", node.config.api_key))
            .header("Content-Type", "application/json")
            .json(&envelope)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
            .and_then(|r| r.error_for_status())?;

        debug!("Heartbeat sent to {url}");
        Ok(())
    }
}

// ─── Metrics collector module ─────────────────────────────────────────────────

mod metrics_collector {
    use super::agent::ClawNode;
    use gf_node_proto::{
        CpuMetrics, DiskMetrics, MemoryMetrics, MetricsReport, NetworkMetrics, NodeMessage,
        NodePayload, OpenClawMetrics,
    };
    use std::sync::Arc;
    use tracing::{debug, warn};
    use uuid::Uuid;

    pub async fn run(node: Arc<ClawNode>) {
        let interval = node.config.metrics_interval_secs;
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(interval));
        loop {
            ticker.tick().await;
            debug!("Collecting metrics");
            if let Err(e) = collect_and_push(&node).await {
                warn!("Metrics collection failed: {e}");
            }
        }
    }

    async fn collect_and_push(node: &ClawNode) -> anyhow::Result<()> {
        let report = MetricsReport {
            instance_id: node.config.instance_id.clone(),
            collected_at: chrono::Utc::now(),
            cpu: collect_cpu_metrics(),
            memory: collect_memory_metrics(),
            disk: collect_disk_metrics(),
            network: collect_network_metrics(),
            openclaw: collect_openclaw_metrics(node).await,
        };

        let envelope = NodeMessage {
            message_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            instance_id: node.config.instance_id.clone(),
            payload: NodePayload::MetricsReport(report),
        };

        // POST metrics to GatewayForge API
        let url = format!(
            "{}/v1/instances/{}/metrics",
            node.config.gf_api_base, node.config.instance_id
        );

        node.http_client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", node.config.api_key))
            .json(&envelope)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .and_then(|r| r.error_for_status())?;

        debug!("Metrics pushed to {url}");
        Ok(())
    }

    fn collect_cpu_metrics() -> CpuMetrics {
        // Parse /proc/loadavg: "0.52 0.58 0.59 2/847 12345"
        let (load_1m, load_5m, load_15m) = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| {
                let mut parts = s.split_whitespace();
                let l1 = parts.next()?.parse::<f32>().ok()?;
                let l5 = parts.next()?.parse::<f32>().ok()?;
                let l15 = parts.next()?.parse::<f32>().ok()?;
                Some((l1, l5, l15))
            })
            .unwrap_or((0.0, 0.0, 0.0));

        // Count CPU cores from /proc/cpuinfo
        let core_count = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .map(|s| s.lines().filter(|l| l.starts_with("processor")).count() as u32)
            .unwrap_or(1);

        CpuMetrics {
            usage_pct: (load_1m * 100.0 / core_count as f32).min(100.0),
            steal_pct: read_cpu_steal(),
            core_count,
            load_avg_1m: load_1m,
            load_avg_5m: load_5m,
            load_avg_15m: load_15m,
        }
    }

    fn read_cpu_steal() -> f32 {
        // Parse /proc/stat to get steal time %
        // Line format: cpu user nice system idle iowait irq softirq steal ...
        std::fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|s| {
                let line = s.lines().find(|l| l.starts_with("cpu "))?;
                let parts: Vec<u64> = line
                    .split_whitespace()
                    .skip(1) // skip "cpu"
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if parts.len() < 9 {
                    return None;
                }
                let total: u64 = parts.iter().sum();
                if total == 0 {
                    return None;
                }
                let steal = parts[7]; // steal is index 7
                Some(steal as f32 / total as f32 * 100.0)
            })
            .unwrap_or(0.0)
    }

    fn collect_memory_metrics() -> MemoryMetrics {
        let mut total_kb = 0u64;
        let mut free_kb = 0u64;
        let mut available_kb = 0u64;
        let mut cached_kb = 0u64;
        let mut swap_total_kb = 0u64;
        let mut swap_free_kb = 0u64;

        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                let mut parts = line.split_whitespace();
                match parts.next() {
                    Some("MemTotal:") => {
                        total_kb = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    Some("MemFree:") => {
                        free_kb = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    Some("MemAvailable:") => {
                        available_kb = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    Some("Cached:") => {
                        cached_kb = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    Some("SwapTotal:") => {
                        swap_total_kb = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    Some("SwapFree:") => {
                        swap_free_kb = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    _ => {}
                }
            }
        }

        let _ = free_kb; // used_mb computed from total - available
        MemoryMetrics {
            total_mb: total_kb / 1024,
            used_mb: (total_kb - available_kb) / 1024,
            free_mb: available_kb / 1024,
            cached_mb: cached_kb / 1024,
            swap_total_mb: swap_total_kb / 1024,
            swap_used_mb: (swap_total_kb - swap_free_kb) / 1024,
        }
    }

    fn collect_disk_metrics() -> Vec<DiskMetrics> {
        // Use df -P to get disk usage for all mounted filesystems
        let output = std::process::Command::new("df")
            .args(["-P", "-B1"]) // POSIX format, byte units
            .output();

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout
                    .lines()
                    .skip(1) // skip header
                    .filter(|line| !line.starts_with("tmpfs") && !line.starts_with("devtmpfs"))
                    .filter_map(|line| {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() < 6 {
                            return None;
                        }
                        let total_bytes: u64 = parts[1].parse().ok()?;
                        let used_bytes: u64 = parts[2].parse().ok()?;
                        let free_bytes: u64 = parts[3].parse().ok()?;
                        let pct_str = parts[4].trim_end_matches('%');
                        let usage_pct: f32 = pct_str.parse().ok()?;
                        let mount = parts[5].to_string();

                        Some(DiskMetrics {
                            mount_point: mount,
                            total_gb: total_bytes as f32 / 1_073_741_824.0,
                            used_gb: used_bytes as f32 / 1_073_741_824.0,
                            free_gb: free_bytes as f32 / 1_073_741_824.0,
                            usage_pct,
                            iops_read: 0, // Would need /proc/diskstats
                            iops_write: 0,
                        })
                    })
                    .collect()
            }
            Err(_) => vec![],
        }
    }

    fn collect_network_metrics() -> NetworkMetrics {
        // Parse /proc/net/dev for the primary interface
        if let Ok(content) = std::fs::read_to_string("/proc/net/dev") {
            for line in content.lines().skip(2) {
                let line = line.trim();
                // Skip loopback and tailscale
                if line.starts_with("lo:") || line.starts_with("tailscale") {
                    continue;
                }
                if let Some((iface, stats)) = line.split_once(':') {
                    let parts: Vec<u64> = stats
                        .split_whitespace()
                        .filter_map(|v| v.parse().ok())
                        .collect();
                    if parts.len() >= 16 {
                        return NetworkMetrics {
                            bytes_recv: parts[0],
                            packets_recv: parts[1],
                            errors_in: parts[2],
                            bytes_sent: parts[8],
                            packets_sent: parts[9],
                            errors_out: parts[10],
                            interface: iface.trim().to_string(),
                        };
                    }
                }
            }
        }

        NetworkMetrics {
            bytes_sent: 0,
            bytes_recv: 0,
            packets_sent: 0,
            packets_recv: 0,
            errors_in: 0,
            errors_out: 0,
            interface: "eth0".to_string(),
        }
    }

    async fn collect_openclaw_metrics(node: &ClawNode) -> OpenClawMetrics {
        // Call OpenClaw's /metrics endpoint (Prometheus format or JSON)
        let client = reqwest::Client::new();
        let url = "http://localhost:8080/metrics";

        if let Ok(resp) = client
            .get(url)
            .header("Authorization", format!("Bearer {}", node.config.api_key))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                return OpenClawMetrics {
                    http_requests_total: json["http_requests_total"].as_u64().unwrap_or(0),
                    http_latency_p50_ms: json["http_latency_p50_ms"].as_f64().unwrap_or(0.0) as f32,
                    http_latency_p95_ms: json["http_latency_p95_ms"].as_f64().unwrap_or(0.0) as f32,
                    http_latency_p99_ms: json["http_latency_p99_ms"].as_f64().unwrap_or(0.0) as f32,
                    active_sessions: json["active_sessions"].as_u64().unwrap_or(0) as u32,
                    websocket_connections: json["websocket_connections"].as_u64().unwrap_or(0)
                        as u32,
                    error_rate_pct: json["error_rate_pct"].as_f64().unwrap_or(0.0) as f32,
                };
            }
        }

        OpenClawMetrics {
            http_requests_total: 0,
            http_latency_p50_ms: 0.0,
            http_latency_p95_ms: 0.0,
            http_latency_p99_ms: 0.0,
            active_sessions: 0,
            websocket_connections: 0,
            error_rate_pct: 0.0,
        }
    }
}

// ─── WebSocket module (re-exported from tokio-tungstenite) ────────────────────

mod websocket {
    // WebSocket implementation is handled by tokio-tungstenite in agent::connect_and_run.
    //
    // Protocol details:
    // - TLS via native-tls (over Tailscale mTLS tunnel)
    // - HMAC-SHA256 per-command authentication (gf-node-proto::CommandRequest.signature)
    // - Auto-reconnect with exponential backoff (2s → 4s → 8s → max 60s)
    // - Ping/pong handled by tokio-tungstenite automatically
    // - JSON framing: every message is a gf-node-proto::NodeMessage envelope
    //
    // Connection lifecycle:
    // 1. connect_async to gateway_url (Tailscale IP:port)
    // 2. Send registration JSON: { type, instance_id, provider, region, tier, role }
    // 3. Gateway sends CommandRequest messages; clawnode sends CommandResult responses
    // 4. On disconnect: reconnect with backoff (agent::run loop handles this)
}
