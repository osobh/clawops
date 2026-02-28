//! Cost analysis engine for the Ledger agent.
//!
//! Detects waste, projects costs, recommends optimisations, and produces
//! provider comparisons.  Matches the PRD §4.1 cost dialogue pattern:
//! "Three categories: (1) 31 idle accounts — $341/month..."

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use claw_proto::{FleetStatus, InstanceTier, VpsProvider};
use serde::{Deserialize, Serialize};

// ─── Provider stats ───────────────────────────────────────────────────────────

/// Aggregated performance + cost data for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStats {
    pub provider: VpsProvider,
    pub instance_count: u32,
    pub avg_provision_time_secs: f64,
    pub provision_failure_rate_pct: f64,
    pub avg_health_score: f64,
    pub cost_per_instance_usd: f64,
    pub period_days: u32,
}

// ─── Waste report ─────────────────────────────────────────────────────────────

/// An idle account: no activity for >= 14 days.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleAccount {
    pub account_id: String,
    pub last_activity: DateTime<Utc>,
    pub idle_days: u32,
    pub monthly_cost_usd: f64,
}

/// An oversized instance: resource usage consistently < 20% of capacity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OversizedInstance {
    pub instance_id: String,
    pub account_id: String,
    pub current_tier: InstanceTier,
    pub recommended_tier: InstanceTier,
    pub avg_cpu_pct: f64,
    pub avg_mem_pct: f64,
    pub monthly_savings_usd: f64,
}

/// A provider arbitrage opportunity: same workload is cheaper elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderArbitrage {
    pub instance_id: String,
    pub current_provider: VpsProvider,
    pub cheaper_provider: VpsProvider,
    pub current_monthly_usd: f64,
    pub alternative_monthly_usd: f64,
    pub monthly_savings_usd: f64,
}

/// Complete waste detection report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasteReport {
    pub generated_at: DateTime<Utc>,
    pub idle_accounts: Vec<IdleAccount>,
    pub oversized_instances: Vec<OversizedInstance>,
    pub provider_arbitrage: Vec<ProviderArbitrage>,
    pub total_recoverable_monthly_usd: f64,
}

impl WasteReport {
    /// Human-readable three-category summary (matches PRD §4.1).
    pub fn summary(&self) -> String {
        let idle_cost: f64 = self.idle_accounts.iter().map(|a| a.monthly_cost_usd).sum();
        let oversize_savings: f64 = self
            .oversized_instances
            .iter()
            .map(|o| o.monthly_savings_usd)
            .sum();
        let arb_savings: f64 = self
            .provider_arbitrage
            .iter()
            .map(|a| a.monthly_savings_usd)
            .sum();

        format!(
            "Three categories: (1) {} idle accounts (14+ days no activity) — ${:.0}/month. \
             Recommend teardown with 30-day archive. \
             (2) {} accounts on oversized tier with low usage (< 20% CPU/RAM) — ${:.0}/month savings if downsized. \
             (3) {} provider arbitrage opportunities — ${:.0}/month. \
             Total recoverable: ~${:.0}/month.",
            self.idle_accounts.len(),
            idle_cost,
            self.oversized_instances.len(),
            oversize_savings,
            self.provider_arbitrage.len(),
            arb_savings,
            self.total_recoverable_monthly_usd,
        )
    }
}

// ─── Cost projection ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostProjection {
    pub generated_at: DateTime<Utc>,
    pub period_days: u32,
    pub current_daily_usd: f64,
    pub projected_total_usd: f64,
    pub actual_to_date_usd: f64,
    pub variance_pct: f64,
    pub trajectory: CostTrajectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostTrajectory {
    /// Spend trending down relative to projection.
    BelowBudget,
    /// Spend within ±5% of projection.
    OnTrack,
    /// Spend trending above projection (5–15%).
    Elevated,
    /// Spend > 15% above projection — trigger Ledger alert.
    Anomaly,
}

// ─── Optimisation recommendations ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OptimizationType {
    /// Downsize to a smaller tier.
    Downsize {
        from_tier: InstanceTier,
        to_tier: InstanceTier,
    },
    /// Teardown idle instance (archive first).
    Teardown { idle_days: u32 },
    /// Migrate to a cheaper provider.
    Migrate {
        from_provider: VpsProvider,
        to_provider: VpsProvider,
    },
    /// Archive data and suspend instance.
    Archive { last_active_days: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Optimization {
    pub instance_id: String,
    pub account_id: String,
    pub optimization_type: OptimizationType,
    pub estimated_savings_monthly_usd: f64,
    pub confidence: OptimizationConfidence,
    pub requires_confirmation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptimizationConfidence {
    High,
    Medium,
    Low,
}

// ─── Provider comparison ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderComparisonEntry {
    pub provider: VpsProvider,
    pub instance_count: u32,
    pub avg_health_score: f64,
    pub avg_provision_secs: f64,
    pub failure_rate_pct: f64,
    pub cost_per_instance_usd: f64,
    pub overall_score: f64,
    pub recommendation: ProviderRecommendation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRecommendation {
    /// Best option for primary provisioning.
    PreferForPrimary,
    /// Good as standby.
    GoodForStandby,
    /// Acceptable; monitor closely.
    Acceptable,
    /// Below threshold; pause new provisioning.
    PausePrimary,
    /// Do not use.
    Avoid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderComparison {
    pub generated_at: DateTime<Utc>,
    pub entries: Vec<ProviderComparisonEntry>,
    pub recommended_primary: VpsProvider,
    pub recommended_standby: VpsProvider,
}

// ─── Cost Engine ──────────────────────────────────────────────────────────────

/// The Ledger agent's core analysis engine.
pub struct CostEngine;

impl CostEngine {
    /// Analyse waste in the fleet (idle, oversized, arbitrage).
    pub fn analyze_waste(fleet: &FleetStatus, accounts: &[AccountActivity]) -> WasteReport {
        let idle_accounts: Vec<IdleAccount> = accounts
            .iter()
            .filter(|a| a.idle_days >= 14)
            .map(|a| IdleAccount {
                account_id: a.account_id.clone(),
                last_activity: a.last_activity,
                idle_days: a.idle_days,
                monthly_cost_usd: a.monthly_cost_usd,
            })
            .collect();

        let oversized_instances: Vec<OversizedInstance> = accounts
            .iter()
            .filter(|a| a.avg_cpu_pct < 20.0 && a.avg_mem_pct < 20.0)
            .filter(|a| a.current_tier != InstanceTier::Nano)
            .map(|a| {
                let recommended_tier = downsize_tier(&a.current_tier);
                OversizedInstance {
                    instance_id: a.instance_id.clone(),
                    account_id: a.account_id.clone(),
                    current_tier: a.current_tier,
                    recommended_tier,
                    avg_cpu_pct: a.avg_cpu_pct,
                    avg_mem_pct: a.avg_mem_pct,
                    monthly_savings_usd: a.monthly_cost_usd * 0.40,
                }
            })
            .collect();

        // Provider arbitrage: stub — in production this queries live pricing APIs
        let provider_arbitrage = vec![];

        let idle_cost: f64 = idle_accounts.iter().map(|a| a.monthly_cost_usd).sum();
        let oversize_savings: f64 = oversized_instances
            .iter()
            .map(|o| o.monthly_savings_usd)
            .sum();

        let total_recoverable = idle_cost + oversize_savings;

        let _ = fleet;
        WasteReport {
            generated_at: Utc::now(),
            idle_accounts,
            oversized_instances,
            provider_arbitrage,
            total_recoverable_monthly_usd: total_recoverable,
        }
    }

    /// Project costs over the next `days` days based on current burn rate.
    pub fn project_costs(
        fleet: &FleetStatus,
        days: u32,
        current_daily_usd: f64,
        actual_to_date_usd: f64,
    ) -> CostProjection {
        let projected_total = current_daily_usd * days as f64;
        let variance_pct = if projected_total == 0.0 {
            0.0
        } else {
            ((actual_to_date_usd - projected_total) / projected_total) * 100.0
        };

        let trajectory = classify_trajectory(variance_pct);

        let _ = fleet;
        CostProjection {
            generated_at: Utc::now(),
            period_days: days,
            current_daily_usd,
            projected_total_usd: projected_total,
            actual_to_date_usd,
            variance_pct,
            trajectory,
        }
    }

    /// Generate actionable optimisation recommendations.
    pub fn recommend_optimizations(
        fleet: &FleetStatus,
        accounts: &[AccountActivity],
    ) -> Vec<Optimization> {
        let mut opts = Vec::new();

        for account in accounts {
            // Teardown idle (>= 14 days)
            if account.idle_days >= 14 {
                opts.push(Optimization {
                    instance_id: account.instance_id.clone(),
                    account_id: account.account_id.clone(),
                    optimization_type: OptimizationType::Teardown {
                        idle_days: account.idle_days,
                    },
                    estimated_savings_monthly_usd: account.monthly_cost_usd,
                    confidence: OptimizationConfidence::High,
                    requires_confirmation: account.monthly_cost_usd > 50.0,
                });
            }

            // Downsize oversized (< 20% usage, not nano)
            if account.avg_cpu_pct < 20.0
                && account.avg_mem_pct < 20.0
                && account.current_tier != InstanceTier::Nano
            {
                let savings = account.monthly_cost_usd * 0.40;
                opts.push(Optimization {
                    instance_id: account.instance_id.clone(),
                    account_id: account.account_id.clone(),
                    optimization_type: OptimizationType::Downsize {
                        from_tier: account.current_tier,
                        to_tier: downsize_tier(&account.current_tier),
                    },
                    estimated_savings_monthly_usd: savings,
                    confidence: OptimizationConfidence::High,
                    requires_confirmation: false,
                });
            }
        }

        let _ = fleet;
        opts
    }

    /// Compare providers side-by-side on cost and performance.
    pub fn compare_providers(providers: &[ProviderStats]) -> ProviderComparison {
        let mut entries: Vec<ProviderComparisonEntry> = providers
            .iter()
            .map(|p| {
                let overall_score = compute_provider_score(p);
                let recommendation = classify_provider(overall_score, p.provision_failure_rate_pct);
                ProviderComparisonEntry {
                    provider: p.provider,
                    instance_count: p.instance_count,
                    avg_health_score: p.avg_health_score,
                    avg_provision_secs: p.avg_provision_time_secs,
                    failure_rate_pct: p.provision_failure_rate_pct,
                    cost_per_instance_usd: p.cost_per_instance_usd,
                    overall_score,
                    recommendation,
                }
            })
            .collect();

        // Sort: highest score first
        entries.sort_by(|a, b| b.overall_score.partial_cmp(&a.overall_score).unwrap());

        let recommended_primary = entries
            .iter()
            .find(|e| matches!(e.recommendation, ProviderRecommendation::PreferForPrimary))
            .map(|e| e.provider)
            .unwrap_or(VpsProvider::Hetzner);

        let recommended_standby = entries
            .iter()
            .find(|e| {
                e.provider != recommended_primary
                    && matches!(
                        e.recommendation,
                        ProviderRecommendation::PreferForPrimary
                            | ProviderRecommendation::GoodForStandby
                    )
            })
            .map(|e| e.provider)
            .unwrap_or(VpsProvider::Vultr);

        ProviderComparison {
            generated_at: Utc::now(),
            entries,
            recommended_primary,
            recommended_standby,
        }
    }
}

// ─── Input types ──────────────────────────────────────────────────────────────

/// Per-account activity record used as input to cost analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountActivity {
    pub account_id: String,
    pub instance_id: String,
    pub current_tier: InstanceTier,
    pub provider: VpsProvider,
    pub last_activity: DateTime<Utc>,
    pub idle_days: u32,
    pub avg_cpu_pct: f64,
    pub avg_mem_pct: f64,
    pub monthly_cost_usd: f64,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn downsize_tier(tier: &InstanceTier) -> InstanceTier {
    match tier {
        InstanceTier::Enterprise => InstanceTier::Pro,
        InstanceTier::Pro => InstanceTier::Standard,
        InstanceTier::Standard => InstanceTier::Nano,
        InstanceTier::Nano => InstanceTier::Nano,
    }
}

fn classify_trajectory(variance_pct: f64) -> CostTrajectory {
    if variance_pct < -5.0 {
        CostTrajectory::BelowBudget
    } else if variance_pct <= 5.0 {
        CostTrajectory::OnTrack
    } else if variance_pct <= 15.0 {
        CostTrajectory::Elevated
    } else {
        CostTrajectory::Anomaly
    }
}

/// Score a provider 0–100 based on health, speed, failure rate, and cost.
fn compute_provider_score(p: &ProviderStats) -> f64 {
    let health_component = p.avg_health_score * 0.40;
    let speed_component = (600.0_f64 - p.avg_provision_time_secs.min(600.0)) / 600.0 * 100.0 * 0.30;
    let reliability_component = (100.0 - p.provision_failure_rate_pct.min(100.0)) * 0.20;
    let cost_component = (20.0_f64 - p.cost_per_instance_usd.min(20.0)) / 20.0 * 100.0 * 0.10;

    (health_component + speed_component + reliability_component + cost_component).clamp(0.0, 100.0)
}

fn classify_provider(score: f64, failure_rate_pct: f64) -> ProviderRecommendation {
    if failure_rate_pct > 10.0 {
        return ProviderRecommendation::Avoid;
    }
    if score >= 80.0 {
        ProviderRecommendation::PreferForPrimary
    } else if score >= 65.0 {
        ProviderRecommendation::GoodForStandby
    } else if score >= 50.0 {
        ProviderRecommendation::Acceptable
    } else if score >= 30.0 {
        ProviderRecommendation::PausePrimary
    } else {
        ProviderRecommendation::Avoid
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn make_fleet() -> FleetStatus {
        FleetStatus {
            total_instances: 100,
            active_pairs: 50,
            degraded_instances: 2,
            failed_instances: 0,
            bootstrapping_instances: 0,
            generated_at: Utc::now(),
        }
    }

    fn make_account(
        idle_days: u32,
        cpu: f64,
        mem: f64,
        tier: InstanceTier,
        cost: f64,
    ) -> AccountActivity {
        AccountActivity {
            account_id: format!("acc-{}", idle_days),
            instance_id: format!("i-{}", idle_days),
            current_tier: tier,
            provider: VpsProvider::Hetzner,
            last_activity: Utc::now() - Duration::days(idle_days as i64),
            idle_days,
            avg_cpu_pct: cpu,
            avg_mem_pct: mem,
            monthly_cost_usd: cost,
        }
    }

    // ─── WasteReport ────────────────────────────────────────────────────────

    #[test]
    fn test_analyze_waste_detects_idle() {
        let fleet = make_fleet();
        let accounts = vec![
            make_account(20, 50.0, 50.0, InstanceTier::Standard, 11.0),
            make_account(5, 50.0, 50.0, InstanceTier::Standard, 11.0),
        ];
        let report = CostEngine::analyze_waste(&fleet, &accounts);
        assert_eq!(report.idle_accounts.len(), 1);
        assert_eq!(report.idle_accounts[0].idle_days, 20);
    }

    #[test]
    fn test_analyze_waste_detects_oversized() {
        let fleet = make_fleet();
        let accounts = vec![
            make_account(0, 10.0, 10.0, InstanceTier::Standard, 11.0),
            make_account(0, 60.0, 60.0, InstanceTier::Standard, 11.0),
        ];
        let report = CostEngine::analyze_waste(&fleet, &accounts);
        assert_eq!(report.oversized_instances.len(), 1);
        assert_eq!(
            report.oversized_instances[0].recommended_tier,
            InstanceTier::Nano
        );
    }

    #[test]
    fn test_analyze_waste_nano_not_oversized() {
        let fleet = make_fleet();
        let accounts = vec![make_account(0, 5.0, 5.0, InstanceTier::Nano, 5.0)];
        let report = CostEngine::analyze_waste(&fleet, &accounts);
        // Nano cannot be downsized further
        assert_eq!(report.oversized_instances.len(), 0);
    }

    #[test]
    fn test_waste_report_summary_format() {
        let fleet = make_fleet();
        let accounts: Vec<AccountActivity> = (0..31)
            .map(|i| {
                make_account(
                    20,
                    50.0,
                    50.0,
                    InstanceTier::Standard,
                    11.0 + i as f64 * 0.1,
                )
            })
            .chain((0..18).map(|_| make_account(0, 10.0, 10.0, InstanceTier::Standard, 6.0)))
            .collect();
        let report = CostEngine::analyze_waste(&fleet, &accounts);
        let summary = report.summary();
        assert!(summary.contains("Three categories:"));
        assert!(summary.contains("idle accounts"));
        assert!(summary.contains("oversized"));
    }

    #[test]
    fn test_waste_report_total_recoverable() {
        let fleet = make_fleet();
        let accounts = vec![
            make_account(20, 50.0, 50.0, InstanceTier::Standard, 10.0),
            make_account(0, 10.0, 10.0, InstanceTier::Standard, 10.0),
        ];
        let report = CostEngine::analyze_waste(&fleet, &accounts);
        // idle: $10 + oversize savings: $10 * 0.40 = $4 → total $14
        assert!((report.total_recoverable_monthly_usd - 14.0).abs() < 0.01);
    }

    // ─── CostProjection ─────────────────────────────────────────────────────

    #[test]
    fn test_project_costs_on_track() {
        let fleet = make_fleet();
        let proj = CostEngine::project_costs(&fleet, 30, 40.0, 40.0 * 30.0);
        assert_eq!(proj.trajectory, CostTrajectory::OnTrack);
        assert!((proj.variance_pct).abs() < 0.01);
    }

    #[test]
    fn test_project_costs_anomaly() {
        let fleet = make_fleet();
        // actual > 15% above projected
        let daily = 40.0;
        let actual = daily * 30.0 * 1.20; // 20% over
        let proj = CostEngine::project_costs(&fleet, 30, daily, actual);
        assert_eq!(proj.trajectory, CostTrajectory::Anomaly);
    }

    #[test]
    fn test_project_costs_below_budget() {
        let fleet = make_fleet();
        let daily = 40.0;
        let actual = daily * 30.0 * 0.90; // 10% under
        let proj = CostEngine::project_costs(&fleet, 30, daily, actual);
        assert_eq!(proj.trajectory, CostTrajectory::BelowBudget);
    }

    #[test]
    fn test_project_costs_elevated() {
        let fleet = make_fleet();
        let daily = 40.0;
        let actual = daily * 30.0 * 1.10; // 10% over
        let proj = CostEngine::project_costs(&fleet, 30, daily, actual);
        assert_eq!(proj.trajectory, CostTrajectory::Elevated);
    }

    // ─── Optimizations ──────────────────────────────────────────────────────

    #[test]
    fn test_recommend_optimizations_teardown() {
        let fleet = make_fleet();
        let accounts = vec![make_account(20, 50.0, 50.0, InstanceTier::Standard, 11.0)];
        let opts = CostEngine::recommend_optimizations(&fleet, &accounts);
        assert!(
            opts.iter()
                .any(|o| matches!(o.optimization_type, OptimizationType::Teardown { .. }))
        );
    }

    #[test]
    fn test_recommend_optimizations_downsize() {
        let fleet = make_fleet();
        let accounts = vec![make_account(0, 10.0, 10.0, InstanceTier::Pro, 20.0)];
        let opts = CostEngine::recommend_optimizations(&fleet, &accounts);
        assert!(
            opts.iter()
                .any(|o| matches!(o.optimization_type, OptimizationType::Downsize { .. }))
        );
    }

    #[test]
    fn test_recommend_optimizations_savings_positive() {
        let fleet = make_fleet();
        let accounts = vec![make_account(0, 10.0, 10.0, InstanceTier::Standard, 10.0)];
        let opts = CostEngine::recommend_optimizations(&fleet, &accounts);
        for opt in &opts {
            assert!(opt.estimated_savings_monthly_usd > 0.0);
        }
    }

    // ─── Provider Comparison ────────────────────────────────────────────────

    fn make_provider(
        provider: VpsProvider,
        health: f64,
        provision_secs: f64,
        failure_rate: f64,
        cost: f64,
    ) -> ProviderStats {
        ProviderStats {
            provider,
            instance_count: 100,
            avg_provision_time_secs: provision_secs,
            provision_failure_rate_pct: failure_rate,
            avg_health_score: health,
            cost_per_instance_usd: cost,
            period_days: 7,
        }
    }

    #[test]
    fn test_compare_providers_ranks_correctly() {
        let providers = vec![
            make_provider(VpsProvider::Hetzner, 95.0, 252.0, 0.5, 5.0),
            make_provider(VpsProvider::Vultr, 88.0, 400.0, 1.0, 7.0),
            make_provider(VpsProvider::Hostinger, 71.0, 582.0, 3.0, 9.0),
        ];
        let cmp = CostEngine::compare_providers(&providers);
        // Hetzner should rank first
        assert_eq!(cmp.entries[0].provider, VpsProvider::Hetzner);
        assert_eq!(cmp.recommended_primary, VpsProvider::Hetzner);
    }

    #[test]
    fn test_compare_providers_avoid_high_failure() {
        let providers = vec![make_provider(VpsProvider::Contabo, 70.0, 300.0, 15.0, 4.0)];
        let cmp = CostEngine::compare_providers(&providers);
        assert_eq!(cmp.entries[0].recommendation, ProviderRecommendation::Avoid);
    }

    #[test]
    fn test_downsize_tier_chain() {
        assert_eq!(downsize_tier(&InstanceTier::Enterprise), InstanceTier::Pro);
        assert_eq!(downsize_tier(&InstanceTier::Pro), InstanceTier::Standard);
        assert_eq!(downsize_tier(&InstanceTier::Standard), InstanceTier::Nano);
        assert_eq!(downsize_tier(&InstanceTier::Nano), InstanceTier::Nano);
    }

    #[test]
    fn test_provider_comparison_serialization() {
        let providers = vec![make_provider(VpsProvider::Hetzner, 95.0, 252.0, 0.5, 5.0)];
        let cmp = CostEngine::compare_providers(&providers);
        let json = serde_json::to_string(&cmp).expect("serialize");
        let back: ProviderComparison = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.entries.len(), 1);
    }
}
