//! Commander orchestration engine — the conversational brain of ClawOps.
//!
//! Parses operator intent, enforces PRD safety rules, routes to specialists,
//! and synthesises human-readable responses.
//!
//! Safety rules (PRD §5.1 / §10.1 — hard constraints):
//! - Never teardown an ACTIVE PRIMARY without confirming STANDBY is ACTIVE
//! - Never push config to > 100 instances without rolling validation
//! - Never execute provider API deletes without audit log entry first
//! - Always check provider status page before declaring widespread incident
//! - Require explicit confirmation for actions affecting > 10 users

#![forbid(unsafe_code)]

use claw_briefer::{FleetBriefing, WeeklyReport};
use claw_ledger::{Optimization, ProviderComparison, WasteReport};
use claw_triage::IncidentReport;
use serde::{Deserialize, Serialize};

// ─── Operator Intent ──────────────────────────────────────────────────────────

/// What the operator wants — classified from free-text message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorIntent {
    /// Provision new VPS pairs.
    ProvisionRequest {
        count: u32,
        tier_hint: Option<String>,
    },
    /// Teardown instances.
    TeardownRequest { scope: TeardownScope },
    /// Cost query or analysis.
    CostQuery { detail: CostQueryDetail },
    /// Fleet or instance health query.
    HealthQuery { scope: HealthScope },
    /// Config push operation.
    ConfigPush { instance_count_hint: Option<u32> },
    /// Incident status or history query.
    IncidentQuery,
    /// Fleet-wide status overview.
    FleetStatus,
    /// Bulk operation across many instances.
    BulkOperation {
        operation: String,
        instance_count: u32,
    },
    /// Intent could not be determined.
    Unknown { raw_message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeardownScope {
    /// Single instance by ID.
    Single { instance_id: String },
    /// All idle accounts.
    IdleAccounts,
    /// Operator-specified list.
    Custom { count: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostQueryDetail {
    Waste,
    Projection,
    ProviderComparison,
    General,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthScope {
    Fleet,
    Instance { instance_id: String },
    Provider { provider_name: String },
    Region { region_name: String },
}

// ─── Specialist actions ───────────────────────────────────────────────────────

/// Which specialist agent should handle this, and what to ask it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpecialistAction {
    /// Spawn the Forge agent for provisioning.
    SpawnForge { task: String },
    /// Send to Guardian for health work.
    SendToGuardian { task: String },
    /// Send to Ledger for cost analysis.
    SendToLedger { task: String },
    /// Spawn Triage for incident investigation.
    SpawnTriage { task: String },
    /// Handle directly in Commander (simple status queries).
    HandleDirectly { task: String },
}

// ─── Specialist result ────────────────────────────────────────────────────────

/// The structured result returned by a specialist agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpecialistResult {
    FleetStatusResult {
        summary: String,
        active_pairs: u32,
    },
    HealthResult {
        summary: String,
        degraded: u32,
        failed: u32,
    },
    CostResult {
        waste_report: Option<WasteReport>,
        summary: String,
    },
    ProvisionResult {
        success_count: u32,
        failed_count: u32,
        summary: String,
    },
    IncidentResult {
        report: Option<IncidentReport>,
        summary: String,
    },
    OptimizationResult {
        optimizations: Vec<Optimization>,
        summary: String,
    },
    ProviderComparisonResult {
        comparison: ProviderComparison,
        summary: String,
    },
    BriefingResult {
        briefing: Option<FleetBriefing>,
        report: Option<WeeklyReport>,
        summary: String,
    },
    GenericResult {
        summary: String,
    },
}

// ─── Safety rules ─────────────────────────────────────────────────────────────

/// Hard safety constraints from PRD §5.1 / §10.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyRules {
    /// Max users an action can affect without requiring explicit confirmation.
    pub max_affected_users_without_confirm: u32,
    /// Max cost spike % above projection before blocking.
    pub max_cost_spike_percent: f64,
    /// Must verify standby is ACTIVE before any teardown of a primary.
    pub require_standby_before_teardown: bool,
    /// Max instances for a non-rolling config push.
    pub max_instances_direct_config_push: u32,
    /// Must write audit log entry before any provider delete.
    pub require_audit_before_delete: bool,
}

impl Default for SafetyRules {
    fn default() -> Self {
        Self {
            max_affected_users_without_confirm: 10,
            max_cost_spike_percent: 20.0,
            require_standby_before_teardown: true,
            max_instances_direct_config_push: 100,
            require_audit_before_delete: true,
        }
    }
}

/// The result of a safety check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyResult {
    /// Action is safe to proceed.
    Approved,
    /// Action requires explicit operator confirmation before proceeding.
    RequiresConfirmation { reason: String },
    /// Action is blocked — safety invariant would be violated.
    Blocked { reason: String },
}

/// A proposed action to be safety-checked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub action_type: ActionType,
    pub affected_users: u32,
    pub affected_instance_count: u32,
    pub is_primary_teardown: bool,
    pub standby_confirmed_active: bool,
    pub estimated_cost_change_pct: f64,
    pub has_audit_log_entry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    Provision,
    Teardown,
    ConfigPush,
    TierResize,
    Failover,
    BulkOperation,
    CostAction,
}

// ─── Commander Engine ─────────────────────────────────────────────────────────

/// The orchestration brain of the ClawOps operator team.
pub struct CommanderEngine {
    pub safety_rules: SafetyRules,
}

impl CommanderEngine {
    pub fn new() -> Self {
        Self {
            safety_rules: SafetyRules::default(),
        }
    }

    pub fn with_safety_rules(safety_rules: SafetyRules) -> Self {
        Self { safety_rules }
    }

    /// Parse operator free-text into a classified OperatorIntent.
    ///
    /// Uses keyword matching — in production this is backed by the LLM
    /// reading the clawops.md skill.
    pub fn parse_intent(&self, message: &str) -> OperatorIntent {
        let lower = message.to_lowercase();

        // Provision
        if lower.contains("provision") || lower.contains("create") && lower.contains("account") {
            let count = extract_number(&lower).unwrap_or(1);
            let tier_hint = extract_tier(&lower);
            return OperatorIntent::ProvisionRequest { count, tier_hint };
        }

        // Teardown
        if lower.contains("teardown") || lower.contains("tear down") || lower.contains("delete") {
            let scope = if lower.contains("idle") {
                TeardownScope::IdleAccounts
            } else {
                let count = extract_number(&lower).unwrap_or(1);
                TeardownScope::Custom { count }
            };
            return OperatorIntent::TeardownRequest { scope };
        }

        // Cost
        if lower.contains("cost")
            || lower.contains("wast")
            || lower.contains("spend")
            || lower.contains("billing")
        {
            let detail = if lower.contains("wast") || lower.contains("idle") {
                CostQueryDetail::Waste
            } else if lower.contains("project") || lower.contains("forecast") {
                CostQueryDetail::Projection
            } else if lower.contains("provider") || lower.contains("compare") {
                CostQueryDetail::ProviderComparison
            } else {
                CostQueryDetail::General
            };
            return OperatorIntent::CostQuery { detail };
        }

        // Config push
        if lower.contains("config") || lower.contains("push") && lower.contains("model") {
            let instance_count_hint = extract_number(&lower);
            return OperatorIntent::ConfigPush {
                instance_count_hint,
            };
        }

        // Incident
        if lower.contains("incident") || lower.contains("down") || lower.contains("outage") {
            return OperatorIntent::IncidentQuery;
        }

        // Health
        if lower.contains("health") || lower.contains("status") || lower.contains("degraded") {
            return OperatorIntent::HealthQuery {
                scope: HealthScope::Fleet,
            };
        }

        // Bulk
        if lower.contains("all instance") || lower.contains("bulk") || lower.contains("restart all")
        {
            let count = extract_number(&lower).unwrap_or(0);
            return OperatorIntent::BulkOperation {
                operation: extract_bulk_op(&lower),
                instance_count: count,
            };
        }

        // Fleet status
        if lower.contains("fleet") || lower.contains("overview") || lower.contains("summary") {
            return OperatorIntent::FleetStatus;
        }

        OperatorIntent::Unknown {
            raw_message: message.to_string(),
        }
    }

    /// Determine which specialist should handle the intent.
    pub fn route_to_specialist(&self, intent: &OperatorIntent) -> SpecialistAction {
        match intent {
            OperatorIntent::ProvisionRequest { count, tier_hint } => SpecialistAction::SpawnForge {
                task: format!(
                    "Provision {} pairs (tier: {})",
                    count,
                    tier_hint.as_deref().unwrap_or("standard")
                ),
            },

            OperatorIntent::TeardownRequest { scope } => match scope {
                TeardownScope::IdleAccounts => SpecialistAction::SendToLedger {
                    task: "Execute teardown of idle accounts with archive".to_string(),
                },
                TeardownScope::Single { instance_id } => SpecialistAction::SpawnForge {
                    task: format!("Teardown instance {}", instance_id),
                },
                TeardownScope::Custom { count } => SpecialistAction::SpawnForge {
                    task: format!("Teardown {} instances", count),
                },
            },

            OperatorIntent::CostQuery { detail } => match detail {
                CostQueryDetail::ProviderComparison => SpecialistAction::SendToLedger {
                    task: "Generate provider comparison report".to_string(),
                },
                _ => SpecialistAction::SendToLedger {
                    task: "Generate cost analysis report".to_string(),
                },
            },

            OperatorIntent::HealthQuery { scope } => SpecialistAction::SendToGuardian {
                task: format!("Run health sweep: {:?}", scope),
            },

            OperatorIntent::ConfigPush {
                instance_count_hint,
            } => {
                let count = instance_count_hint.unwrap_or(0);
                if count > self.safety_rules.max_instances_direct_config_push {
                    SpecialistAction::HandleDirectly {
                        task: format!(
                            "Config push to {} instances — requires rolling push (>{})",
                            count, self.safety_rules.max_instances_direct_config_push
                        ),
                    }
                } else {
                    SpecialistAction::SendToGuardian {
                        task: "Execute config push".to_string(),
                    }
                }
            }

            OperatorIntent::IncidentQuery => SpecialistAction::SpawnTriage {
                task: "Investigate and report current incident".to_string(),
            },

            OperatorIntent::FleetStatus | OperatorIntent::Unknown { .. } => {
                SpecialistAction::HandleDirectly {
                    task: "Return fleet status overview".to_string(),
                }
            }

            OperatorIntent::BulkOperation {
                operation,
                instance_count,
            } => SpecialistAction::SendToGuardian {
                task: format!("Bulk {} across {} instances", operation, instance_count),
            },
        }
    }

    /// Check whether an action passes the PRD safety invariants.
    pub fn safety_check(&self, action: &Action) -> SafetyResult {
        // Hard block: teardown primary without confirmed standby
        if action.action_type == ActionType::Teardown
            && action.is_primary_teardown
            && self.safety_rules.require_standby_before_teardown
            && !action.standby_confirmed_active
        {
            return SafetyResult::Blocked {
                reason: "SAFETY: Cannot teardown ACTIVE PRIMARY — standby is not confirmed ACTIVE"
                    .to_string(),
            };
        }

        // Hard block: provider delete without audit log
        if action.action_type == ActionType::Teardown
            && self.safety_rules.require_audit_before_delete
            && !action.has_audit_log_entry
        {
            return SafetyResult::Blocked {
                reason: "SAFETY: Cannot execute provider delete without audit log entry"
                    .to_string(),
            };
        }

        // Hard block: config push to > 100 without rolling validation
        if action.action_type == ActionType::ConfigPush
            && action.affected_instance_count > self.safety_rules.max_instances_direct_config_push
        {
            return SafetyResult::Blocked {
                reason: format!(
                    "SAFETY: Config push to {} instances requires rolling validation (max direct: {})",
                    action.affected_instance_count,
                    self.safety_rules.max_instances_direct_config_push
                ),
            };
        }

        // Require confirmation: > 10 users affected
        if action.affected_users > self.safety_rules.max_affected_users_without_confirm {
            return SafetyResult::RequiresConfirmation {
                reason: format!(
                    "Action affects {} users — explicit confirmation required (threshold: {})",
                    action.affected_users, self.safety_rules.max_affected_users_without_confirm
                ),
            };
        }

        // Require confirmation: cost spike
        if action.estimated_cost_change_pct > self.safety_rules.max_cost_spike_percent {
            return SafetyResult::RequiresConfirmation {
                reason: format!(
                    "Estimated cost change +{:.1}% exceeds threshold ({:.1}%)",
                    action.estimated_cost_change_pct, self.safety_rules.max_cost_spike_percent
                ),
            };
        }

        SafetyResult::Approved
    }

    /// Synthesise specialist results into a human-readable operator response.
    pub fn synthesize_response(&self, results: Vec<SpecialistResult>) -> String {
        if results.is_empty() {
            return "[CMD] No results from specialists.".to_string();
        }

        let mut parts: Vec<String> = Vec::new();

        for result in results {
            let line = match result {
                SpecialistResult::FleetStatusResult {
                    summary,
                    active_pairs,
                } => {
                    format!("[CMD] Fleet: {} active pairs. {}", active_pairs, summary)
                }
                SpecialistResult::HealthResult {
                    summary,
                    degraded,
                    failed,
                } => {
                    format!(
                        "[CMD] Health: {} degraded, {} failed. {}",
                        degraded, failed, summary
                    )
                }
                SpecialistResult::CostResult { summary, .. } => {
                    format!("[Ledger] {}", summary)
                }
                SpecialistResult::ProvisionResult {
                    success_count,
                    failed_count,
                    summary,
                } => {
                    format!(
                        "[CMD] Provisioned {}/{} pairs. {}",
                        success_count,
                        success_count + failed_count,
                        summary
                    )
                }
                SpecialistResult::IncidentResult { summary, .. } => {
                    format!("[CMD] {}", summary)
                }
                SpecialistResult::OptimizationResult {
                    optimizations,
                    summary,
                } => {
                    format!(
                        "[Ledger] {} optimisation(s) recommended. {}",
                        optimizations.len(),
                        summary
                    )
                }
                SpecialistResult::ProviderComparisonResult { summary, .. } => {
                    format!("[Ledger] {}", summary)
                }
                SpecialistResult::BriefingResult { summary, .. } => {
                    format!("[Briefer] {}", summary)
                }
                SpecialistResult::GenericResult { summary } => {
                    format!("[CMD] {}", summary)
                }
            };
            parts.push(line);
        }

        parts.join("\n")
    }
}

impl Default for CommanderEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn extract_number(text: &str) -> Option<u32> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for w in words {
        if let Ok(n) = w.parse::<u32>() {
            return Some(n);
        }
    }
    None
}

fn extract_tier(text: &str) -> Option<String> {
    if text.contains("enterprise") {
        Some("enterprise".to_string())
    } else if text.contains("pro") {
        Some("pro".to_string())
    } else if text.contains("nano") {
        Some("nano".to_string())
    } else if text.contains("standard") {
        Some("standard".to_string())
    } else {
        None
    }
}

fn extract_bulk_op(text: &str) -> String {
    if text.contains("restart") {
        "restart".to_string()
    } else if text.contains("config") {
        "config push".to_string()
    } else if text.contains("health") {
        "health check".to_string()
    } else {
        "operation".to_string()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn eng() -> CommanderEngine {
        CommanderEngine::new()
    }

    // ─── Intent parsing ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_provision_intent() {
        let intent = eng().parse_intent("Provision 20 new standard-tier accounts");
        assert!(
            matches!(intent, OperatorIntent::ProvisionRequest { count: 20, .. }),
            "got {:?}",
            intent
        );
    }

    #[test]
    fn test_parse_provision_with_tier() {
        let intent = eng().parse_intent("Provision 5 enterprise accounts for beta");
        if let OperatorIntent::ProvisionRequest { tier_hint, .. } = intent {
            assert_eq!(tier_hint, Some("enterprise".to_string()));
        } else {
            panic!("Expected ProvisionRequest");
        }
    }

    #[test]
    fn test_parse_cost_waste_query() {
        let intent = eng().parse_intent("What are we wasting money on this month?");
        assert_eq!(
            intent,
            OperatorIntent::CostQuery {
                detail: CostQueryDetail::Waste
            }
        );
    }

    #[test]
    fn test_parse_cost_general_query() {
        let intent = eng().parse_intent("How much are we spending?");
        assert_eq!(
            intent,
            OperatorIntent::CostQuery {
                detail: CostQueryDetail::General
            }
        );
    }

    #[test]
    fn test_parse_teardown_idle() {
        let intent = eng().parse_intent("Teardown idle accounts");
        assert_eq!(
            intent,
            OperatorIntent::TeardownRequest {
                scope: TeardownScope::IdleAccounts
            }
        );
    }

    #[test]
    fn test_parse_config_push() {
        let intent = eng().parse_intent("Push the new config to 847 instances");
        assert!(
            matches!(
                intent,
                OperatorIntent::ConfigPush {
                    instance_count_hint: Some(847)
                }
            ),
            "got {:?}",
            intent
        );
    }

    #[test]
    fn test_parse_incident_query() {
        let intent = eng().parse_intent("Hetzner Nuremberg looks down. What's our exposure?");
        assert_eq!(intent, OperatorIntent::IncidentQuery);
    }

    #[test]
    fn test_parse_health_query() {
        let intent = eng().parse_intent("What's the fleet health?");
        assert!(
            matches!(intent, OperatorIntent::HealthQuery { .. }),
            "got {:?}",
            intent
        );
    }

    #[test]
    fn test_parse_fleet_status() {
        let intent = eng().parse_intent("Give me a fleet overview");
        assert_eq!(intent, OperatorIntent::FleetStatus);
    }

    #[test]
    fn test_parse_unknown() {
        let intent = eng().parse_intent("Hello there");
        assert!(matches!(intent, OperatorIntent::Unknown { .. }));
    }

    // ─── Routing ────────────────────────────────────────────────────────────

    #[test]
    fn test_route_provision_to_forge() {
        let action = eng().route_to_specialist(&OperatorIntent::ProvisionRequest {
            count: 20,
            tier_hint: Some("standard".to_string()),
        });
        assert!(matches!(action, SpecialistAction::SpawnForge { .. }));
    }

    #[test]
    fn test_route_cost_to_ledger() {
        let action = eng().route_to_specialist(&OperatorIntent::CostQuery {
            detail: CostQueryDetail::Waste,
        });
        assert!(matches!(action, SpecialistAction::SendToLedger { .. }));
    }

    #[test]
    fn test_route_incident_to_triage() {
        let action = eng().route_to_specialist(&OperatorIntent::IncidentQuery);
        assert!(matches!(action, SpecialistAction::SpawnTriage { .. }));
    }

    #[test]
    fn test_route_health_to_guardian() {
        let action = eng().route_to_specialist(&OperatorIntent::HealthQuery {
            scope: HealthScope::Fleet,
        });
        assert!(matches!(action, SpecialistAction::SendToGuardian { .. }));
    }

    #[test]
    fn test_route_large_config_push_to_direct() {
        let action = eng().route_to_specialist(&OperatorIntent::ConfigPush {
            instance_count_hint: Some(847),
        });
        // > 100 instances → must handle directly (rolling push required)
        assert!(matches!(action, SpecialistAction::HandleDirectly { .. }));
    }

    #[test]
    fn test_route_small_config_push_to_guardian() {
        let action = eng().route_to_specialist(&OperatorIntent::ConfigPush {
            instance_count_hint: Some(50),
        });
        assert!(matches!(action, SpecialistAction::SendToGuardian { .. }));
    }

    // ─── Safety checks ──────────────────────────────────────────────────────

    fn safe_action(action_type: ActionType) -> Action {
        Action {
            action_type,
            affected_users: 5,
            affected_instance_count: 10,
            is_primary_teardown: false,
            standby_confirmed_active: true,
            estimated_cost_change_pct: 0.0,
            has_audit_log_entry: true,
        }
    }

    #[test]
    fn test_safety_approved_simple() {
        let result = eng().safety_check(&safe_action(ActionType::Provision));
        assert_eq!(result, SafetyResult::Approved);
    }

    #[test]
    fn test_safety_blocked_teardown_no_standby() {
        let mut action = safe_action(ActionType::Teardown);
        action.is_primary_teardown = true;
        action.standby_confirmed_active = false;

        let result = eng().safety_check(&action);
        assert!(
            matches!(result, SafetyResult::Blocked { .. }),
            "expected Blocked, got {:?}",
            result
        );
    }

    #[test]
    fn test_safety_blocked_teardown_no_audit() {
        let mut action = safe_action(ActionType::Teardown);
        action.has_audit_log_entry = false;

        let result = eng().safety_check(&action);
        assert!(matches!(result, SafetyResult::Blocked { .. }));
    }

    #[test]
    fn test_safety_blocked_config_push_too_large() {
        let mut action = safe_action(ActionType::ConfigPush);
        action.affected_instance_count = 150;

        let result = eng().safety_check(&action);
        assert!(matches!(result, SafetyResult::Blocked { .. }));
    }

    #[test]
    fn test_safety_requires_confirmation_many_users() {
        let mut action = safe_action(ActionType::BulkOperation);
        action.affected_users = 50;

        let result = eng().safety_check(&action);
        assert!(matches!(result, SafetyResult::RequiresConfirmation { .. }));
    }

    #[test]
    fn test_safety_requires_confirmation_cost_spike() {
        let mut action = safe_action(ActionType::Provision);
        action.estimated_cost_change_pct = 25.0;

        let result = eng().safety_check(&action);
        assert!(matches!(result, SafetyResult::RequiresConfirmation { .. }));
    }

    #[test]
    fn test_safety_config_push_exactly_100_approved() {
        let mut action = safe_action(ActionType::ConfigPush);
        action.affected_instance_count = 100;
        // 100 is NOT > 100, so it should pass

        let result = eng().safety_check(&action);
        assert_eq!(result, SafetyResult::Approved);
    }

    #[test]
    fn test_safety_teardown_standby_confirmed_approved() {
        let mut action = safe_action(ActionType::Teardown);
        action.is_primary_teardown = true;
        action.standby_confirmed_active = true;
        action.has_audit_log_entry = true;

        let result = eng().safety_check(&action);
        assert_eq!(result, SafetyResult::Approved);
    }

    // ─── Response synthesis ─────────────────────────────────────────────────

    #[test]
    fn test_synthesize_empty() {
        let r = eng().synthesize_response(vec![]);
        assert!(r.contains("No results"));
    }

    #[test]
    fn test_synthesize_fleet_status() {
        let results = vec![SpecialistResult::FleetStatusResult {
            summary: "All pairs healthy.".to_string(),
            active_pairs: 847,
        }];
        let r = eng().synthesize_response(results);
        assert!(r.contains("[CMD]"));
        assert!(r.contains("847"));
    }

    #[test]
    fn test_synthesize_multiple_results() {
        let results = vec![
            SpecialistResult::FleetStatusResult {
                summary: "Nominal.".to_string(),
                active_pairs: 100,
            },
            SpecialistResult::CostResult {
                waste_report: None,
                summary: "$492/month recoverable.".to_string(),
            },
        ];
        let r = eng().synthesize_response(results);
        assert!(r.contains("[CMD]"));
        assert!(r.contains("[Ledger]"));
    }

    #[test]
    fn test_synthesize_provision_result() {
        let results = vec![SpecialistResult::ProvisionResult {
            success_count: 20,
            failed_count: 0,
            summary: "All pairs ACTIVE.".to_string(),
        }];
        let r = eng().synthesize_response(results);
        assert!(r.contains("20/20"));
    }

    #[test]
    fn test_safety_rules_default_values() {
        let rules = SafetyRules::default();
        assert_eq!(rules.max_affected_users_without_confirm, 10);
        assert_eq!(rules.max_cost_spike_percent, 20.0);
        assert!(rules.require_standby_before_teardown);
        assert_eq!(rules.max_instances_direct_config_push, 100);
        assert!(rules.require_audit_before_delete);
    }

    #[test]
    fn test_intent_serialization() {
        let intent = OperatorIntent::ProvisionRequest {
            count: 20,
            tier_hint: Some("standard".to_string()),
        };
        let json = serde_json::to_string(&intent).expect("serialize");
        let back: OperatorIntent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(intent, back);
    }
}
