//! Incident management, severity classification, and structured reporting.
//!
//! The Triage agent is spawned on-demand and feeds structured incident reports
//! back to the Commander.  Matches PRD §4.1 incident response pattern:
//! "Triage report: 94 primary instances in Nuremberg. Guardian confirms 87
//!  have failover complete..."

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use claw_proto::{HealthCheck, VpsProvider};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Severity ─────────────────────────────────────────────────────────────────

/// PRD-defined severity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    /// Cosmetic / no user impact.
    P4,
    /// Single instance affected.
    P3,
    /// > 10 users affected.
    P2,
    /// > 50 users affected or data-loss risk.
    P1,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::P1 => write!(f, "P1"),
            Self::P2 => write!(f, "P2"),
            Self::P3 => write!(f, "P3"),
            Self::P4 => write!(f, "P4"),
        }
    }
}

/// Classify severity from affected user count and data-loss risk.
///
/// PRD rule:
/// - P1: > 50 users OR data-loss risk
/// - P2: > 10 users
/// - P3: single instance (1-10 users)
/// - P4: cosmetic / zero users
pub fn classify_severity(affected_users: u32, data_loss_risk: bool) -> Severity {
    if data_loss_risk || affected_users > 50 {
        Severity::P1
    } else if affected_users > 10 {
        Severity::P2
    } else if affected_users >= 1 {
        Severity::P3
    } else {
        Severity::P4
    }
}

// ─── Incident status ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    Open,
    Investigating,
    Mitigated,
    Resolved,
}

// ─── Timeline ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub timestamp: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub outcome: String,
}

// ─── Root cause ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootCauseCategory {
    ProviderOutage,
    HardwareFailure,
    NetworkIssue,
    ConfigurationError,
    ResourceExhaustion,
    SoftwareBug,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootCause {
    pub category: RootCauseCategory,
    pub description: String,
    pub confidence: RootCauseConfidence,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RootCauseConfidence {
    High,
    Medium,
    Low,
}

// ─── Health event trigger ─────────────────────────────────────────────────────

/// The event that triggers incident creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthEvent {
    pub instance_id: String,
    pub account_id: String,
    pub health_score: u8,
    pub provider: VpsProvider,
    pub region: String,
    pub description: String,
    pub affected_users: u32,
    pub data_loss_risk: bool,
    pub detected_at: DateTime<Utc>,
}

// ─── Incident ─────────────────────────────────────────────────────────────────

/// A fully structured incident record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Incident {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub description: String,
    pub status: IncidentStatus,
    pub affected_instances: Vec<String>,
    pub affected_users: u32,
    pub provider: VpsProvider,
    pub region: String,
    pub timeline: Vec<TimelineEntry>,
    pub root_cause: Option<RootCause>,
    pub actions_taken: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl Incident {
    /// Duration from creation to now (or to resolution).
    pub fn duration_mins(&self) -> u64 {
        let end = self.resolved_at.unwrap_or_else(Utc::now);
        let diff = end.signed_duration_since(self.created_at);
        diff.num_minutes().unsigned_abs()
    }
}

// ─── Incident report ──────────────────────────────────────────────────────────

/// Formatted report ready for Commander synthesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentReport {
    pub incident_id: String,
    pub severity: Severity,
    pub title: String,
    pub status: IncidentStatus,
    pub summary: String,
    pub timeline_entries: Vec<TimelineEntry>,
    pub root_cause: Option<RootCause>,
    pub actions_taken: Vec<String>,
    pub recommended_next_steps: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

// ─── Incident Manager ─────────────────────────────────────────────────────────

/// The Triage agent's incident lifecycle manager.
pub struct IncidentManager {
    incidents: Vec<Incident>,
}

impl IncidentManager {
    pub fn new() -> Self {
        Self {
            incidents: Vec::new(),
        }
    }

    /// Create a new incident from a health event.
    pub fn create_incident(&mut self, trigger: HealthEvent) -> &Incident {
        let severity = classify_severity(trigger.affected_users, trigger.data_loss_risk);
        let id = Uuid::new_v4().to_string();

        let title = build_incident_title(&trigger, severity);

        let initial_entry = TimelineEntry {
            timestamp: trigger.detected_at,
            actor: "guardian".to_string(),
            action: "detected".to_string(),
            outcome: format!(
                "Health event detected: {} — score {}",
                trigger.description, trigger.health_score
            ),
        };

        let incident = Incident {
            id: id.clone(),
            severity,
            title,
            description: trigger.description,
            status: IncidentStatus::Open,
            affected_instances: vec![trigger.instance_id],
            affected_users: trigger.affected_users,
            provider: trigger.provider,
            region: trigger.region,
            timeline: vec![initial_entry],
            root_cause: None,
            actions_taken: Vec::new(),
            created_at: trigger.detected_at,
            resolved_at: None,
        };

        self.incidents.push(incident);
        self.incidents.last().unwrap()
    }

    /// Add a timeline entry to an existing incident.
    pub fn add_timeline_entry(
        &mut self,
        incident_id: &str,
        entry: TimelineEntry,
    ) -> Result<(), String> {
        match self.incidents.iter_mut().find(|i| i.id == incident_id) {
            Some(inc) => {
                inc.timeline.push(entry);
                Ok(())
            }
            None => Err(format!("Incident {} not found", incident_id)),
        }
    }

    /// Update incident status.
    pub fn update_status(
        &mut self,
        incident_id: &str,
        status: IncidentStatus,
    ) -> Result<(), String> {
        match self.incidents.iter_mut().find(|i| i.id == incident_id) {
            Some(inc) => {
                inc.status = status;
                if status == IncidentStatus::Resolved {
                    inc.resolved_at = Some(Utc::now());
                }
                Ok(())
            }
            None => Err(format!("Incident {} not found", incident_id)),
        }
    }

    /// Add an instance to the affected list.
    pub fn add_affected_instance(
        &mut self,
        incident_id: &str,
        instance_id: String,
    ) -> Result<(), String> {
        match self.incidents.iter_mut().find(|i| i.id == incident_id) {
            Some(inc) => {
                if !inc.affected_instances.contains(&instance_id) {
                    inc.affected_instances.push(instance_id);
                }
                Ok(())
            }
            None => Err(format!("Incident {} not found", incident_id)),
        }
    }

    /// Determine root cause from health check data.
    pub fn determine_root_cause(
        &self,
        incident: &Incident,
        health_data: &[HealthCheck],
    ) -> RootCause {
        determine_root_cause_from_data(incident, health_data)
    }

    /// Generate a structured incident report for Commander.
    pub fn generate_report(&self, incident: &Incident) -> IncidentReport {
        let summary = build_summary(incident);
        let recommended_next_steps = build_next_steps(incident);

        IncidentReport {
            incident_id: incident.id.clone(),
            severity: incident.severity,
            title: incident.title.clone(),
            status: incident.status,
            summary,
            timeline_entries: incident.timeline.clone(),
            root_cause: incident.root_cause.clone(),
            actions_taken: incident.actions_taken.clone(),
            recommended_next_steps,
            generated_at: Utc::now(),
        }
    }

    /// Find an incident by ID.
    pub fn get(&self, incident_id: &str) -> Option<&Incident> {
        self.incidents.iter().find(|i| i.id == incident_id)
    }

    /// Return all open incidents.
    pub fn open_incidents(&self) -> Vec<&Incident> {
        self.incidents
            .iter()
            .filter(|i| i.status != IncidentStatus::Resolved)
            .collect()
    }

    /// Count by severity.
    pub fn count_by_severity(&self, severity: Severity) -> usize {
        self.incidents
            .iter()
            .filter(|i| i.severity == severity)
            .count()
    }
}

impl Default for IncidentManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn build_incident_title(trigger: &HealthEvent, severity: Severity) -> String {
    format!(
        "[{}] {} {} — {} (score: {})",
        severity, trigger.provider, trigger.region, trigger.description, trigger.health_score,
    )
}

fn build_summary(incident: &Incident) -> String {
    let affected_count = incident.affected_instances.len();
    let failover_entries: Vec<_> = incident
        .timeline
        .iter()
        .filter(|e| e.action.contains("failover"))
        .collect();

    let mut parts = vec![format!(
        "Triage report: {} instance(s) affected in {} {}.",
        affected_count, incident.provider, incident.region,
    )];

    if !failover_entries.is_empty() {
        parts.push(format!(
            "{} failover action(s) recorded.",
            failover_entries.len()
        ));
    }

    match incident.status {
        IncidentStatus::Open => parts.push("Incident is OPEN — investigating.".to_string()),
        IncidentStatus::Investigating => {
            parts.push("Active investigation in progress.".to_string())
        }
        IncidentStatus::Mitigated => {
            parts.push("Incident MITIGATED — monitoring for recurrence.".to_string())
        }
        IncidentStatus::Resolved => parts.push(format!(
            "Incident RESOLVED in {} minutes.",
            incident.duration_mins()
        )),
    }

    parts.join(" ")
}

fn build_next_steps(incident: &Incident) -> Vec<String> {
    let mut steps = Vec::new();

    match incident.status {
        IncidentStatus::Open | IncidentStatus::Investigating => {
            steps.push("Check provider status page for regional issues".to_string());
            if incident.severity <= Severity::P2 {
                steps.push(
                    "Confirm all affected instances have ACTIVE standbys before failover"
                        .to_string(),
                );
            }
            if incident.severity == Severity::P1 {
                steps.push("CRITICAL: Escalate to Commander immediately".to_string());
                steps.push("Verify zero users without an active gateway".to_string());
            }
        }
        IncidentStatus::Mitigated => {
            steps.push("Monitor for 30 minutes before closing".to_string());
            steps.push("Verify standby instances are reprovisioning".to_string());
        }
        IncidentStatus::Resolved => {
            steps.push("Schedule post-incident review".to_string());
            steps.push("Update runbook with lessons learned".to_string());
        }
    }

    steps
}

fn determine_root_cause_from_data(incident: &Incident, health_data: &[HealthCheck]) -> RootCause {
    // Analyse health check patterns to classify root cause
    let failing_checks: Vec<&HealthCheck> = health_data
        .iter()
        .filter(|c| {
            matches!(
                c.status,
                claw_proto::CheckStatus::Critical | claw_proto::CheckStatus::Degraded
            )
        })
        .collect();

    let has_network = failing_checks
        .iter()
        .any(|c| c.name.contains("tailscale") || c.name.contains("network"));
    let has_resource = failing_checks
        .iter()
        .any(|c| c.name.contains("cpu") || c.name.contains("mem") || c.name.contains("disk"));
    let has_service = failing_checks
        .iter()
        .any(|c| c.name.contains("openclaw") || c.name.contains("docker"));

    // Multiple instances in same region → likely provider outage
    let category = if incident.affected_instances.len() > 10 {
        RootCauseCategory::ProviderOutage
    } else if has_network {
        RootCauseCategory::NetworkIssue
    } else if has_resource {
        RootCauseCategory::ResourceExhaustion
    } else if has_service {
        RootCauseCategory::SoftwareBug
    } else {
        RootCauseCategory::Unknown
    };

    let evidence: Vec<String> = failing_checks
        .iter()
        .map(|c| format!("{}: {}", c.name, c.message))
        .collect();

    let confidence = if failing_checks.len() > 2 {
        RootCauseConfidence::High
    } else if failing_checks.len() == 1 {
        RootCauseConfidence::Medium
    } else {
        RootCauseConfidence::Low
    };

    let description = format!(
        "{:?} — {} failing health checks detected",
        category,
        failing_checks.len()
    );

    RootCause {
        category,
        description,
        confidence,
        evidence,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use claw_proto::{CheckStatus, VpsProvider};

    fn make_trigger(users: u32, data_loss: bool) -> HealthEvent {
        HealthEvent {
            instance_id: "i-test".to_string(),
            account_id: "acc-1".to_string(),
            health_score: 20,
            provider: VpsProvider::Hetzner,
            region: "eu-hetzner-nbg1".to_string(),
            description: "OpenClaw gateway unreachable".to_string(),
            affected_users: users,
            data_loss_risk: data_loss,
            detected_at: Utc::now(),
        }
    }

    // ─── Severity classification ─────────────────────────────────────────────

    #[test]
    fn test_severity_p1_data_loss() {
        assert_eq!(classify_severity(1, true), Severity::P1);
    }

    #[test]
    fn test_severity_p1_many_users() {
        assert_eq!(classify_severity(51, false), Severity::P1);
        assert_eq!(classify_severity(100, false), Severity::P1);
    }

    #[test]
    fn test_severity_p2() {
        assert_eq!(classify_severity(11, false), Severity::P2);
        assert_eq!(classify_severity(50, false), Severity::P2);
    }

    #[test]
    fn test_severity_p3() {
        assert_eq!(classify_severity(1, false), Severity::P3);
        assert_eq!(classify_severity(10, false), Severity::P3);
    }

    #[test]
    fn test_severity_p4() {
        assert_eq!(classify_severity(0, false), Severity::P4);
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::P1 > Severity::P4);
        assert!(Severity::P2 > Severity::P3);
    }

    // ─── IncidentManager ────────────────────────────────────────────────────

    #[test]
    fn test_create_incident_p3() {
        let mut mgr = IncidentManager::new();
        let trigger = make_trigger(5, false);
        let inc = mgr.create_incident(trigger);
        assert_eq!(inc.severity, Severity::P3);
        assert_eq!(inc.status, IncidentStatus::Open);
        assert_eq!(inc.timeline.len(), 1);
        assert!(inc.title.contains("P3"));
    }

    #[test]
    fn test_create_incident_p1_data_loss() {
        let mut mgr = IncidentManager::new();
        let trigger = make_trigger(1, true);
        let inc = mgr.create_incident(trigger);
        assert_eq!(inc.severity, Severity::P1);
    }

    #[test]
    fn test_add_timeline_entry() {
        let mut mgr = IncidentManager::new();
        let trigger = make_trigger(5, false);
        let id = {
            let inc = mgr.create_incident(trigger);
            inc.id.clone()
        };

        let entry = TimelineEntry {
            timestamp: Utc::now(),
            actor: "guardian".to_string(),
            action: "failover triggered".to_string(),
            outcome: "Standby promoted to primary".to_string(),
        };

        mgr.add_timeline_entry(&id, entry).unwrap();
        let inc = mgr.get(&id).unwrap();
        assert_eq!(inc.timeline.len(), 2);
    }

    #[test]
    fn test_add_timeline_entry_not_found() {
        let mut mgr = IncidentManager::new();
        let entry = TimelineEntry {
            timestamp: Utc::now(),
            actor: "guardian".to_string(),
            action: "test".to_string(),
            outcome: "test".to_string(),
        };
        let result = mgr.add_timeline_entry("nonexistent-id", entry);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_status_resolved() {
        let mut mgr = IncidentManager::new();
        let trigger = make_trigger(5, false);
        let id = {
            let inc = mgr.create_incident(trigger);
            inc.id.clone()
        };

        mgr.update_status(&id, IncidentStatus::Resolved).unwrap();
        let inc = mgr.get(&id).unwrap();
        assert_eq!(inc.status, IncidentStatus::Resolved);
        assert!(inc.resolved_at.is_some());
    }

    #[test]
    fn test_open_incidents_filter() {
        let mut mgr = IncidentManager::new();
        let id1 = mgr.create_incident(make_trigger(5, false)).id.clone();
        mgr.create_incident(make_trigger(5, false));

        mgr.update_status(&id1, IncidentStatus::Resolved).unwrap();
        assert_eq!(mgr.open_incidents().len(), 1);
    }

    #[test]
    fn test_count_by_severity() {
        let mut mgr = IncidentManager::new();
        mgr.create_incident(make_trigger(100, false)); // P1
        mgr.create_incident(make_trigger(20, false)); // P2
        mgr.create_incident(make_trigger(5, false)); // P3

        assert_eq!(mgr.count_by_severity(Severity::P1), 1);
        assert_eq!(mgr.count_by_severity(Severity::P2), 1);
        assert_eq!(mgr.count_by_severity(Severity::P3), 1);
        assert_eq!(mgr.count_by_severity(Severity::P4), 0);
    }

    #[test]
    fn test_add_affected_instance_no_dupe() {
        let mut mgr = IncidentManager::new();
        let id = mgr.create_incident(make_trigger(5, false)).id.clone();

        mgr.add_affected_instance(&id, "i-test".to_string())
            .unwrap();
        // i-test already in list from trigger
        let inc = mgr.get(&id).unwrap();
        assert_eq!(inc.affected_instances.len(), 1);

        mgr.add_affected_instance(&id, "i-new".to_string()).unwrap();
        let inc = mgr.get(&id).unwrap();
        assert_eq!(inc.affected_instances.len(), 2);
    }

    // ─── Root cause ─────────────────────────────────────────────────────────

    #[test]
    fn test_determine_root_cause_provider_outage() {
        let mut mgr = IncidentManager::new();
        let trigger = make_trigger(100, false);
        let id = mgr.create_incident(trigger).id.clone();

        // Simulate 15 affected instances
        for n in 0..14 {
            mgr.add_affected_instance(&id, format!("i-extra-{}", n))
                .unwrap();
        }

        let inc = mgr.get(&id).unwrap().clone();
        let health_data = vec![];
        let rc = mgr.determine_root_cause(&inc, &health_data);
        assert_eq!(rc.category, RootCauseCategory::ProviderOutage);
    }

    #[test]
    fn test_determine_root_cause_network() {
        let mgr = IncidentManager::new();
        let trigger = make_trigger(3, false);
        // Construct incident manually for test
        let incident = Incident {
            id: "test-inc".to_string(),
            severity: Severity::P3,
            title: "test".to_string(),
            description: trigger.description,
            status: IncidentStatus::Open,
            affected_instances: vec!["i-test".to_string()],
            affected_users: 3,
            provider: VpsProvider::Hetzner,
            region: "eu-hetzner-nbg1".to_string(),
            timeline: vec![],
            root_cause: None,
            actions_taken: vec![],
            created_at: Utc::now(),
            resolved_at: None,
        };
        let health_data = vec![HealthCheck {
            name: "tailscale".to_string(),
            status: CheckStatus::Critical,
            message: "Tailscale disconnected".to_string(),
            value: None,
        }];
        let rc = mgr.determine_root_cause(&incident, &health_data);
        assert_eq!(rc.category, RootCauseCategory::NetworkIssue);
    }

    // ─── Report generation ───────────────────────────────────────────────────

    #[test]
    fn test_generate_report_structure() {
        let mut mgr = IncidentManager::new();
        let trigger = make_trigger(60, false); // P1
        let id = mgr.create_incident(trigger).id.clone();
        let inc = mgr.get(&id).unwrap().clone();
        let report = mgr.generate_report(&inc);

        assert_eq!(report.incident_id, id);
        assert_eq!(report.severity, Severity::P1);
        assert!(!report.summary.is_empty());
        assert!(!report.recommended_next_steps.is_empty());
        // P1 should escalate
        assert!(
            report
                .recommended_next_steps
                .iter()
                .any(|s| s.contains("CRITICAL"))
        );
    }

    #[test]
    fn test_generate_report_resolved() {
        let mut mgr = IncidentManager::new();
        let id = mgr.create_incident(make_trigger(5, false)).id.clone();
        mgr.update_status(&id, IncidentStatus::Resolved).unwrap();
        let inc = mgr.get(&id).unwrap().clone();
        let report = mgr.generate_report(&inc);

        assert!(report.summary.contains("RESOLVED"));
        assert!(
            report
                .recommended_next_steps
                .iter()
                .any(|s| s.contains("post-incident"))
        );
    }

    #[test]
    fn test_incident_serialization() {
        let mut mgr = IncidentManager::new();
        let trigger = make_trigger(5, false);
        let id = mgr.create_incident(trigger).id.clone();
        let inc = mgr.get(&id).unwrap().clone();
        let json = serde_json::to_string(&inc).expect("serialize");
        let back: Incident = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, id);
        assert_eq!(back.severity, Severity::P3);
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(Severity::P1.to_string(), "P1");
        assert_eq!(Severity::P4.to_string(), "P4");
    }
}
