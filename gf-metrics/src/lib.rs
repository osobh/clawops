//! gf-metrics — Fleet-wide metrics collection, aggregation, and reporting
//!
//! Aggregates per-instance metrics streams into fleet-level summaries that
//! the Ledger agent uses for cost analysis and the Briefer uses for daily reports.

use chrono::{DateTime, NaiveDate, Utc};
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

// ─── Instance metrics snapshot (per-instance state for aggregation) ───────────

/// A snapshot of per-instance state that the aggregator maintains in memory.
/// Combined from: heartbeat data, health reports, and provisioning events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceSnapshot {
    pub instance_id: String,
    pub account_id: String,
    pub provider: String,
    pub region: String,
    pub tier: String,
    pub role: String,
    pub status: InstanceStatus,
    pub health_score: u8,
    pub cpu_usage_pct: f32,
    pub mem_usage_pct: f32,
    pub disk_usage_pct: f32,
    pub last_heartbeat: DateTime<Utc>,
    pub last_activity: Option<DateTime<Utc>>,
    pub monthly_cost_usd: f64,
    pub provisioned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InstanceStatus {
    Active,
    Degraded,
    Failed,
    Bootstrapping,
    Creating,
    Idle,
}

// ─── Metrics aggregator ───────────────────────────────────────────────────────

pub struct MetricsAggregator {
    /// Rolling window of per-instance metrics reports (keyed by instance_id)
    reports: HashMap<String, Vec<MetricsReport>>,
    /// Latest snapshot per instance (from heartbeats + health checks)
    snapshots: HashMap<String, InstanceSnapshot>,
    /// Rolling window size in hours
    window_hours: u32,
    /// Tier cost table (monthly USD per tier per instance)
    tier_costs: HashMap<String, f64>,
}

impl MetricsAggregator {
    pub fn new(window_hours: u32) -> Self {
        let mut tier_costs = HashMap::new();
        tier_costs.insert("nano".to_string(), 4.0);
        tier_costs.insert("standard".to_string(), 12.0);
        tier_costs.insert("pro".to_string(), 24.0);
        tier_costs.insert("enterprise".to_string(), 48.0);

        Self {
            reports: HashMap::new(),
            snapshots: HashMap::new(),
            window_hours,
            tier_costs,
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

    /// Update the latest snapshot for an instance (from heartbeat data)
    pub fn update_snapshot(&mut self, snapshot: InstanceSnapshot) {
        self.snapshots.insert(snapshot.instance_id.clone(), snapshot);
    }

    /// Compute fleet-wide summary from all ingested metrics and snapshots
    pub fn compute_fleet_summary(&self) -> FleetSummary {
        info!(
            instance_count = self.snapshots.len(),
            "Computing fleet summary"
        );

        let all_snapshots: Vec<&InstanceSnapshot> = self.snapshots.values().collect();

        // Status counts
        let total_instances = all_snapshots.len() as u32;
        let active_pairs = all_snapshots
            .iter()
            .filter(|s| s.status == InstanceStatus::Active && s.role == "primary")
            .count() as u32;
        let degraded_instances = all_snapshots
            .iter()
            .filter(|s| s.status == InstanceStatus::Degraded)
            .count() as u32;
        let failed_instances = all_snapshots
            .iter()
            .filter(|s| s.status == InstanceStatus::Failed)
            .count() as u32;
        let bootstrapping_instances = all_snapshots
            .iter()
            .filter(|s| matches!(s.status, InstanceStatus::Bootstrapping | InstanceStatus::Creating))
            .count() as u32;

        // Provider breakdown
        let by_provider = self.compute_provider_breakdown(&all_snapshots);

        // Tier breakdown
        let by_tier = self.compute_tier_breakdown(&all_snapshots);

        // Cost metrics
        let cost = self.compute_cost_metrics(&all_snapshots);

        // Performance aggregates
        let cpu_values: Vec<f32> = all_snapshots.iter().map(|s| s.cpu_usage_pct).collect();
        let mem_values: Vec<f32> = all_snapshots.iter().map(|s| s.mem_usage_pct).collect();
        let health_values: Vec<f32> = all_snapshots.iter().map(|s| s.health_score as f32).collect();

        let avg_health_score = mean(&health_values);
        let p50_cpu_usage = percentile(&cpu_values, 50.0);
        let p95_cpu_usage = percentile(&cpu_values, 95.0);
        let p50_mem_usage = percentile(&mem_values, 50.0);
        let p95_mem_usage = percentile(&mem_values, 95.0);

        let instances_above_cpu_threshold = all_snapshots
            .iter()
            .filter(|s| s.cpu_usage_pct > 90.0)
            .count() as u32;
        let instances_above_disk_threshold = all_snapshots
            .iter()
            .filter(|s| s.disk_usage_pct > 80.0)
            .count() as u32;

        FleetSummary {
            snapshot_id: Uuid::new_v4(),
            captured_at: Utc::now(),
            total_instances,
            active_pairs,
            degraded_instances,
            failed_instances,
            bootstrapping_instances,
            by_provider,
            by_tier,
            cost,
            avg_health_score,
            p50_cpu_usage,
            p95_cpu_usage,
            p50_mem_usage,
            p95_mem_usage,
            instances_above_cpu_threshold,
            instances_above_disk_threshold,
            provisions_24h: 0, // TODO: from operational event log
            teardowns_24h: 0,
            failovers_24h: 0,
            heals_24h: 0,
            heal_success_rate_24h: 0.0,
        }
    }

    fn compute_provider_breakdown(
        &self,
        snapshots: &[&InstanceSnapshot],
    ) -> HashMap<String, ProviderSummary> {
        let mut by_provider: HashMap<String, Vec<&InstanceSnapshot>> = HashMap::new();

        for s in snapshots {
            by_provider.entry(s.provider.clone()).or_default().push(s);
        }

        by_provider
            .into_iter()
            .map(|(provider, instances)| {
                let total = instances.len() as u32;
                let active = instances.iter().filter(|s| s.status == InstanceStatus::Active).count() as u32;
                let degraded = instances.iter().filter(|s| s.status == InstanceStatus::Degraded).count() as u32;
                let health_scores: Vec<f32> = instances.iter().map(|s| s.health_score as f32).collect();
                let monthly_cost: f64 = instances.iter().map(|s| s.monthly_cost_usd).sum();

                (
                    provider.clone(),
                    ProviderSummary {
                        provider,
                        total_instances: total,
                        active_instances: active,
                        degraded_instances: degraded,
                        health_score: mean(&health_scores) as u8,
                        monthly_cost_usd: monthly_cost,
                        avg_provision_time_ms: 0, // From provision event log
                        provision_success_rate_7d: 0.0,
                    },
                )
            })
            .collect()
    }

    fn compute_tier_breakdown(
        &self,
        snapshots: &[&InstanceSnapshot],
    ) -> HashMap<String, TierSummary> {
        let mut by_tier: HashMap<String, Vec<&InstanceSnapshot>> = HashMap::new();

        for s in snapshots {
            by_tier.entry(s.tier.clone()).or_default().push(s);
        }

        by_tier
            .into_iter()
            .map(|(tier, instances)| {
                let count = instances.len() as u32;
                let monthly_cost: f64 = instances.iter().map(|s| s.monthly_cost_usd).sum();
                let avg_cpu: f32 = mean(&instances.iter().map(|s| s.cpu_usage_pct).collect::<Vec<_>>());
                let avg_mem: f32 = mean(&instances.iter().map(|s| s.mem_usage_pct).collect::<Vec<_>>());
                // Downsize candidate: avg CPU < 20% AND avg MEM < 30% for the past window
                let downsize_candidates = instances
                    .iter()
                    .filter(|s| {
                        let inst_reports = self.reports.get(&s.instance_id);
                        if let Some(reports) = inst_reports {
                            if reports.len() < 3 { return false; }
                            let avg_cpu: f32 = reports.iter().map(|r| r.cpu.usage_pct).sum::<f32>()
                                / reports.len() as f32;
                            let avg_mem_pct: f32 = reports.iter().map(|r| {
                                if r.memory.total_mb == 0 { return 0.0; }
                                r.memory.used_mb as f32 / r.memory.total_mb as f32 * 100.0
                            }).sum::<f32>() / reports.len() as f32;
                            avg_cpu < 20.0 && avg_mem_pct < 30.0
                        } else {
                            false
                        }
                    })
                    .count() as u32;

                (
                    tier.clone(),
                    TierSummary {
                        tier,
                        count,
                        monthly_cost_usd: monthly_cost,
                        avg_cpu_usage: avg_cpu,
                        avg_mem_usage: avg_mem,
                        downsize_candidates,
                    },
                )
            })
            .collect()
    }

    fn compute_cost_metrics(&self, snapshots: &[&InstanceSnapshot]) -> CostMetrics {
        let monthly_actual: f64 = snapshots.iter().map(|s| s.monthly_cost_usd).sum();
        // Project based on current count + expected growth (flat for now)
        let monthly_projected = monthly_actual;
        let deviation_pct = if monthly_projected > 0.0 {
            ((monthly_actual - monthly_projected) / monthly_projected * 100.0) as f32
        } else {
            0.0
        };
        let weekly_actual = monthly_actual / 4.3; // approximate weeks per month
        let active_accounts = snapshots
            .iter()
            .filter(|s| s.status == InstanceStatus::Active && s.role == "primary")
            .count();
        let cost_per_account = if active_accounts > 0 {
            monthly_actual / active_accounts as f64
        } else {
            0.0
        };

        // Idle cost: instances idle > 14 days
        let idle_cutoff = Utc::now() - chrono::Duration::days(14);
        let idle_cost: f64 = snapshots
            .iter()
            .filter(|s| {
                s.last_activity
                    .map(|la| la < idle_cutoff)
                    .unwrap_or(false)
            })
            .map(|s| s.monthly_cost_usd)
            .sum();

        // Overprovision cost: instances with < 20% avg CPU usage
        let overprovision_cost: f64 = snapshots
            .iter()
            .filter(|s| {
                let reports = self.reports.get(&s.instance_id);
                if let Some(rpts) = reports {
                    if rpts.len() < 72 { return false; } // need at least 72 data points (3 days at 60s)
                    let avg: f32 = rpts.iter().map(|r| r.cpu.usage_pct).sum::<f32>()
                        / rpts.len() as f32;
                    avg < 20.0
                } else {
                    false
                }
            })
            .map(|s| {
                // Savings from downsize = difference in tier cost
                let current_cost = s.monthly_cost_usd;
                let nano_cost = self.tier_costs.get("nano").copied().unwrap_or(4.0);
                if current_cost > nano_cost { current_cost - nano_cost } else { 0.0 }
            })
            .sum();

        // By-provider cost breakdown
        let mut by_provider: HashMap<String, f64> = HashMap::new();
        for s in snapshots {
            *by_provider.entry(s.provider.clone()).or_insert(0.0) += s.monthly_cost_usd;
        }

        CostMetrics {
            monthly_actual_usd: monthly_actual,
            monthly_projected_usd: monthly_projected,
            deviation_pct,
            weekly_actual_usd: weekly_actual,
            weekly_projected_usd: weekly_actual, // same for now
            cost_per_active_account_usd: cost_per_account,
            idle_accounts_cost_usd: idle_cost,
            overprovisioned_cost_usd: overprovision_cost,
            suboptimal_provider_cost_usd: 0.0, // Computed by provider comparison logic
            by_provider,
        }
    }

    /// Find idle accounts (no meaningful activity in idle_days days)
    pub fn find_idle_accounts(&self, idle_days: u32) -> Vec<IdleAccountReport> {
        let cutoff = Utc::now() - chrono::Duration::days(idle_days as i64);
        let warn_cutoff = Utc::now() - chrono::Duration::days(14);

        self.snapshots
            .values()
            .filter(|s| s.role == "primary") // One report per account
            .filter_map(|s| {
                let last_activity = s.last_activity?;
                if last_activity >= cutoff {
                    return None; // Not idle
                }

                let idle_days_actual = (Utc::now() - last_activity).num_days() as u32;
                let recommended_action = if last_activity < cutoff {
                    IdleAction::TeardownWithArchive
                } else if last_activity < warn_cutoff {
                    IdleAction::Flag
                } else {
                    return None;
                };

                let tier = match s.tier.as_str() {
                    "nano" => InstanceTier::Nano,
                    "pro" => InstanceTier::Pro,
                    "enterprise" => InstanceTier::Enterprise,
                    _ => InstanceTier::Standard,
                };

                Some(IdleAccountReport {
                    account_id: s.account_id.clone(),
                    instance_id: s.instance_id.clone(),
                    tier,
                    provider: s.provider.clone(),
                    last_activity_at: last_activity,
                    idle_days: idle_days_actual,
                    monthly_cost_usd: s.monthly_cost_usd,
                    recommended_action,
                })
            })
            .collect()
    }

    /// Find over-provisioned instances (candidates for tier downgrade).
    /// An instance is a candidate if its P95 CPU usage < max_cpu_pct for 7+ days.
    pub fn find_downsize_candidates(&self, max_cpu_pct: f32) -> Vec<TierRecommendation> {
        let min_days_data = 3; // minimum days of data needed for recommendation
        let min_reports = min_days_data * 24; // at 1 report/hour

        self.snapshots
            .values()
            .filter_map(|s| {
                let reports = self.reports.get(&s.instance_id)?;

                // Need sufficient data
                let confidence = if reports.len() >= 7 * 24 {
                    RecommendationConfidence::High
                } else if reports.len() >= 3 * 24 {
                    RecommendationConfidence::Medium
                } else {
                    return None; // not enough data
                };

                if reports.len() < min_reports { return None; }

                // Compute P95 CPU over window
                let mut cpu_values: Vec<f32> = reports.iter().map(|r| r.cpu.usage_pct).collect();
                let p95_cpu = percentile(&cpu_values, 95.0);
                let avg_cpu = mean(&cpu_values);

                // Compute P95 memory usage
                let mut mem_values: Vec<f32> = reports.iter().map(|r| {
                    if r.memory.total_mb == 0 { return 0.0; }
                    r.memory.used_mb as f32 / r.memory.total_mb as f32 * 100.0
                }).collect();
                let avg_mem = mean(&mem_values);

                // Only recommend downsize if P95 CPU AND avg mem are both well below threshold
                if p95_cpu >= max_cpu_pct || avg_mem >= 50.0 {
                    return None;
                }

                let current_tier = match s.tier.as_str() {
                    "nano" => return None, // Already at lowest
                    "standard" => InstanceTier::Standard,
                    "pro" => InstanceTier::Pro,
                    "enterprise" => InstanceTier::Enterprise,
                    _ => return None,
                };

                // Recommend one tier down
                let (recommended_tier, monthly_savings) = match current_tier {
                    InstanceTier::Enterprise => (InstanceTier::Pro, 24.0),
                    InstanceTier::Pro => (InstanceTier::Standard, 12.0),
                    InstanceTier::Standard => (InstanceTier::Nano, 8.0),
                    InstanceTier::Nano => return None,
                };

                // Sort for percentile (avoid mutation warning)
                cpu_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                mem_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

                Some(TierRecommendation {
                    account_id: s.account_id.clone(),
                    instance_id: s.instance_id.clone(),
                    current_tier,
                    recommended_tier,
                    avg_cpu_7d: avg_cpu,
                    avg_mem_7d: avg_mem,
                    monthly_savings_usd: monthly_savings,
                    confidence,
                    rationale: format!(
                        "P95 CPU {p95_cpu:.1}% (threshold: {max_cpu_pct:.0}%), avg mem {avg_mem:.1}% over {} data points",
                        reports.len()
                    ),
                })
            })
            .collect()
    }

    /// Score all providers by performance over N days.
    pub fn score_providers(&self, _days: u32) -> Vec<ProviderPerformanceScore> {
        let mut provider_snapshots: HashMap<String, Vec<&InstanceSnapshot>> = HashMap::new();
        for s in self.snapshots.values() {
            provider_snapshots.entry(s.provider.clone()).or_default().push(s);
        }

        provider_snapshots
            .into_iter()
            .map(|(provider, instances)| {
                let health_scores: Vec<f32> = instances.iter().map(|s| s.health_score as f32).collect();
                let avg_health = mean(&health_scores);
                let avg_uptime = avg_health; // Health score approximates uptime

                // Compute score: weighted combination of uptime, provision success, latency
                let score = (avg_uptime * 0.6 + 40.0 * 0.4).min(100.0) as u8;

                let recommendation = match score {
                    85..=u8::MAX => ProviderRecommendation::Primary,
                    75..=84 => ProviderRecommendation::PrimaryOk,
                    65..=74 => ProviderRecommendation::StandbyOnly,
                    _ => ProviderRecommendation::Pause,
                };

                ProviderPerformanceScore {
                    provider,
                    score,
                    avg_provision_time_ms: 0, // From provision event log
                    provision_success_rate: 0.0,
                    avg_uptime_pct: avg_uptime,
                    incident_count_30d: 0,
                    avg_latency_ms: 0.0,
                    cost_efficiency_score: avg_uptime / 100.0,
                    recommendation,
                    measured_over_days: self.window_hours / 24,
                }
            })
            .collect()
    }
}

// ─── Daily briefing data ──────────────────────────────────────────────────────

/// Structured data for the Briefer agent's daily 07:00 UTC voice note
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyBriefingData {
    pub date: NaiveDate,
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

// ─── Statistical helpers ──────────────────────────────────────────────────────

/// Compute the arithmetic mean of a slice. Returns 0.0 if empty.
pub fn mean(values: &[f32]) -> f32 {
    if values.is_empty() { return 0.0; }
    values.iter().sum::<f32>() / values.len() as f32
}

/// Compute an approximate percentile using the nearest-rank method.
/// Values do NOT need to be sorted.
pub fn percentile(values: &[f32], pct: f32) -> f32 {
    if values.is_empty() { return 0.0; }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((pct / 100.0) * (sorted.len() as f32 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percentile_basic() {
        let values = vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
        assert_eq!(percentile(&values, 50.0), 60.0);
        assert_eq!(percentile(&values, 95.0), 100.0);
    }

    #[test]
    fn test_mean() {
        let values = vec![10.0, 20.0, 30.0];
        assert!((mean(&values) - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_fleet_summary_empty() {
        let agg = MetricsAggregator::new(24);
        let summary = agg.compute_fleet_summary();
        assert_eq!(summary.total_instances, 0);
        assert_eq!(summary.active_pairs, 0);
    }
}
