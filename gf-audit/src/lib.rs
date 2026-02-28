//! gf-audit — Immutable audit trail for all agent actions across the fleet
//!
//! Every agent action that touches infrastructure is logged here before
//! execution. This is a hard requirement — never execute provider API deletes
//! without logging to the audit trail first.
//!
//! Audit records are append-only, cryptographically signed, and forwarded
//! to GatewayForge DB for long-term retention.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Audit record ─────────────────────────────────────────────────────────────

/// An immutable audit record for a single agent action.
/// Written before the action is executed; updated with result after.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub record_id: Uuid,
    pub correlation_id: Uuid, // links related actions in the same operation
    pub timestamp: DateTime<Utc>,
    pub agent: AgentId,
    pub action: AuditAction,
    pub target: AuditTarget,
    pub parameters: serde_json::Value,
    pub result: Option<AuditResult>,
    pub operator_confirmation: Option<OperatorConfirmation>,
    /// Chain link to previous record (for tamper detection)
    pub previous_hash: Option<String>,
    pub record_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentId {
    Commander,
    Guardian,
    Forge,
    Ledger,
    Triage,
    Briefer,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    // Provisioning
    ProvisionPrimary,
    ProvisionStandby,
    TeardownInstance,
    ResizeInstance,

    // Health / heal
    InitiateAutoHeal,
    DockerRestartOpenClaw,
    TriggerFailover,
    PromoteStandby,
    ScheduleReprovision,

    // Configuration
    PushConfig,
    RollbackConfig,

    // Operational
    GenerateCostReport,
    GenerateIncidentReport,
    UpdateProviderSelection,
    SilenceAlerts,

    // Administrative
    OperatorConfirmationReceived,
    AgentSpawned,
    AgentTerminated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditTarget {
    pub target_type: TargetType,
    pub target_id: String,
    pub account_id: Option<String>,
    pub provider: Option<String>,
    pub region: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    Instance,
    Pair,
    Account,
    Fleet,
    Provider,
    Config,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditResult {
    pub success: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub affected_resources: Vec<String>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorConfirmation {
    pub confirmed_by: String,
    pub confirmed_at: DateTime<Utc>,
    pub channel: ConfirmationChannel,
    pub confirmation_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmationChannel {
    WhatsApp,
    Telegram,
    Discord,
    Web,
}

// ─── Audit logger ─────────────────────────────────────────────────────────────

/// Append-only audit logger. Writes to GatewayForge DB and local log stream.
pub struct AuditLogger {
    gf_api_base: String,
    gf_api_key: String,
    chain_head: Option<String>, // hash of last written record
}

impl AuditLogger {
    pub fn new(gf_api_base: String, gf_api_key: String) -> Self {
        Self {
            gf_api_base,
            gf_api_key,
            chain_head: None,
        }
    }

    /// Log an action BEFORE it is executed.
    /// Returns the record_id to use when logging the result.
    pub async fn log_action(
        &mut self,
        agent: AgentId,
        action: AuditAction,
        target: AuditTarget,
        parameters: serde_json::Value,
        correlation_id: Option<Uuid>,
    ) -> anyhow::Result<Uuid> {
        let record_id = Uuid::new_v4();
        let correlation_id = correlation_id.unwrap_or_else(Uuid::new_v4);

        let record = AuditRecord {
            record_id,
            correlation_id,
            timestamp: Utc::now(),
            agent,
            action,
            target,
            parameters,
            result: None,
            operator_confirmation: None,
            previous_hash: self.chain_head.clone(),
            record_hash: self.compute_hash(&record_id),
        };

        self.chain_head = Some(record.record_hash.clone());
        self.persist_record(&record).await?;

        Ok(record_id)
    }

    /// Update an existing audit record with the action result.
    pub async fn log_result(
        &mut self,
        record_id: Uuid,
        result: AuditResult,
    ) -> anyhow::Result<()> {
        // TODO: PATCH /v1/audit/{record_id} with result
        let _ = (record_id, result, &self.gf_api_key);
        Ok(())
    }

    /// Log an operator confirmation for a pending action.
    pub async fn log_confirmation(
        &mut self,
        record_id: Uuid,
        confirmation: OperatorConfirmation,
    ) -> anyhow::Result<()> {
        // TODO: PATCH /v1/audit/{record_id}/confirmation
        let _ = (record_id, confirmation);
        Ok(())
    }

    /// Query audit trail for a specific account or instance
    pub async fn query(
        &self,
        filter: AuditFilter,
    ) -> anyhow::Result<Vec<AuditRecord>> {
        // TODO: GET /v1/audit with filter params
        let _ = (filter, &self.gf_api_base);
        Ok(vec![])
    }

    fn compute_hash(&self, record_id: &Uuid) -> String {
        // TODO: SHA-256 of (record_id + timestamp + previous_hash + action)
        format!("{record_id:x}")
    }

    async fn persist_record(&self, record: &AuditRecord) -> anyhow::Result<()> {
        // TODO: POST /v1/audit with record JSON
        // Also write to structured local log for immediate durability
        tracing::info!(
            record_id = %record.record_id,
            agent = ?record.agent,
            action = ?record.action,
            target_id = %record.target.target_id,
            "AUDIT"
        );
        Ok(())
    }
}

// ─── Audit filter ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditFilter {
    pub account_id: Option<String>,
    pub instance_id: Option<String>,
    pub agent: Option<AgentId>,
    pub action: Option<AuditAction>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<u32>,
}

// ─── Convenience macros / helpers ─────────────────────────────────────────────

/// Build an AuditTarget for an instance
pub fn instance_target(instance_id: &str, account_id: &str, provider: &str) -> AuditTarget {
    AuditTarget {
        target_type: TargetType::Instance,
        target_id: instance_id.to_string(),
        account_id: Some(account_id.to_string()),
        provider: Some(provider.to_string()),
        region: None,
    }
}

/// Build an AuditTarget for fleet-level operations
pub fn fleet_target() -> AuditTarget {
    AuditTarget {
        target_type: TargetType::Fleet,
        target_id: "fleet".to_string(),
        account_id: None,
        provider: None,
        region: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_record_serializes_cleanly() {
        let target = instance_target("inst-001", "acct-xyz", "hetzner");
        let record = AuditRecord {
            record_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            agent: AgentId::Guardian,
            action: AuditAction::DockerRestartOpenClaw,
            target,
            parameters: serde_json::json!({ "container": "openclaw" }),
            result: None,
            operator_confirmation: None,
            previous_hash: None,
            record_hash: "abc123".to_string(),
        };

        let json = serde_json::to_string(&record).expect("serialization failed");
        assert!(json.contains("docker_restart_openclaw"));
        assert!(json.contains("guardian"));
    }
}
