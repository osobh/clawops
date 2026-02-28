//! gf-metrics — Fleet-wide metrics collection, aggregation, and reporting
//!
//! Aggregates per-instance metrics streams into fleet-level summaries that
//! the Ledger agent uses for cost analysis and the Briefer uses for daily reports.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;
use uuid::Uuid;

pub use gf_node_proto::{InstanceTier, MetricsReport, VpsProvider};

// ─── Fleet summary ────────────────────────────────────────────────────────────

/// Aggregated fleet-wide metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetSummary {
    pub snapshot_id: Uuid,
    pub captured_at: DateTime<Utc>,
    pub total_instances: u32,
    pub active_pairs: u32,
    pub degraded_instances: u32,
    pub failed_instances: u32,
    pub bootstrapping_instances: u32,

    // Provider breakdown
    pub by_provider: HashMap<String, ProviderSummary>,

    // Tier breakdown
    pub by_tier: HashMap<String, TierSummary>,

    // Cost metrics
    pub cost: CostMetrics,

    // Performance aggregates
    pub avg_health_score: f32,
    pub p50_cpu_usage: f32,
    pub p95_cpu_usage: f32,
    pub p50_mem_usage: f32,
    pub p95_mem_usage: f32,
    pub instances_above_cpu_threshold: u32,
    pub instances_above_disk_threshold: u32,

    // Operational metrics
    pub provisions_24h: u32,
    pub teardowns_24h: u32,
    pub failovers_24h: u32,
    pub heals_24h: u32,
    pub heal_success_rate_24h: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub provider: String,
    pub total_instances: u32,
    pub active_instances: u32,
    pub degraded_instances: u32,
    pub health_score: u8,
    pub monthly_cost_usd: f64,
    pub avg_provision_time_ms: u64,
    pub provision_success_rate_7d: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierSummary {
    pub tier: String,
    pub count: u32,
    pub monthly_cost_usd: f64,
    pub avg_cpu_usage: f32,
    pub avg_mem_usage: f32,
    /// Instances that could be downsized (< 20% avg utilization)
    pub downsize_candidates: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostMetrics {
    pub monthly_actual_usd: f64,
    pub monthly_projected_usd: f64,
    pub deviation_pct: f32,
    pub weekly_actual_usd: f64,
    pub weekly_projected_usd: f64,
    pub cost_per_active_account_usd: f64,

    // Waste identification
    pub idle_accounts_cost_usd: f64,     // 14+ days no activity
    pub overprovisioned_cost_usd: f64,   // < 20% avg utilization
    pub suboptimal_provider_cost_usd: f64, // cheaper alternative available

    // Provider cost breakdown
    pub by_provider: HashMap<String, f64>,
}

// ─── Idle account detection ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleAccountReport {
    pub account_id: String,
    pub instance_id: String,
    pub tier: InstanceTier,
    pub provider: String,
    pub last_activity_at: DateTime<Utc>,
    pub idle_days: u32,
    pub monthly_cost_usd: f64,
    pub recommended_action: IdleAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdleAction {
    /// 14–30 days idle — flag for review
    Flag,
    /// 30+ days idle — recommend teardown with archive
    TeardownWithArchive,
    /// Special accounts that should never be auto-teardown flagged
    Exempt,
}

// ─── Utilization-based tier recommendations ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierRecommendation {
    pub account_id: String,
    pub instance_id: String,
    pub current_tier: InstanceTier,
    pub recommended_tier: InstanceTier,
    pub avg_cpu_7d: f32,
    pub avg_mem_7d: f32,
    pub monthly_savings_usd: f64,
    pub confidence: RecommendationConfidence,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecommendationConfidence {
    /// < 3 days of data
    Low,
    /// 3–7 days of consistent data
    Medium,
    /// 7+ days of consistent data
    High,
}

// ─── Provider performance scoring ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPerformanceScore {
    pub provider: String,
    pub score: u8, // 0–100
    pub avg_provision_time_ms: u64,
    pub provision_success_rate: f32,
    pub avg_uptime_pct: f32,
    pub incident_count_30d: u32,
    pub avg_latency_ms: f32,
    pub cost_efficiency_score: f32, // performance per dollar
    pub recommendation: ProviderRecommendation,
    pub measured_over_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRecommendation {
    Primary,        // score >= 85: excellent primary choice
    PrimaryOk,      // score 75–84: acceptable primary
    StandbyOnly,    // score 65–74: use as standby, not primary
    Pause,          // score < 65: pause new provisions
    Emergency,      // active incident: no new provisions
}

// ─── Metrics aggregator ───────────────────────────────────────────────────────

pub struct MetricsAggregator {
    /// Rolling window of per-instance metrics reports (keyed by instance_id)
    reports: HashMap<String, Vec<MetricsReport>>,
    /// Rolling window size in hours
    window_hours: u32,
}

impl MetricsAggregator {
    pub fn new(window_hours: u32) -> Self {
        Self {
            reports: HashMap::new(),
            window_hours,
        }
    }

    /// Ingest a new metrics report from an instance
    pub fn ingest(&mut self, report: MetricsReport) {
        let window = self.window_hours;
        let entry = self.reports.entry(report.instance_id.clone()).or_default();
        entry.push(report);

        // Trim old entries beyond window
        let cutoff = Utc::now() - chrono::Duration::hours(window as i64);
        entry.retain(|r| r.collected_at > cutoff);
    }

    /// Compute fleet-wide summary from all ingested metrics
    pub fn compute_fleet_summary(&self) -> FleetSummary {
        info!(
            instance_count = self.reports.len(),
            "Computing fleet summary"
        );

        // TODO: implement full aggregation logic
        FleetSummary {
            snapshot_id: Uuid::new_v4(),
            captured_at: Utc::now(),
            total_instances: self.reports.len() as u32,
            active_pairs: 0,
            degraded_instances: 0,
            failed_instances: 0,
            bootstrapping_instances: 0,
            by_provider: HashMap::new(),
            by_tier: HashMap::new(),
            cost: CostMetrics {
                monthly_actual_usd: 0.0,
                monthly_projected_usd: 0.0,
                deviation_pct: 0.0,
                weekly_actual_usd: 0.0,
                weekly_projected_usd: 0.0,
                cost_per_active_account_usd: 0.0,
                idle_accounts_cost_usd: 0.0,
                overprovisioned_cost_usd: 0.0,
                suboptimal_provider_cost_usd: 0.0,
                by_provider: HashMap::new(),
            },
            avg_health_score: 0.0,
            p50_cpu_usage: 0.0,
            p95_cpu_usage: 0.0,
            p50_mem_usage: 0.0,
            p95_mem_usage: 0.0,
            instances_above_cpu_threshold: 0,
            instances_above_disk_threshold: 0,
            provisions_24h: 0,
            teardowns_24h: 0,
            failovers_24h: 0,
            heals_24h: 0,
            heal_success_rate_24h: 0.0,
        }
    }

    /// Find idle accounts (no meaningful activity in N days)
    pub fn find_idle_accounts(&self, idle_days: u32) -> Vec<IdleAccountReport> {
        let _ = idle_days;
        // TODO: query activity data and identify truly idle instances
        vec![]
    }

    /// Find over-provisioned instances (candidates for tier downgrade)
    pub fn find_downsize_candidates(&self, max_cpu_pct: f32) -> Vec<TierRecommendation> {
        let _ = max_cpu_pct;
        // TODO: analyze 7-day rolling average CPU and memory
        // Flag instances where P95 CPU < max_cpu_pct for 7+ days
        vec![]
    }

    /// Score all providers by performance over N days
    pub fn score_providers(&self, days: u32) -> Vec<ProviderPerformanceScore> {
        let _ = days;
        // TODO: aggregate provision times, success rates, uptime per provider
        vec![]
    }
}

// ─── Daily briefing data ──────────────────────────────────────────────────────

/// Structured data for the Briefer agent's daily 07:00 UTC voice note
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyBriefingData {
    pub date: chrono::NaiveDate,
    pub fleet_summary: FleetSummary,
    pub overnight_incidents: Vec<IncidentSummary>,
    pub heals_executed: u32,
    pub failovers_executed: u32,
    pub provisions_completed: u32,
    pub teardowns_completed: u32,
    pub cost_this_week_usd: f64,
    pub cost_projected_usd: f64,
    pub cost_deviation_pct: f32,
    pub top_alerts: Vec<String>,
    pub sla_breaches: u32,
    pub recommended_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentSummary {
    pub incident_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub severity: String,
    pub affected_accounts: u32,
    pub description: String,
    pub resolution: String,
}
