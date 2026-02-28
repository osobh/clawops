//! clawnode — ClawOps VPS Node Agent
//!
//! Forked from clawbernetes/crates/clawnode. GPU/container/mesh logic removed.
//! VPS fleet management commands substituted.

#![forbid(unsafe_code)]

pub mod auth_cmd;
pub mod client;
pub mod commands;
pub mod config;
pub mod config_cmd;
pub mod error;
pub mod health_cmd;
pub mod identity;
pub mod persist;
pub mod secrets_cmd;
pub mod vps_cmd;

use std::sync::Arc;
use tokio::sync::RwLock;

pub use client::GatewayClient;
pub use config::NodeConfig;

// ─── Node state ───────────────────────────────────────────────────────────────

/// Mutable per-node runtime state.
#[derive(Debug)]
pub struct NodeState {
    pub config: NodeConfig,
    pub connected: bool,
    pub node_id: Option<String>,
    pub node_token: Option<String>,
    pub approved: bool,
    pub capabilities: Vec<String>,
    pub commands: Vec<String>,
}

impl NodeState {
    pub fn new(config: NodeConfig) -> Self {
        // VPS capabilities
        let capabilities = vec![
            "system".to_string(),
            "vps".to_string(),
            "docker".to_string(),
            "tailscale".to_string(),
            "openclaw".to_string(),
            "health".to_string(),
        ];

        let commands = vec![
            // VPS core
            "vps.info".to_string(),
            "vps.status".to_string(),
            "vps.metrics".to_string(),
            "vps.restart".to_string(),
            "system.info".to_string(),
            "system.run".to_string(),
            // OpenClaw
            "openclaw.health".to_string(),
            // Docker
            "docker.status".to_string(),
            "docker.restart".to_string(),
            // Health
            "health.check".to_string(),
            "health.score".to_string(),
            "node.health".to_string(),
            "node.capabilities".to_string(),
            // Config
            "config.create".to_string(),
            "config.get".to_string(),
            "config.set".to_string(),
            "config.update".to_string(),
            "config.delete".to_string(),
            "config.list".to_string(),
            // Secrets
            "secret.create".to_string(),
            "secret.get".to_string(),
            "secret.delete".to_string(),
            "secret.list".to_string(),
            "secret.rotate".to_string(),
            // Auth & audit
            "auth.create_key".to_string(),
            "auth.revoke_key".to_string(),
            "auth.list_keys".to_string(),
            "audit.query".to_string(),
        ];

        Self {
            config,
            connected: false,
            node_id: None,
            node_token: None,
            approved: false,
            capabilities,
            commands,
        }
    }
}

// ─── Shared state ─────────────────────────────────────────────────────────────

/// Shared state — passed by reference into every command handler.
pub struct SharedState {
    inner: Arc<RwLock<NodeState>>,
    pub capabilities: Vec<String>,
    pub commands: Vec<String>,
    pub node_token: Option<String>,

    // ─── Persistence stores ───────────────────────────────────────────────
    pub vps_store: Arc<RwLock<persist::VpsInstanceStore>>,
    pub event_store: Arc<RwLock<persist::EventStore>>,
    pub secret_store: Arc<RwLock<claw_secrets::SecretStore>>,
    pub config_store: Arc<RwLock<claw_config::ConfigStore>>,
    pub api_key_store: Arc<RwLock<claw_auth::ApiKeyStore>>,
    pub audit_log_store: Arc<RwLock<claw_auth::AuditLogStore>>,
}

impl SharedState {
    pub fn new(config: NodeConfig) -> Self {
        let state_path = config.state_path.clone();
        let state = NodeState::new(config);

        let capabilities = state.capabilities.clone();
        let commands = state.commands.clone();

        Self {
            capabilities,
            commands,
            node_token: None,
            inner: Arc::new(RwLock::new(state)),
            vps_store: Arc::new(RwLock::new(persist::VpsInstanceStore::new(&state_path))),
            event_store: Arc::new(RwLock::new(persist::EventStore::new(&state_path))),
            secret_store: Arc::new(RwLock::new(claw_secrets::SecretStore::new(&state_path))),
            config_store: Arc::new(RwLock::new(claw_config::ConfigStore::new(&state_path))),
            api_key_store: Arc::new(RwLock::new(claw_auth::ApiKeyStore::new(&state_path))),
            audit_log_store: Arc::new(RwLock::new(claw_auth::AuditLogStore::new(&state_path))),
        }
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, NodeState> {
        self.inner.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, NodeState> {
        self.inner.write().await
    }
}

/// Create shared state from config.
pub fn create_state(config: NodeConfig) -> SharedState {
    SharedState::new(config)
}
