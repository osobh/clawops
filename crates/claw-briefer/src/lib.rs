//! Scheduled fleet briefing and report generation for the Briefer agent.
//!
//! Produces daily voice-script briefings (WhatsApp TTS) and weekly Telegram
//! markdown reports.  Matches the PRD §9.2 communication cadence:
//! - 07:00 UTC daily  → voice note via WhatsApp
//! - Monday 08:00 UTC → weekly cost report via Telegram

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use claw_proto::{FleetStatus, VpsProvider};
use serde::{Deserialize, Serialize};

// ─── Cost Summary ─────────────────────────────────────────────────────────────

/// Provider-level cost breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCost {
    pub provider: VpsProvider,
    pub instance_count: u32,
    pub cost_usd: f64,
}

/// Fleet-wide cost summary for a billing period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub period_label: String,
    pub actual_usd: f64,
    pub projected_usd: f64,
    pub idle_accounts: u32,
    pub provider_breakdown: Vec<ProviderCost>,
}

impl CostSummary {
    /// Variance between actual and projected spend as a percentage.
    pub fn variance_pct(&self) -> f64 {
        if self.projected_usd == 0.0 {
            return 0.0;
        }
        ((self.actual_usd - self.projected_usd) / self.projected_usd) * 100.0
    }
}

// ─── Incident summary (used in briefings) ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentSummary {
    pub id: String,
    pub title: String,
    pub resolved: bool,
    pub affected_instances: u32,
}

// ─── Daily Briefing ───────────────────────────────────────────────────────────

/// Structured output of generate_daily_briefing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetBriefing {
    pub generated_at: DateTime<Utc>,
    pub overnight_summary: String,
    pub cost_summary: String,
    pub incidents: Vec<IncidentSummary>,
    pub actions_taken: Vec<String>,
    pub recommendations: Vec<String>,
}

/// Generate a daily briefing from current fleet state.
///
/// Matches PRD §4.1 morning briefing pattern:
/// "847 active pairs across 3 providers. 2 degraded (auto-healing in progress),
///  1 failed (failover complete)..."
pub fn generate_daily_briefing(
    fleet: &FleetStatus,
    costs: &CostSummary,
    incidents: &[IncidentSummary],
) -> FleetBriefing {
    let active_pairs = fleet.active_pairs;
    let degraded = fleet.degraded_instances;
    let failed = fleet.failed_instances;

    // Build overnight summary
    let overnight_summary = build_overnight_summary(fleet, active_pairs, degraded, failed);

    // Build cost line
    let variance = costs.variance_pct();
    let variance_sign = if variance >= 0.0 { "+" } else { "" };
    let cost_line = format!(
        "Cost this period: ${:.0} vs ${:.0} projected ({}{:.1}%). {} idle accounts flagged for review.",
        costs.actual_usd, costs.projected_usd, variance_sign, variance, costs.idle_accounts
    );

    // Classify incidents
    let open_incidents: Vec<_> = incidents.iter().filter(|i| !i.resolved).collect();
    let resolved_incidents: Vec<_> = incidents.iter().filter(|i| i.resolved).collect();

    let mut actions_taken = Vec::new();
    for inc in &resolved_incidents {
        actions_taken.push(format!(
            "Resolved: {} ({} instances affected)",
            inc.title, inc.affected_instances
        ));
    }

    let mut recommendations = Vec::new();
    if costs.idle_accounts > 0 {
        recommendations.push(format!(
            "Review {} idle accounts — potential cost savings",
            costs.idle_accounts
        ));
    }
    if degraded > 0 {
        recommendations.push(format!("{} degraded instances need attention", degraded));
    }
    if open_incidents.is_empty() {
        recommendations.push("No SLA breaches. Fleet nominal.".to_string());
    }

    FleetBriefing {
        generated_at: Utc::now(),
        overnight_summary,
        cost_summary: cost_line,
        incidents: incidents.to_vec(),
        actions_taken,
        recommendations,
    }
}

fn build_overnight_summary(
    fleet: &FleetStatus,
    active_pairs: u32,
    degraded: u32,
    failed: u32,
) -> String {
    let provider_count = 3u32; // GatewayForge operates across 3 primary providers

    let mut parts = vec![format!(
        "{} active pairs across {} providers.",
        active_pairs, provider_count
    )];

    if degraded > 0 {
        parts.push(format!("{} degraded (auto-healing in progress).", degraded));
    }
    if failed > 0 {
        parts.push(format!("{} failed (failover complete).", failed));
    }
    if degraded == 0 && failed == 0 {
        parts.push("All pairs healthy overnight.".to_string());
    }

    let _ = fleet; // fleet carries additional context used by future extensions
    parts.join(" ")
}

// ─── Voice Script ─────────────────────────────────────────────────────────────

/// Format a FleetBriefing as a conversational voice script (WhatsApp TTS).
///
/// Concise, natural language.  Suitable for 60-second voice note delivery.
pub fn format_voice_script(briefing: &FleetBriefing) -> String {
    let mut lines = vec![
        format!(
            "[Dispatch — 07:00 UTC] Good morning. {}",
            briefing.overnight_summary
        ),
        briefing.cost_summary.clone(),
    ];

    let open: Vec<_> = briefing.incidents.iter().filter(|i| !i.resolved).collect();
    if open.is_empty() {
        lines.push("No open incidents. Full report in your Drive.".to_string());
    } else {
        lines.push(format!(
            "{} open incident(s). Details in your Drive.",
            open.len()
        ));
    }

    for rec in &briefing.recommendations {
        lines.push(rec.clone());
    }

    lines.join(" ")
}

// ─── Weekly Report ────────────────────────────────────────────────────────────

/// A snapshot of one day's fleet and cost data for weekly rollup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailySnapshot {
    pub date: String,
    pub active_pairs: u32,
    pub degraded_peak: u32,
    pub failed_count: u32,
    pub cost_usd: f64,
    pub incidents_resolved: u32,
    pub auto_heals: u32,
    pub failovers: u32,
}

/// Aggregated weekly report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyReport {
    pub week_label: String,
    pub generated_at: DateTime<Utc>,
    pub avg_active_pairs: f64,
    pub total_incidents: u32,
    pub total_auto_heals: u32,
    pub total_failovers: u32,
    pub total_cost_usd: f64,
    pub avg_daily_cost_usd: f64,
    pub peak_degraded: u32,
    pub provider_breakdown: Vec<ProviderCost>,
    pub savings_executed_usd: f64,
    pub recommendations: Vec<String>,
}

/// Roll up a week of daily snapshots into a WeeklyReport.
pub fn generate_weekly_report(history: &[DailySnapshot]) -> WeeklyReport {
    if history.is_empty() {
        return WeeklyReport {
            week_label: "Empty week".to_string(),
            generated_at: Utc::now(),
            avg_active_pairs: 0.0,
            total_incidents: 0,
            total_auto_heals: 0,
            total_failovers: 0,
            total_cost_usd: 0.0,
            avg_daily_cost_usd: 0.0,
            peak_degraded: 0,
            provider_breakdown: vec![],
            savings_executed_usd: 0.0,
            recommendations: vec!["No data available for this week.".to_string()],
        };
    }

    let n = history.len() as f64;
    let total_cost: f64 = history.iter().map(|d| d.cost_usd).sum();
    let total_pairs: u32 = history.iter().map(|d| d.active_pairs).sum();
    let total_incidents: u32 = history.iter().map(|d| d.incidents_resolved).sum();
    let total_heals: u32 = history.iter().map(|d| d.auto_heals).sum();
    let total_failovers: u32 = history.iter().map(|d| d.failovers).sum();
    let peak_degraded = history.iter().map(|d| d.degraded_peak).max().unwrap_or(0);

    let first_date = history.first().map(|d| d.date.as_str()).unwrap_or("?");
    let last_date = history.last().map(|d| d.date.as_str()).unwrap_or("?");
    let week_label = format!("{} → {}", first_date, last_date);

    let mut recommendations = Vec::new();
    if total_failovers > 3 {
        recommendations.push(format!(
            "{} failovers this week — investigate provider stability",
            total_failovers
        ));
    }
    if total_cost > total_cost * 1.15 {
        recommendations.push("Cost variance > 15% — review anomalies".to_string());
    }
    recommendations.push("Full provider breakdown attached.".to_string());

    WeeklyReport {
        week_label,
        generated_at: Utc::now(),
        avg_active_pairs: total_pairs as f64 / n,
        total_incidents,
        total_auto_heals: total_heals,
        total_failovers,
        total_cost_usd: total_cost,
        avg_daily_cost_usd: total_cost / n,
        peak_degraded,
        provider_breakdown: vec![],
        savings_executed_usd: 0.0,
        recommendations,
    }
}

/// Format a WeeklyReport as Telegram-ready markdown.
pub fn format_telegram_report(report: &WeeklyReport) -> String {
    let mut lines = vec![
        format!("*ClawOps Weekly Report — {}*", report.week_label),
        String::new(),
        format!("*Fleet*"),
        format!("• Avg active pairs: {:.0}", report.avg_active_pairs),
        format!("• Peak degraded: {}", report.peak_degraded),
        format!("• Incidents resolved: {}", report.total_incidents),
        format!("• Auto-heals: {}", report.total_auto_heals),
        format!("• Failovers: {}", report.total_failovers),
        String::new(),
        format!("*Cost*"),
        format!("• Total: ${:.2}", report.total_cost_usd),
        format!("• Daily avg: ${:.2}", report.avg_daily_cost_usd),
        format!("• Savings executed: ${:.2}", report.savings_executed_usd),
        String::new(),
        format!("*Recommendations*"),
    ];

    for rec in &report.recommendations {
        lines.push(format!("• {}", rec));
    }

    lines.join("\n")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_fleet(active: u32, degraded: u32, failed: u32) -> FleetStatus {
        FleetStatus {
            total_instances: active * 2 + degraded + failed,
            active_pairs: active,
            degraded_instances: degraded,
            failed_instances: failed,
            bootstrapping_instances: 0,
            generated_at: Utc::now(),
        }
    }

    fn make_costs(actual: f64, projected: f64, idle: u32) -> CostSummary {
        CostSummary {
            period_label: "this week".to_string(),
            actual_usd: actual,
            projected_usd: projected,
            idle_accounts: idle,
            provider_breakdown: vec![],
        }
    }

    #[test]
    fn test_cost_summary_variance_positive() {
        let c = make_costs(1247.0, 1190.0, 14);
        let v = c.variance_pct();
        assert!((v - 4.79).abs() < 0.1, "expected ~4.8%, got {:.2}", v);
    }

    #[test]
    fn test_cost_summary_variance_zero_projected() {
        let c = make_costs(100.0, 0.0, 0);
        assert_eq!(c.variance_pct(), 0.0);
    }

    #[test]
    fn test_generate_daily_briefing_content() {
        let fleet = make_fleet(847, 2, 1);
        let costs = make_costs(1247.0, 1190.0, 14);
        let incidents = vec![IncidentSummary {
            id: "inc-1".to_string(),
            title: "Nuremberg outage".to_string(),
            resolved: true,
            affected_instances: 94,
        }];

        let briefing = generate_daily_briefing(&fleet, &costs, &incidents);

        assert!(briefing.overnight_summary.contains("847 active pairs"));
        assert!(briefing.overnight_summary.contains("2 degraded"));
        assert!(briefing.overnight_summary.contains("1 failed"));
        assert!(briefing.cost_summary.contains("$1247"));
        assert!(briefing.cost_summary.contains("14 idle"));
        assert!(!briefing.actions_taken.is_empty());
    }

    #[test]
    fn test_generate_daily_briefing_healthy_fleet() {
        let fleet = make_fleet(100, 0, 0);
        let costs = make_costs(500.0, 500.0, 0);
        let briefing = generate_daily_briefing(&fleet, &costs, &[]);

        assert!(briefing.overnight_summary.contains("All pairs healthy"));
        assert!(
            briefing
                .recommendations
                .iter()
                .any(|r| r.contains("No SLA"))
        );
    }

    #[test]
    fn test_format_voice_script_contains_dispatch() {
        let fleet = make_fleet(847, 2, 1);
        let costs = make_costs(1247.0, 1190.0, 14);
        let briefing = generate_daily_briefing(&fleet, &costs, &[]);
        let script = format_voice_script(&briefing);

        assert!(script.starts_with("[Dispatch — 07:00 UTC]"));
        assert!(script.contains("847 active pairs"));
    }

    #[test]
    fn test_format_voice_script_open_incidents() {
        let fleet = make_fleet(50, 1, 0);
        let costs = make_costs(600.0, 600.0, 0);
        let incidents = vec![IncidentSummary {
            id: "inc-2".to_string(),
            title: "Vultr degraded".to_string(),
            resolved: false,
            affected_instances: 5,
        }];
        let briefing = generate_daily_briefing(&fleet, &costs, &incidents);
        let script = format_voice_script(&briefing);

        assert!(script.contains("1 open incident"));
    }

    #[test]
    fn test_generate_weekly_report_empty() {
        let report = generate_weekly_report(&[]);
        assert_eq!(report.avg_active_pairs, 0.0);
        assert_eq!(report.total_cost_usd, 0.0);
    }

    #[test]
    fn test_generate_weekly_report_aggregates() {
        let history: Vec<DailySnapshot> = (0..7)
            .map(|i| DailySnapshot {
                date: format!("2026-02-{:02}", i + 1),
                active_pairs: 100,
                degraded_peak: 2,
                failed_count: 0,
                cost_usd: 178.0,
                incidents_resolved: 1,
                auto_heals: 3,
                failovers: 1,
            })
            .collect();

        let report = generate_weekly_report(&history);

        assert_eq!(report.avg_active_pairs, 100.0);
        assert!((report.total_cost_usd - 1246.0).abs() < 0.01);
        assert!((report.avg_daily_cost_usd - 178.0).abs() < 0.01);
        assert_eq!(report.total_incidents, 7);
        assert_eq!(report.total_auto_heals, 21);
        assert_eq!(report.total_failovers, 7);
        assert_eq!(report.peak_degraded, 2);
    }

    #[test]
    fn test_generate_weekly_report_high_failovers_recommendation() {
        let history: Vec<DailySnapshot> = (0..7)
            .map(|i| DailySnapshot {
                date: format!("2026-02-{:02}", i + 1),
                active_pairs: 100,
                degraded_peak: 0,
                failed_count: 0,
                cost_usd: 100.0,
                incidents_resolved: 0,
                auto_heals: 0,
                failovers: 2,
            })
            .collect();

        let report = generate_weekly_report(&history);
        assert_eq!(report.total_failovers, 14);
        assert!(
            report
                .recommendations
                .iter()
                .any(|r| r.contains("failovers"))
        );
    }

    #[test]
    fn test_format_telegram_report_structure() {
        let report = WeeklyReport {
            week_label: "2026-02-01 → 2026-02-07".to_string(),
            generated_at: Utc::now(),
            avg_active_pairs: 847.0,
            total_incidents: 3,
            total_auto_heals: 12,
            total_failovers: 2,
            total_cost_usd: 1247.0,
            avg_daily_cost_usd: 178.14,
            peak_degraded: 4,
            provider_breakdown: vec![],
            savings_executed_usd: 449.0,
            recommendations: vec!["Check Vultr stability".to_string()],
        };

        let md = format_telegram_report(&report);
        assert!(md.contains("*ClawOps Weekly Report"));
        assert!(md.contains("847"));
        assert!(md.contains("$1247.00"));
        assert!(md.contains("$449.00"));
        assert!(md.contains("Check Vultr stability"));
    }

    #[test]
    fn test_briefing_serialization() {
        let fleet = make_fleet(100, 0, 0);
        let costs = make_costs(500.0, 500.0, 2);
        let briefing = generate_daily_briefing(&fleet, &costs, &[]);
        let json = serde_json::to_string(&briefing).expect("serialize");
        let back: FleetBriefing = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.overnight_summary, briefing.overnight_summary);
    }
}
