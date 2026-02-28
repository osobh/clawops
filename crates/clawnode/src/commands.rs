//! Command dispatch for VPS node invocations
//!
//! Handles commands sent from the OpenClaw gateway, routing to VPS-specific
//! handlers for health checks, metrics, Docker, config, secrets, and auth.

use crate::SharedState;
use serde_json::Value;
use tracing::debug;

/// Command request from gateway
#[derive(Debug, Clone)]
pub struct CommandRequest {
    pub command: String,
    pub params: Value,
}

/// Command error type
pub type CommandError = Box<dyn std::error::Error + Send + Sync>;

/// Handle an incoming command from the gateway.
pub async fn handle_command(
    state: &SharedState,
    request: CommandRequest,
) -> Result<Value, CommandError> {
    debug!(command = %request.command, "handling command");

    match request.command.as_str() {
        // ── System / VPS core ──────────────────────────────────────────────
        "system.info" | "vps.info" => {
            crate::vps_cmd::handle_vps_info(state).await
        }
        "vps.status" => crate::vps_cmd::handle_vps_status(state).await,
        "vps.metrics" => crate::vps_cmd::handle_vps_metrics(state).await,
        "vps.restart" => crate::vps_cmd::handle_vps_restart(state).await,
        "system.run" => crate::vps_cmd::handle_system_run(state, request.params).await,

        // ── OpenClaw gateway checks ────────────────────────────────────────
        "openclaw.health" => crate::vps_cmd::handle_openclaw_health(state).await,

        // ── Docker ────────────────────────────────────────────────────────
        "docker.status" => crate::vps_cmd::handle_docker_status(state).await,
        "docker.restart" => crate::vps_cmd::handle_docker_restart(state).await,

        // ── Health checks ─────────────────────────────────────────────────
        "health.check" | "node.health" => crate::health_cmd::handle_health_check(state).await,
        "health.score" => crate::health_cmd::handle_health_score(state).await,
        "node.capabilities" => crate::health_cmd::handle_node_capabilities(state).await,

        // ── Config commands ───────────────────────────────────────────────
        "config.create" | "config.set" | "config.get" | "config.update" | "config.delete"
        | "config.list" => crate::config_cmd::handle_config_command(state, request).await,

        // ── Secret commands ───────────────────────────────────────────────
        "secret.create" | "secret.get" | "secret.delete" | "secret.list" | "secret.rotate" => {
            crate::secrets_cmd::handle_secret_command(state, request).await
        }

        // ── Auth & audit commands ─────────────────────────────────────────
        "auth.create_key" | "auth.revoke_key" | "auth.list_keys" | "audit.query" => {
            crate::auth_cmd::handle_auth_command(state, request).await
        }

        unknown => Err(format!("unknown command: {unknown}").into()),
    }
}
