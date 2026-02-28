//! Rolling config push — batch-by-batch deployment with per-batch validation.
//!
//! Implements the PRD §4.1 rolling push pattern:
//! "Rolling push started. Batch 1/17 (50 instances): 49 applied,
//!  1 config validation error (instance af3c-uuid — malformed existing config)."
//!
//! Safety rule: never push config to > 100 instances all-at-once; always
//! validate each batch before proceeding to the next.

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── Types ─────────────────────────────────────────────────────────────────

/// A target instance for config push.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub instance_id: String,
    pub account_id: String,
}

/// Outcome of pushing config to a single instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancePushResult {
    pub instance_id: String,
    pub success: bool,
    pub error: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
}

/// Outcome of one batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub batch_number: usize,
    pub total_batches: usize,
    pub instance_results: Vec<InstancePushResult>,
    pub validation_passed: bool,
    pub completed_at: DateTime<Utc>,
}

impl BatchResult {
    pub fn success_count(&self) -> usize {
        self.instance_results.iter().filter(|r| r.success).count()
    }

    pub fn failure_count(&self) -> usize {
        self.instance_results.iter().filter(|r| !r.success).count()
    }

    /// Human-readable progress line (matches PRD pattern).
    pub fn progress_line(&self) -> String {
        format!(
            "Batch {}/{} ({} instances): {} applied, {} config validation error(s).",
            self.batch_number,
            self.total_batches,
            self.instance_results.len(),
            self.success_count(),
            self.failure_count(),
        )
    }
}

/// Validation result for a batch after push.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchValidation {
    pub batch_number: usize,
    pub instances_checked: usize,
    pub instances_valid: usize,
    pub instances_invalid: usize,
    pub errors: Vec<ValidationError>,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    pub instance_id: String,
    pub error: String,
}

/// Full result of a rolling push across all batches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollingPushResult {
    pub config_name: String,
    pub total_instances: usize,
    pub batch_size: usize,
    pub total_batches: usize,
    pub batch_results: Vec<BatchResult>,
    pub rollbacks_triggered: usize,
    pub overall_success: bool,
    pub completed_at: DateTime<Utc>,
}

impl RollingPushResult {
    pub fn total_applied(&self) -> usize {
        self.batch_results.iter().map(|b| b.success_count()).sum()
    }

    pub fn total_failed(&self) -> usize {
        self.batch_results.iter().map(|b| b.failure_count()).sum()
    }

    /// Summary line for Commander.
    pub fn summary(&self) -> String {
        format!(
            "Rolling push complete: {}/{} applied, {} failed, {} rollbacks. {}",
            self.total_applied(),
            self.total_instances,
            self.total_failed(),
            self.rollbacks_triggered,
            if self.overall_success {
                "SUCCESS."
            } else {
                "PARTIAL — manual review required."
            }
        )
    }
}

// ─── Rolling Push Engine ─────────────────────────────────────────────────────

/// Configuration for a rolling push operation.
#[derive(Debug, Clone)]
pub struct RollingPush {
    pub config_name: String,
    pub config_payload: Value,
    pub instances: Vec<Instance>,
    pub batch_size: usize,
    pub stop_on_validation_failure: bool,
}

impl RollingPush {
    pub fn new(
        config_name: impl Into<String>,
        config_payload: Value,
        instances: Vec<Instance>,
        batch_size: usize,
    ) -> Self {
        Self {
            config_name: config_name.into(),
            config_payload,
            instances,
            batch_size: batch_size.max(1),
            stop_on_validation_failure: true,
        }
    }

    /// Total batch count.
    pub fn total_batches(&self) -> usize {
        self.instances.len().div_ceil(self.batch_size)
    }

    /// Execute the rolling push using the provided push and validate callbacks.
    ///
    /// `push_fn`     — called with a batch; returns per-instance results.
    /// `validate_fn` — called after each batch push; returns validation result.
    /// `rollback_fn` — called if validation fails.
    pub fn execute<P, V, R>(
        &self,
        mut push_fn: P,
        mut validate_fn: V,
        mut rollback_fn: R,
    ) -> RollingPushResult
    where
        P: FnMut(&[Instance], &Value) -> Vec<InstancePushResult>,
        V: FnMut(&[Instance]) -> BatchValidation,
        R: FnMut(&[Instance], &Value),
    {
        let total_batches = self.total_batches();
        let mut batch_results: Vec<BatchResult> = Vec::new();
        let mut rollbacks_triggered = 0usize;
        let mut overall_success = true;

        for (batch_idx, chunk) in self.instances.chunks(self.batch_size).enumerate() {
            let batch_number = batch_idx + 1;

            // Push config to this batch
            let instance_results = push_fn(chunk, &self.config_payload);

            // Validate the batch
            let validation = validate_fn(chunk);
            let validation_passed = validation.passed;

            let batch_result = BatchResult {
                batch_number,
                total_batches,
                instance_results,
                validation_passed,
                completed_at: Utc::now(),
            };

            batch_results.push(batch_result);

            if !validation_passed {
                overall_success = false;
                rollback_fn(chunk, &self.config_payload);
                rollbacks_triggered += 1;

                if self.stop_on_validation_failure {
                    break;
                }
            }
        }

        RollingPushResult {
            config_name: self.config_name.clone(),
            total_instances: self.instances.len(),
            batch_size: self.batch_size,
            total_batches,
            batch_results,
            rollbacks_triggered,
            overall_success,
            completed_at: Utc::now(),
        }
    }
}

/// Validate a batch of instances after a config push.
///
/// In production this calls `config.get` on each instance and checks
/// the config version/checksum.  Here we provide the pure validation logic.
pub fn validate_batch(
    batch: &[Instance],
    expected_config: &Value,
    actual_configs: &[Option<Value>],
) -> BatchValidation {
    let mut errors = Vec::new();

    for (instance, actual) in batch.iter().zip(actual_configs.iter()) {
        match actual {
            None => errors.push(ValidationError {
                instance_id: instance.instance_id.clone(),
                error: "config not found after push".to_string(),
            }),
            Some(cfg) => {
                if cfg != expected_config {
                    errors.push(ValidationError {
                        instance_id: instance.instance_id.clone(),
                        error: "config mismatch — malformed existing config".to_string(),
                    });
                }
            }
        }
    }

    let instances_invalid = errors.len();
    let instances_valid = batch.len().saturating_sub(instances_invalid);

    BatchValidation {
        batch_number: 0, // set by caller
        instances_checked: batch.len(),
        instances_valid,
        instances_invalid,
        errors,
        passed: instances_invalid == 0,
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_instances(n: usize) -> Vec<Instance> {
        (0..n)
            .map(|i| Instance {
                instance_id: format!("i-{:04}", i),
                account_id: format!("acc-{}", i),
            })
            .collect()
    }

    fn always_succeed(batch: &[Instance], _cfg: &Value) -> Vec<InstancePushResult> {
        batch
            .iter()
            .map(|i| InstancePushResult {
                instance_id: i.instance_id.clone(),
                success: true,
                error: None,
                applied_at: Some(Utc::now()),
            })
            .collect()
    }

    fn always_validate_pass(batch: &[Instance]) -> BatchValidation {
        BatchValidation {
            batch_number: 0,
            instances_checked: batch.len(),
            instances_valid: batch.len(),
            instances_invalid: 0,
            errors: vec![],
            passed: true,
        }
    }

    fn never_rollback(_batch: &[Instance], _cfg: &Value) {}

    // ─── RollingPush ──────────────────────────────────────────────────────

    #[test]
    fn test_total_batches_exact() {
        let rp = RollingPush::new("cfg-v1", json!({}), make_instances(100), 50);
        assert_eq!(rp.total_batches(), 2);
    }

    #[test]
    fn test_total_batches_remainder() {
        let rp = RollingPush::new("cfg-v1", json!({}), make_instances(101), 50);
        assert_eq!(rp.total_batches(), 3);
    }

    #[test]
    fn test_execute_all_succeed() {
        let rp = RollingPush::new(
            "cfg-v1",
            json!({ "model": "kimi-k2.5" }),
            make_instances(100),
            50,
        );

        let result = rp.execute(always_succeed, always_validate_pass, never_rollback);

        assert_eq!(result.total_batches, 2);
        assert_eq!(result.batch_results.len(), 2);
        assert_eq!(result.total_applied(), 100);
        assert_eq!(result.total_failed(), 0);
        assert_eq!(result.rollbacks_triggered, 0);
        assert!(result.overall_success);
    }

    #[test]
    fn test_execute_batch_failure_stops() {
        let rp = RollingPush::new("cfg-v1", json!({}), make_instances(100), 50);

        let mut batch_called = 0usize;

        let result = rp.execute(
            |batch, cfg| always_succeed(batch, cfg),
            |batch| {
                batch_called += 1;
                if batch_called == 1 {
                    // First batch validation fails
                    BatchValidation {
                        batch_number: 0,
                        instances_checked: batch.len(),
                        instances_valid: batch.len() - 1,
                        instances_invalid: 1,
                        errors: vec![ValidationError {
                            instance_id: "i-0000".to_string(),
                            error: "malformed config".to_string(),
                        }],
                        passed: false,
                    }
                } else {
                    always_validate_pass(batch)
                }
            },
            |_batch, _cfg| {},
        );

        // Stop on first failure — only 1 batch executed
        assert_eq!(result.batch_results.len(), 1);
        assert_eq!(result.rollbacks_triggered, 1);
        assert!(!result.overall_success);
    }

    #[test]
    fn test_execute_single_batch() {
        let rp = RollingPush::new("cfg-v1", json!({}), make_instances(10), 50);
        let result = rp.execute(always_succeed, always_validate_pass, never_rollback);
        assert_eq!(result.total_batches, 1);
        assert_eq!(result.total_applied(), 10);
    }

    #[test]
    fn test_execute_empty_instances() {
        let rp = RollingPush::new("cfg-v1", json!({}), vec![], 50);
        let result = rp.execute(always_succeed, always_validate_pass, never_rollback);
        assert_eq!(result.total_applied(), 0);
        assert!(result.overall_success);
    }

    // ─── BatchResult helpers ─────────────────────────────────────────────

    #[test]
    fn test_batch_result_progress_line() {
        let br = BatchResult {
            batch_number: 1,
            total_batches: 17,
            instance_results: vec![
                InstancePushResult {
                    instance_id: "i-0".to_string(),
                    success: true,
                    error: None,
                    applied_at: Some(Utc::now()),
                },
                InstancePushResult {
                    instance_id: "i-1".to_string(),
                    success: false,
                    error: Some("malformed config".to_string()),
                    applied_at: None,
                },
            ],
            validation_passed: false,
            completed_at: Utc::now(),
        };

        let line = br.progress_line();
        assert!(line.contains("Batch 1/17"));
        assert!(line.contains("1 applied"));
        assert!(line.contains("1 config validation error"));
    }

    #[test]
    fn test_rolling_push_result_summary_success() {
        let result = RollingPushResult {
            config_name: "cfg-v47".to_string(),
            total_instances: 847,
            batch_size: 50,
            total_batches: 17,
            batch_results: vec![],
            rollbacks_triggered: 0,
            overall_success: true,
            completed_at: Utc::now(),
        };
        let s = result.summary();
        assert!(s.contains("SUCCESS"));
        assert!(s.contains("0/847"));
    }

    #[test]
    fn test_rolling_push_result_summary_partial() {
        let result = RollingPushResult {
            config_name: "cfg-v47".to_string(),
            total_instances: 100,
            batch_size: 50,
            total_batches: 2,
            batch_results: vec![],
            rollbacks_triggered: 1,
            overall_success: false,
            completed_at: Utc::now(),
        };
        let s = result.summary();
        assert!(s.contains("PARTIAL"));
        assert!(s.contains("1 rollbacks"));
    }

    // ─── validate_batch ──────────────────────────────────────────────────

    #[test]
    fn test_validate_batch_all_match() {
        let instances = make_instances(3);
        let cfg = json!({ "model": "kimi-k2.5" });
        let actuals: Vec<Option<Value>> =
            vec![Some(cfg.clone()), Some(cfg.clone()), Some(cfg.clone())];
        let validation = validate_batch(&instances, &cfg, &actuals);
        assert!(validation.passed);
        assert_eq!(validation.instances_valid, 3);
        assert_eq!(validation.instances_invalid, 0);
    }

    #[test]
    fn test_validate_batch_one_mismatch() {
        let instances = make_instances(3);
        let cfg = json!({ "model": "kimi-k2.5" });
        let actuals: Vec<Option<Value>> = vec![
            Some(cfg.clone()),
            Some(json!({ "model": "old-model" })), // mismatch
            Some(cfg.clone()),
        ];
        let validation = validate_batch(&instances, &cfg, &actuals);
        assert!(!validation.passed);
        assert_eq!(validation.instances_invalid, 1);
        assert_eq!(validation.errors[0].instance_id, "i-0001");
    }

    #[test]
    fn test_validate_batch_missing_config() {
        let instances = make_instances(2);
        let cfg = json!({});
        let actuals: Vec<Option<Value>> = vec![Some(cfg.clone()), None];
        let validation = validate_batch(&instances, &cfg, &actuals);
        assert!(!validation.passed);
        assert_eq!(validation.instances_invalid, 1);
        assert!(validation.errors[0].error.contains("not found"));
    }

    #[test]
    fn test_batch_size_minimum_one() {
        let rp = RollingPush::new("cfg", json!({}), make_instances(5), 0);
        assert_eq!(rp.batch_size, 1);
    }
}
