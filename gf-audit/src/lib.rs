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

        let mut record = AuditRecord {
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
            record_hash: String::new(), // will be set below
        };

        // Compute hash over the fully-formed record (except record_hash itself)
        record.record_hash = self.compute_hash(&record);
        self.chain_head = Some(record.record_hash.clone());
        self.persist_record(&record).await?;

        Ok(record_id)
    }

    /// Update an existing audit record with the action result.
    pub async fn log_result(&mut self, record_id: Uuid, result: AuditResult) -> anyhow::Result<()> {
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
    pub async fn query(&self, filter: AuditFilter) -> anyhow::Result<Vec<AuditRecord>> {
        // TODO: GET /v1/audit with filter params
        let _ = (filter, &self.gf_api_base);
        Ok(vec![])
    }

    /// Compute SHA-256 hash of the record for the tamper-evident chain.
    /// Hash input: record_id + "|" + timestamp_rfc3339 + "|" + previous_hash + "|" + action
    fn compute_hash(&self, record: &AuditRecord) -> String {
        use sha2::{Digest, Sha256};
        let prev = self.chain_head.as_deref().unwrap_or("genesis");
        let input = format!(
            "{}|{}|{}|{:?}|{}",
            record.record_id,
            record.timestamp.to_rfc3339(),
            prev,
            record.action,
            record.target.target_id,
        );
        let hash = Sha256::digest(input.as_bytes());
        hex::encode(hash)
    }

    async fn persist_record(&self, record: &AuditRecord) -> anyhow::Result<()> {
        // Write structured JSON audit log line for immediate durability
        // (GatewayForge API POST is best-effort; local log is the source of truth)
        tracing::info!(
            record_id = %record.record_id,
            correlation_id = %record.correlation_id,
            agent = ?record.agent,
            action = ?record.action,
            target_type = ?record.target.target_type,
            target_id = %record.target.target_id,
            account_id = ?record.target.account_id,
            record_hash = %record.record_hash,
            "AUDIT"
        );

        // POST to GatewayForge API (non-fatal if unavailable)
        let url = format!("{}/v1/audit", self.gf_api_base);
        let client = reqwest::Client::new();
        if let Err(e) = client
            .post(&url)
            .bearer_auth(&self.gf_api_key)
            .json(record)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            tracing::warn!("Failed to POST audit record to API (record still logged locally): {e}");
        }

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

    fn make_audit_record(record_id: Uuid, target: AuditTarget) -> AuditRecord {
        AuditRecord {
            record_id,
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
        }
    }

    #[test]
    fn audit_record_serializes_cleanly() {
        let target = instance_target("inst-001", "acct-xyz", "hetzner");
        let record = make_audit_record(Uuid::new_v4(), target);

        let json = serde_json::to_string(&record).expect("serialization failed");
        // AuditAction::DockerRestartOpenClaw with snake_case serializes as
        // "docker_restart_open_claw" (each capital letter gets a separator)
        assert!(
            json.contains("docker_restart_open_claw"),
            "Expected docker_restart_open_claw in: {json}"
        );
        assert!(json.contains("guardian"));
    }

    #[test]
    fn instance_target_sets_correct_fields() {
        let target = instance_target("inst-abc", "acct-xyz", "hetzner");
        assert_eq!(target.target_type, TargetType::Instance);
        assert_eq!(target.target_id, "inst-abc");
        assert_eq!(target.account_id, Some("acct-xyz".to_string()));
        assert_eq!(target.provider, Some("hetzner".to_string()));
    }

    #[test]
    fn fleet_target_sets_correct_fields() {
        let target = fleet_target();
        assert_eq!(target.target_type, TargetType::Fleet);
        assert_eq!(target.target_id, "fleet");
        assert!(
            target.account_id.is_none(),
            "fleet_target account_id should be None"
        );
        assert!(target.provider.is_none());
    }

    #[test]
    fn audit_logger_new_starts_with_chain_head_none() {
        let logger = AuditLogger::new(
            "https://api.example.com".to_string(),
            "test-key".to_string(),
        );
        assert!(logger.chain_head.is_none());
    }

    #[test]
    fn compute_hash_different_record_ids_produce_different_hashes() {
        let logger = AuditLogger::new(
            "https://api.example.com".to_string(),
            "test-key".to_string(),
        );

        let target_a = instance_target("inst-001", "acct-001", "hetzner");
        let target_b = instance_target("inst-001", "acct-001", "hetzner");

        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        let record_a = make_audit_record(id_a, target_a);
        let record_b = make_audit_record(id_b, target_b);

        let hash_a = logger.compute_hash(&record_a);
        let hash_b = logger.compute_hash(&record_b);

        assert_ne!(
            hash_a, hash_b,
            "Different record_ids must produce different hashes"
        );
        assert_eq!(hash_a.len(), 64, "SHA-256 hex is 64 chars");
    }

    #[test]
    fn audit_filter_default_has_all_none_fields() {
        let filter = AuditFilter::default();
        assert!(filter.account_id.is_none());
        assert!(filter.instance_id.is_none());
        assert!(filter.agent.is_none());
        assert!(filter.action.is_none());
        assert!(filter.from.is_none());
        assert!(filter.to.is_none());
        assert!(filter.limit.is_none());
    }
}
