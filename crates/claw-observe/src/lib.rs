//! Structured observability for ClawOps fleet operations.
//!
//! Provides:
//! - [`OperationsMetrics`] — atomic counters for all key operations
//! - [`MetricsExporter`] — Prometheus text format export
//! - [`AuditLogger`] — structured JSON logging of fleet operations

#![forbid(unsafe_code)]

use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{error, info, warn};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────
// Atomic Counter
// ─────────────────────────────────────────────────────────────

/// A thread-safe u64 counter backed by an atomic.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Increment the counter by one.
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Read the current counter value.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────
// Operations Metrics
// ─────────────────────────────────────────────────────────────

/// Atomic operation counters for key ClawOps fleet operations.
///
/// All counters are thread-safe and can be shared via [`Arc`].
///
/// # Example
/// ```rust
/// # use claw_observe::OperationsMetrics;
/// # use std::sync::Arc;
/// let metrics = Arc::new(OperationsMetrics::new());
/// metrics.provision_total.inc();
/// assert_eq!(metrics.provision_total.get(), 1);
/// ```
#[derive(Debug, Default)]
pub struct OperationsMetrics {
    /// Total VPS provision attempts (success + failure).
    pub provision_total: Counter,
    /// Total failed provision attempts.
    pub provision_errors: Counter,
    /// Total health check cycles completed.
    pub health_checks_total: Counter,
    /// Total auto-heal sequences initiated.
    pub heals_attempted: Counter,
    /// Total successful auto-heal sequences.
    pub heals_succeeded: Counter,
    /// Total failover sequences triggered.
    pub failovers_triggered: Counter,
    /// Total config push operations.
    pub config_pushes_total: Counter,
    /// Total config push failures.
    pub config_push_errors: Counter,
    /// Total cost report generations.
    pub cost_reports_total: Counter,
    /// Total incident reports generated.
    pub incident_reports_total: Counter,
    /// Total provider API calls made.
    pub provider_api_calls_total: Counter,
    /// Total provider API errors (including timeouts).
    pub provider_api_errors: Counter,
}

impl OperationsMetrics {
    /// Create a new zeroed metrics instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a provision attempt. Call before attempting.
    pub fn record_provision_attempt(&self) {
        self.provision_total.inc();
        tracing::info!(
            counter = "provision_total",
            value = self.provision_total.get(),
            "provision attempt"
        );
    }

    /// Record a failed provision.
    pub fn record_provision_error(&self) {
        self.provision_errors.inc();
        tracing::warn!(
            counter = "provision_errors",
            value = self.provision_errors.get(),
            "provision failed"
        );
    }

    /// Record a completed health check.
    pub fn record_health_check(&self) {
        self.health_checks_total.inc();
    }

    /// Record an auto-heal attempt.
    pub fn record_heal_attempt(&self) {
        self.heals_attempted.inc();
        info!(
            counter = "heals_attempted",
            value = self.heals_attempted.get(),
            "heal attempt"
        );
    }

    /// Record a successful auto-heal.
    pub fn record_heal_success(&self) {
        self.heals_succeeded.inc();
        info!(
            counter = "heals_succeeded",
            value = self.heals_succeeded.get(),
            "heal succeeded"
        );
    }

    /// Record a failover trigger.
    pub fn record_failover(&self) {
        self.failovers_triggered.inc();
        warn!(
            counter = "failovers_triggered",
            value = self.failovers_triggered.get(),
            "failover triggered"
        );
    }

    /// Record a config push attempt.
    pub fn record_config_push(&self) {
        self.config_pushes_total.inc();
    }

    /// Record a config push failure.
    pub fn record_config_push_error(&self) {
        self.config_push_errors.inc();
        error!(
            counter = "config_push_errors",
            value = self.config_push_errors.get(),
            "config push failed"
        );
    }

    /// Record a provider API call.
    pub fn record_provider_call(&self) {
        self.provider_api_calls_total.inc();
    }

    /// Record a provider API error.
    pub fn record_provider_error(&self) {
        self.provider_api_errors.inc();
        warn!(
            counter = "provider_api_errors",
            value = self.provider_api_errors.get(),
            "provider API error"
        );
    }
}

// ─────────────────────────────────────────────────────────────
// Metrics Exporter (Prometheus text format)
// ─────────────────────────────────────────────────────────────

/// Exports [`OperationsMetrics`] in Prometheus text format.
pub struct MetricsExporter {
    metrics: Arc<OperationsMetrics>,
    /// Label prefix added to all metric names (default: `clawops`).
    prefix: String,
}

impl MetricsExporter {
    /// Create a new exporter wrapping the given metrics.
    pub fn new(metrics: Arc<OperationsMetrics>) -> Self {
        Self {
            metrics,
            prefix: "clawops".to_string(),
        }
    }

    /// Create with a custom metric name prefix.
    pub fn with_prefix(metrics: Arc<OperationsMetrics>, prefix: impl Into<String>) -> Self {
        Self {
            metrics,
            prefix: prefix.into(),
        }
    }

    /// Render all metrics as a Prometheus text format string.
    ///
    /// Each metric is rendered with `# HELP`, `# TYPE`, and value lines.
    pub fn render(&self) -> String {
        let m = &self.metrics;
        let p = &self.prefix;
        let mut out = String::new();

        self.write_counter(
            &mut out,
            p,
            "provision_total",
            "Total VPS provision attempts",
            m.provision_total.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "provision_errors",
            "Total failed VPS provision attempts",
            m.provision_errors.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "health_checks_total",
            "Total health check cycles completed",
            m.health_checks_total.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "heals_attempted",
            "Total auto-heal sequences initiated",
            m.heals_attempted.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "heals_succeeded",
            "Total successful auto-heal sequences",
            m.heals_succeeded.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "failovers_triggered",
            "Total failover sequences triggered",
            m.failovers_triggered.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "config_pushes_total",
            "Total config push operations",
            m.config_pushes_total.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "config_push_errors",
            "Total config push failures",
            m.config_push_errors.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "cost_reports_total",
            "Total cost report generations",
            m.cost_reports_total.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "incident_reports_total",
            "Total incident reports generated",
            m.incident_reports_total.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "provider_api_calls_total",
            "Total provider API calls made",
            m.provider_api_calls_total.get(),
        );
        self.write_counter(
            &mut out,
            p,
            "provider_api_errors",
            "Total provider API errors including timeouts",
            m.provider_api_errors.get(),
        );

        out
    }

    fn write_counter(&self, out: &mut String, prefix: &str, name: &str, help: &str, value: u64) {
        out.push_str(&format!("# HELP {prefix}_{name} {help}\n"));
        out.push_str(&format!("# TYPE {prefix}_{name} counter\n"));
        out.push_str(&format!("{prefix}_{name} {value}\n\n"));
    }
}

// ─────────────────────────────────────────────────────────────
// Audit Logger
// ─────────────────────────────────────────────────────────────

/// Category of fleet operation being logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// VPS instance provisioning.
    Provision,
    /// VPS teardown (intentional).
    Teardown,
    /// Auto-heal sequence.
    AutoHeal,
    /// Failover sequence.
    Failover,
    /// Configuration push.
    ConfigPush,
    /// Fleet health check sweep.
    HealthCheck,
    /// Cost analysis.
    CostAnalysis,
    /// Incident report generation.
    IncidentReport,
    /// Operator action (manual override).
    OperatorAction,
}

/// Outcome of a logged operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    /// Completed successfully.
    Success,
    /// Failed with an error.
    Failure,
    /// Blocked by a safety constraint.
    BlockedBySafety,
    /// Requires operator confirmation.
    PendingConfirmation,
}

/// A single structured audit log entry for a fleet operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetAuditEntry {
    /// Unique entry ID.
    pub id: String,
    /// Timestamp when the operation occurred.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Agent or system that initiated the operation.
    pub actor: String,
    /// Operation category.
    pub kind: OperationKind,
    /// Target resource (instance ID, config name, etc.).
    pub resource_id: Option<String>,
    /// Operation outcome.
    pub outcome: OperationOutcome,
    /// Duration of the operation in milliseconds.
    pub duration_ms: Option<u64>,
    /// Additional structured details (provider, region, tier, etc.).
    pub details: HashMap<String, String>,
}

/// Structured JSON audit logger for fleet operations.
///
/// Maintains an in-memory log with thread-safe access. Emits structured
/// tracing events for each logged entry.
pub struct AuditLogger {
    entries: RwLock<Vec<FleetAuditEntry>>,
    /// Maximum number of entries to retain in memory.
    max_entries: usize,
}

impl AuditLogger {
    /// Create a new audit logger retaining up to `max_entries` in memory.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries,
        }
    }

    /// Create with default capacity (10,000 entries).
    pub fn default_capacity() -> Self {
        Self::new(10_000)
    }

    /// Log a fleet operation.
    pub fn log(
        &self,
        actor: impl Into<String>,
        kind: OperationKind,
        resource_id: Option<&str>,
        outcome: OperationOutcome,
        duration_ms: Option<u64>,
        details: HashMap<String, String>,
    ) {
        let entry = FleetAuditEntry {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            actor: actor.into(),
            kind,
            resource_id: resource_id.map(String::from),
            outcome,
            duration_ms,
            details,
        };

        // Emit structured tracing event
        let details_json = serde_json::to_string(&entry.details).unwrap_or_default();
        match outcome {
            OperationOutcome::Success => {
                info!(
                    audit_id = %entry.id,
                    actor = %entry.actor,
                    kind = ?entry.kind,
                    resource_id = ?entry.resource_id,
                    duration_ms = ?entry.duration_ms,
                    details = %details_json,
                    "fleet operation succeeded"
                );
            }
            OperationOutcome::Failure => {
                error!(
                    audit_id = %entry.id,
                    actor = %entry.actor,
                    kind = ?entry.kind,
                    resource_id = ?entry.resource_id,
                    details = %details_json,
                    "fleet operation failed"
                );
            }
            OperationOutcome::BlockedBySafety => {
                warn!(
                    audit_id = %entry.id,
                    actor = %entry.actor,
                    kind = ?entry.kind,
                    resource_id = ?entry.resource_id,
                    details = %details_json,
                    "fleet operation blocked by safety constraint"
                );
            }
            OperationOutcome::PendingConfirmation => {
                info!(
                    audit_id = %entry.id,
                    actor = %entry.actor,
                    kind = ?entry.kind,
                    resource_id = ?entry.resource_id,
                    details = %details_json,
                    "fleet operation pending operator confirmation"
                );
            }
        }

        let mut entries = self.entries.write();
        entries.push(entry);
        // Evict oldest entries if over capacity
        if entries.len() > self.max_entries {
            let excess = entries.len() - self.max_entries;
            entries.drain(0..excess);
        }
    }

    /// Query entries filtered by kind and/or actor.
    pub fn query(
        &self,
        kind: Option<OperationKind>,
        actor: Option<&str>,
        limit: usize,
    ) -> Vec<FleetAuditEntry> {
        let entries = self.entries.read();
        entries
            .iter()
            .filter(|e| kind.is_none_or(|k| e.kind == k))
            .filter(|e| actor.is_none_or(|a| e.actor == a))
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return all entries as a JSON array string.
    pub fn to_json(&self) -> String {
        let entries = self.entries.read();
        serde_json::to_string_pretty(&*entries).unwrap_or_else(|_| "[]".to_string())
    }

    /// Total number of entries logged.
    pub fn count(&self) -> usize {
        self.entries.read().len()
    }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operations_metrics_counters() {
        let m = OperationsMetrics::new();
        assert_eq!(m.provision_total.get(), 0);

        m.record_provision_attempt();
        m.record_provision_attempt();
        assert_eq!(m.provision_total.get(), 2);

        m.record_provision_error();
        assert_eq!(m.provision_errors.get(), 1);

        m.record_heal_attempt();
        m.record_heal_success();
        assert_eq!(m.heals_attempted.get(), 1);
        assert_eq!(m.heals_succeeded.get(), 1);

        m.record_failover();
        assert_eq!(m.failovers_triggered.get(), 1);
    }

    #[test]
    fn test_metrics_exporter_prometheus_format() {
        let metrics = Arc::new(OperationsMetrics::new());
        metrics.record_provision_attempt();
        metrics.record_provision_attempt();
        metrics.record_provision_error();

        let exporter = MetricsExporter::new(metrics);
        let output = exporter.render();

        assert!(
            output.contains("# HELP clawops_provision_total"),
            "must have HELP line"
        );
        assert!(
            output.contains("# TYPE clawops_provision_total counter"),
            "must have TYPE line"
        );
        assert!(
            output.contains("clawops_provision_total 2"),
            "must have correct count"
        );
        assert!(
            output.contains("clawops_provision_errors 1"),
            "must have error count"
        );
        assert!(
            output.contains("clawops_failovers_triggered 0"),
            "zero counters must appear"
        );
    }

    #[test]
    fn test_metrics_exporter_custom_prefix() {
        let metrics = Arc::new(OperationsMetrics::new());
        let exporter = MetricsExporter::with_prefix(metrics, "myapp");
        let output = exporter.render();
        assert!(
            output.contains("myapp_provision_total"),
            "custom prefix must be used"
        );
        assert!(
            !output.contains("clawops_provision_total"),
            "default prefix must not appear"
        );
    }

    #[test]
    fn test_audit_logger_log_and_query() {
        let logger = AuditLogger::new(100);

        logger.log(
            "commander",
            OperationKind::Provision,
            Some("i-test-1"),
            OperationOutcome::Success,
            Some(1500),
            HashMap::from([("provider".to_string(), "hetzner".to_string())]),
        );

        logger.log(
            "forge",
            OperationKind::Provision,
            Some("i-test-2"),
            OperationOutcome::Failure,
            Some(300),
            HashMap::new(),
        );

        assert_eq!(logger.count(), 2);

        let all = logger.query(None, None, 10);
        assert_eq!(all.len(), 2);

        let successes = logger.query(None, Some("commander"), 10);
        assert_eq!(successes.len(), 1);
        assert_eq!(successes[0].outcome, OperationOutcome::Success);

        let provisions = logger.query(Some(OperationKind::Provision), None, 10);
        assert_eq!(provisions.len(), 2);
    }

    #[test]
    fn test_audit_logger_json_output() {
        let logger = AuditLogger::new(100);
        logger.log(
            "guardian",
            OperationKind::HealthCheck,
            None,
            OperationOutcome::Success,
            Some(50),
            HashMap::new(),
        );

        let json = logger.to_json();
        assert!(
            json.contains("health_check"),
            "JSON must contain operation kind"
        );
        assert!(json.contains("success"), "JSON must contain outcome");
        assert!(json.contains("guardian"), "JSON must contain actor");
    }

    #[test]
    fn test_audit_logger_evicts_old_entries() {
        let logger = AuditLogger::new(5); // tiny capacity

        for i in 0..10 {
            logger.log(
                "system",
                OperationKind::HealthCheck,
                Some(&format!("i-{i}")),
                OperationOutcome::Success,
                None,
                HashMap::new(),
            );
        }

        // Should retain only the most recent 5
        assert_eq!(
            logger.count(),
            5,
            "logger must evict old entries over max_entries"
        );
    }

    #[test]
    fn test_audit_logger_blocked_by_safety_logged() {
        let logger = AuditLogger::new(100);
        logger.log(
            "forge",
            OperationKind::Teardown,
            Some("i-primary-1"),
            OperationOutcome::BlockedBySafety,
            None,
            HashMap::from([("reason".to_string(), "standby not active".to_string())]),
        );

        let blocked = logger.query(None, None, 1);
        assert_eq!(blocked[0].outcome, OperationOutcome::BlockedBySafety);
        assert_eq!(blocked[0].details["reason"], "standby not active");
    }
}
