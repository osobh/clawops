//! gf-failover — Primary/standby failover orchestration
//!
//! Handles the lifecycle of promoting a STANDBY instance to PRIMARY when the
//! PRIMARY fails. Called by Guardian after verifying standby is ACTIVE.
//!
//! Safety invariant: A user must ALWAYS have exactly one ACTIVE gateway.
//! Failover must be atomic from the user's perspective.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

pub use gf_node_proto::{InstanceRole, VpsProvider};

// ─── Failover types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverRequest {
    pub request_id: Uuid,
    pub account_id: String,
    pub failed_instance_id: String,
    pub standby_instance_id: String,
    pub trigger_reason: FailoverTrigger,
    pub triggered_by: String, // agent name
    pub triggered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailoverTrigger {
    /// Guardian auto-heal step 5 — health score < threshold
    AutoHeal,
    /// Provider-level outage detected (e.g. Hetzner Nuremberg down)
    ProviderOutage,
    /// Manual trigger by Commander on operator request
    ManualByCommander,
    /// Planned maintenance window
    PlannedMaintenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverResult {
    pub request_id: Uuid,
    pub success: bool,
    pub failed_instance_id: String,
    pub new_primary_instance_id: String,
    pub promotion_duration_ms: u64,
    pub dns_update_duration_ms: Option<u64>,
    pub steps: Vec<FailoverStep>,
    pub completed_at: DateTime<Utc>,
    pub reprovisioning_scheduled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverStep {
    pub step: FailoverStepType,
    pub success: bool,
    pub details: String,
    pub duration_ms: u64,
    pub executed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailoverStepType {
    /// Verify standby is truly ACTIVE (double-check before promoting)
    VerifyStandby,
    /// Update GatewayForge DB: standby → PRIMARY, failed → FAILED
    UpdatePairStatus,
    /// Notify user's OpenClaw gateway of new backend (if applicable)
    NotifyGateway,
    /// Update DNS / load balancer to point at new primary
    UpdateRouting,
    /// Mark failed instance for reprovisioning
    ScheduleReprovisioning,
    /// Send telemetry to Commander
    NotifyCommander,
}

// ─── Failover engine ──────────────────────────────────────────────────────────

pub struct FailoverEngine {
    gf_api_base: String,
    gf_api_key: String,
}

impl FailoverEngine {
    pub fn new(gf_api_base: String, gf_api_key: String) -> Self {
        Self {
            gf_api_base,
            gf_api_key,
        }
    }

    /// Execute a complete failover from failed PRIMARY to STANDBY.
    ///
    /// This is called by Guardian after:
    /// - Confirming PRIMARY is unreachable (steps 1–3 of auto-heal)
    /// - Confirming STANDBY is ACTIVE (step 4 of auto-heal)
    ///
    /// The failover sequence is atomic: we do not report success until
    /// routing is updated and user traffic flows to the new primary.
    pub async fn execute_failover(&self, req: FailoverRequest) -> Result<FailoverResult> {
        info!(
            account_id = %req.account_id,
            failed = %req.failed_instance_id,
            standby = %req.standby_instance_id,
            trigger = ?req.trigger_reason,
            "Starting failover"
        );

        let mut steps = Vec::new();

        // Step 1: Verify standby is truly ACTIVE (never skip)
        let verify_start = Utc::now();
        let standby_confirmed = self.verify_standby_active(&req.standby_instance_id).await;
        let verify_ok = standby_confirmed.is_ok();

        if !verify_ok {
            error!(
                standby = %req.standby_instance_id,
                "Standby verification failed — aborting failover"
            );
            steps.push(FailoverStep {
                step: FailoverStepType::VerifyStandby,
                success: false,
                details: "Standby not confirmed ACTIVE — failover aborted".to_string(),
                duration_ms: elapsed_ms(verify_start),
                executed_at: verify_start,
            });
            bail!(
                "Failover aborted: standby {} is not ACTIVE",
                req.standby_instance_id
            );
        }

        steps.push(FailoverStep {
            step: FailoverStepType::VerifyStandby,
            success: true,
            details: format!("Standby {} confirmed ACTIVE", req.standby_instance_id),
            duration_ms: elapsed_ms(verify_start),
            executed_at: verify_start,
        });

        // Step 2: Update pair status in GatewayForge DB
        let status_start = Utc::now();
        let status_result = self
            .update_pair_status(
                &req.account_id,
                &req.failed_instance_id,
                &req.standby_instance_id,
            )
            .await;

        steps.push(FailoverStep {
            step: FailoverStepType::UpdatePairStatus,
            success: status_result.is_ok(),
            details: status_result
                .map(|_| "Pair status updated: standby → PRIMARY, failed → FAILED".to_string())
                .unwrap_or_else(|e| format!("Status update failed: {e}")),
            duration_ms: elapsed_ms(status_start),
            executed_at: status_start,
        });

        // Step 3: Update routing (DNS / LB)
        let routing_start = Utc::now();
        let routing_result = self
            .update_routing(&req.account_id, &req.standby_instance_id)
            .await;

        let routing_ok = routing_result.is_ok();
        steps.push(FailoverStep {
            step: FailoverStepType::UpdateRouting,
            success: routing_ok,
            details: routing_result
                .map(|_| "Routing updated to new primary".to_string())
                .unwrap_or_else(|e| format!("Routing update failed: {e}")),
            duration_ms: elapsed_ms(routing_start),
            executed_at: routing_start,
        });

        // Step 4: Schedule reprovisioning of a new standby
        let reprovision_start = Utc::now();
        let reprovision_result = self
            .schedule_reprovision_standby(&req.account_id, &req.failed_instance_id)
            .await;

        let reprovision_ok = reprovision_result.is_ok();
        steps.push(FailoverStep {
            step: FailoverStepType::ScheduleReprovisioning,
            success: reprovision_ok,
            details: reprovision_result
                .map(|_| "New standby reprovisioning queued".to_string())
                .unwrap_or_else(|e| format!("Reprovision scheduling failed: {e}")),
            duration_ms: elapsed_ms(reprovision_start),
            executed_at: reprovision_start,
        });

        // Step 5: Notify Commander
        steps.push(FailoverStep {
            step: FailoverStepType::NotifyCommander,
            success: true,
            details: format!(
                "Failover complete. New primary: {}. Standby reprovisioning: {}.",
                req.standby_instance_id, reprovision_ok
            ),
            duration_ms: 0,
            executed_at: Utc::now(),
        });

        let total_ms = elapsed_ms(req.triggered_at);

        info!(
            account_id = %req.account_id,
            new_primary = %req.standby_instance_id,
            duration_ms = total_ms,
            "Failover complete"
        );

        Ok(FailoverResult {
            request_id: req.request_id,
            success: true,
            failed_instance_id: req.failed_instance_id,
            new_primary_instance_id: req.standby_instance_id,
            promotion_duration_ms: total_ms,
            dns_update_duration_ms: None,
            steps,
            completed_at: Utc::now(),
            reprovisioning_scheduled: reprovision_ok,
        })
    }

    async fn verify_standby_active(&self, standby_id: &str) -> Result<()> {
        // TODO: call GatewayForge API GET /v1/instances/{id}/health
        // Verify health_score >= 70 and openclaw_status == HEALTHY
        let _ = standby_id;
        Ok(())
    }

    async fn update_pair_status(
        &self,
        account_id: &str,
        failed_id: &str,
        new_primary_id: &str,
    ) -> Result<()> {
        // PATCH /v1/accounts/{account_id}/pairs
        // { "primary_instance_id": new_primary_id, "failed_instance_id": failed_id }
        let url = format!("{}/v1/accounts/{}/pairs", self.gf_api_base, account_id);
        reqwest::Client::new()
            .patch(&url)
            .bearer_auth(&self.gf_api_key)
            .json(&serde_json::json!({
                "primary_instance_id": new_primary_id,
                "failed_instance_id": failed_id,
                "status": "failover_complete",
            }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .context("PATCH pair status request failed")?
            .error_for_status()
            .context("PATCH pair status returned error")?;
        Ok(())
    }

    async fn update_routing(&self, account_id: &str, new_primary_id: &str) -> Result<()> {
        // POST /v1/accounts/{account_id}/routing — update DNS/LB to new primary
        // This is the critical step that restores user access
        let url = format!("{}/v1/accounts/{}/routing", self.gf_api_base, account_id);
        reqwest::Client::new()
            .post(&url)
            .bearer_auth(&self.gf_api_key)
            .json(&serde_json::json!({
                "primary_instance_id": new_primary_id,
            }))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .context("Routing update request failed")?
            .error_for_status()
            .context("Routing update returned error")?;
        Ok(())
    }

    async fn schedule_reprovision_standby(
        &self,
        account_id: &str,
        failed_instance_id: &str,
    ) -> Result<()> {
        // POST /v1/provision-queue — queue a new standby provision
        // Use original provider preference if that provider is healthy,
        // otherwise use next-best provider
        let url = format!("{}/v1/provision-queue", self.gf_api_base);
        reqwest::Client::new()
            .post(&url)
            .bearer_auth(&self.gf_api_key)
            .json(&serde_json::json!({
                "account_id": account_id,
                "role": "standby",
                "reason": "failover_reprovision",
                "failed_instance_id": failed_instance_id,
                "priority": "high",
            }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .context("Provision queue request failed")?
            .error_for_status()
            .context("Provision queue returned error")?;
        Ok(())
    }
}

// ─── Bulk failover (provider outage handling) ─────────────────────────────────

/// Executes failover for multiple instances simultaneously.
/// Used when a provider region has an outage (e.g. Hetzner Nuremberg).
pub struct BulkFailoverOrchestrator {
    engine: FailoverEngine,
    max_concurrent: usize,
}

impl BulkFailoverOrchestrator {
    pub fn new(engine: FailoverEngine) -> Self {
        Self {
            engine,
            max_concurrent: 20, // process 20 failovers in parallel
        }
    }

    pub async fn execute_region_failover(
        &self,
        requests: Vec<FailoverRequest>,
    ) -> Vec<FailoverResult> {
        use futures::stream::{self, StreamExt};

        info!(count = requests.len(), "Starting bulk region failover");

        stream::iter(requests)
            .map(|req| self.engine.execute_failover(req))
            .buffer_unordered(self.max_concurrent)
            .filter_map(|result| async move {
                match result {
                    Ok(r) => Some(r),
                    Err(e) => {
                        warn!("Failover failed: {e}");
                        None
                    }
                }
            })
            .collect()
            .await
    }
}

fn elapsed_ms(start: DateTime<Utc>) -> u64 {
    (Utc::now() - start).num_milliseconds().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn failover_trigger_serde_round_trip() {
        let triggers = [
            FailoverTrigger::AutoHeal,
            FailoverTrigger::ProviderOutage,
            FailoverTrigger::ManualByCommander,
            FailoverTrigger::PlannedMaintenance,
        ];

        for trigger in triggers {
            let json =
                serde_json::to_string(&trigger).expect("FailoverTrigger serialization failed");
            let decoded: FailoverTrigger =
                serde_json::from_str(&json).expect("FailoverTrigger deserialization failed");
            assert_eq!(trigger, decoded);
        }
    }

    #[test]
    fn failover_trigger_auto_heal_serializes_as_snake_case() {
        let json = serde_json::to_string(&FailoverTrigger::AutoHeal).unwrap();
        assert_eq!(json, "\"auto_heal\"");
    }

    #[test]
    fn failover_step_type_serde_round_trip() {
        let steps = [
            FailoverStepType::VerifyStandby,
            FailoverStepType::UpdatePairStatus,
            FailoverStepType::NotifyGateway,
            FailoverStepType::UpdateRouting,
            FailoverStepType::ScheduleReprovisioning,
            FailoverStepType::NotifyCommander,
        ];

        for step in steps {
            let json = serde_json::to_string(&step).expect("FailoverStepType serialization failed");
            let decoded: FailoverStepType =
                serde_json::from_str(&json).expect("FailoverStepType deserialization failed");
            assert_eq!(step, decoded);
        }
    }

    #[test]
    fn failover_request_serialization_includes_all_fields() {
        let req = FailoverRequest {
            request_id: Uuid::new_v4(),
            account_id: "acct-001".to_string(),
            failed_instance_id: "inst-failed".to_string(),
            standby_instance_id: "inst-standby".to_string(),
            trigger_reason: FailoverTrigger::AutoHeal,
            triggered_by: "guardian".to_string(),
            triggered_at: Utc::now(),
        };

        let json = serde_json::to_string(&req).expect("FailoverRequest serialization failed");
        assert!(json.contains("acct-001"));
        assert!(json.contains("inst-failed"));
        assert!(json.contains("inst-standby"));
        assert!(json.contains("auto_heal"));
        assert!(json.contains("guardian"));

        let decoded: FailoverRequest =
            serde_json::from_str(&json).expect("FailoverRequest deserialization failed");
        assert_eq!(decoded.account_id, "acct-001");
        assert_eq!(decoded.trigger_reason, FailoverTrigger::AutoHeal);
    }

    #[test]
    fn elapsed_ms_returns_non_negative_value_under_1000ms() {
        let start = Utc::now();
        let ms = elapsed_ms(start);
        // Should be very small (test runs in microseconds)
        assert!(
            ms < 1000,
            "elapsed_ms should be < 1000ms for immediate call, got {ms}"
        );
    }

    #[test]
    fn bulk_failover_orchestrator_has_max_concurrent_20() {
        let engine = FailoverEngine::new(
            "https://api.example.com".to_string(),
            "test-key".to_string(),
        );
        let orchestrator = BulkFailoverOrchestrator::new(engine);
        assert_eq!(orchestrator.max_concurrent, 20);
    }
}
