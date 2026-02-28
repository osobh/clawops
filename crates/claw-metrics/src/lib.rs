//! VPS fleet metrics collection for ClawOps.
//!
//! Provides a lightweight push-based metrics store for VPS health metrics
//! (CPU, memory, disk, network, OpenClaw gateway health).

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

pub use error::{MetricsError, Result};
pub use types::{MetricName, MetricPoint, TimeRange};

/// Central metric storage with retention policy.
pub struct MetricStore {
    /// Per-metric time series (deque for efficient front-pop).
    series: RwLock<HashMap<String, VecDeque<MetricPoint>>>,
    /// How long to retain data.
    retention: Duration,
}

impl MetricStore {
    /// Create a new metric store with the given retention window.
    pub fn new(retention: Duration) -> Self {
        Self {
            series: RwLock::new(HashMap::new()),
            retention,
        }
    }

    /// Push a metric point. Evicts expired data.
    pub fn push(&self, name: &MetricName, point: MetricPoint) -> Result<()> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.retention)
                .map_err(|_| MetricsError::InvalidRetention)?;

        let mut series = self.series.write();
        let deque = series.entry(name.0.clone()).or_default();

        // Evict old data
        while deque.front().is_some_and(|p| p.timestamp < cutoff) {
            deque.pop_front();
        }

        deque.push_back(point);
        Ok(())
    }

    /// Query all points in the given time range.
    pub fn query(
        &self,
        name: &MetricName,
        range: TimeRange,
        limit: Option<usize>,
    ) -> Result<Vec<MetricPoint>> {
        let series = self.series.read();
        let Some(deque) = series.get(&name.0) else {
            return Ok(vec![]);
        };

        let mut points: Vec<MetricPoint> = deque
            .iter()
            .filter(|p: &&MetricPoint| p.timestamp >= range.start && p.timestamp <= range.end)
            .cloned()
            .collect();

        if let Some(n) = limit {
            points.truncate(n);
        }

        Ok(points)
    }

    /// Get the last value for a metric.
    pub fn last_value(&self, name: &MetricName) -> Option<f64> {
        let series = self.series.read();
        series.get(&name.0)?.back().map(|p| p.value)
    }

    /// Compute average over a time range.
    pub fn average_over(&self, name: &MetricName, range: TimeRange) -> Option<f64> {
        let points = self.query(name, range, None).ok()?;
        if points.is_empty() {
            return None;
        }
        let sum: f64 = points.iter().map(|p| p.value).sum();
        Some(sum / points.len() as f64)
    }

    /// Get all known metric names.
    pub fn metric_names(&self) -> Vec<String> {
        self.series.read().keys().cloned().collect()
    }
}

/// Shared reference to a metric store (cheap to clone).
pub type SharedMetricStore = Arc<MetricStore>;

/// Helper to push a VPS health metric snapshot.
pub fn push_vps_snapshot(
    store: &MetricStore,
    instance_id: &str,
    cpu_pct: f64,
    mem_pct: f64,
    disk_pct: f64,
    health_score: f64,
) {
    let now = Utc::now();

    let metrics = [
        (format!("{instance_id}.cpu"), cpu_pct),
        (format!("{instance_id}.mem"), mem_pct),
        (format!("{instance_id}.disk"), disk_pct),
        (format!("{instance_id}.health"), health_score),
    ];

    for (name_str, value) in metrics {
        let Ok(name) = MetricName::new(&name_str) else {
            continue;
        };
        if let Err(e) = store.push(
            &name,
            MetricPoint {
                timestamp: now,
                value,
                labels: HashMap::new(),
            },
        ) {
            warn!(metric = %name_str, error = %e, "failed to push metric");
        }
    }
}

// ─── Fleet Metrics Aggregation ────────────────────────────────────────────────

/// Snapshot of a single instance for fleet aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceSnapshot {
    pub instance_id: String,
    pub provider: String,
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub disk_pct: f64,
    pub health_score: f64,
    pub monthly_cost_usd: f64,
    pub recorded_at: DateTime<Utc>,
}

/// Fleet-wide aggregated metrics computed from a set of instance snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetMetrics {
    pub total_instances: u32,
    pub avg_cpu_pct: f64,
    pub avg_mem_pct: f64,
    pub avg_disk_pct: f64,
    pub avg_health_score: f64,
    pub total_monthly_cost_usd: f64,
    pub by_provider: HashMap<String, ProviderMetrics>,
    pub computed_at: DateTime<Utc>,
}

/// Per-provider aggregated metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderMetrics {
    pub provider: String,
    pub instance_count: u32,
    pub avg_health_score: f64,
    pub avg_cpu_pct: f64,
    pub avg_mem_pct: f64,
    pub monthly_cost_usd: f64,
}

impl FleetMetrics {
    /// Compute fleet metrics from a slice of instance snapshots.
    pub fn compute(snapshots: &[InstanceSnapshot]) -> Self {
        if snapshots.is_empty() {
            return Self {
                total_instances: 0,
                avg_cpu_pct: 0.0,
                avg_mem_pct: 0.0,
                avg_disk_pct: 0.0,
                avg_health_score: 0.0,
                total_monthly_cost_usd: 0.0,
                by_provider: HashMap::new(),
                computed_at: Utc::now(),
            };
        }

        let n = snapshots.len() as f64;
        let avg_cpu = snapshots.iter().map(|s| s.cpu_pct).sum::<f64>() / n;
        let avg_mem = snapshots.iter().map(|s| s.mem_pct).sum::<f64>() / n;
        let avg_disk = snapshots.iter().map(|s| s.disk_pct).sum::<f64>() / n;
        let avg_health = snapshots.iter().map(|s| s.health_score).sum::<f64>() / n;
        let total_cost = snapshots.iter().map(|s| s.monthly_cost_usd).sum::<f64>();

        // Group by provider
        let mut by_provider: HashMap<String, Vec<&InstanceSnapshot>> = HashMap::new();
        for snap in snapshots {
            by_provider
                .entry(snap.provider.clone())
                .or_default()
                .push(snap);
        }

        let provider_metrics: HashMap<String, ProviderMetrics> = by_provider
            .iter()
            .map(|(provider, instances)| {
                let pn = instances.len() as f64;
                let pm = ProviderMetrics {
                    provider: provider.clone(),
                    instance_count: instances.len() as u32,
                    avg_health_score: instances.iter().map(|s| s.health_score).sum::<f64>() / pn,
                    avg_cpu_pct: instances.iter().map(|s| s.cpu_pct).sum::<f64>() / pn,
                    avg_mem_pct: instances.iter().map(|s| s.mem_pct).sum::<f64>() / pn,
                    monthly_cost_usd: instances.iter().map(|s| s.monthly_cost_usd).sum::<f64>(),
                };
                (provider.clone(), pm)
            })
            .collect();

        Self {
            total_instances: snapshots.len() as u32,
            avg_cpu_pct: avg_cpu,
            avg_mem_pct: avg_mem,
            avg_disk_pct: avg_disk,
            avg_health_score: avg_health,
            total_monthly_cost_usd: total_cost,
            by_provider: provider_metrics,
            computed_at: Utc::now(),
        }
    }
}

// ─── Time-Series Ring Buffer ──────────────────────────────────────────────────

/// A fixed-capacity ring buffer for metric snapshots per instance.
///
/// Holds the last `capacity` snapshots; older data is automatically evicted.
pub struct TimeSeriesBuffer {
    capacity: usize,
    buffer: VecDeque<InstanceSnapshot>,
}

impl TimeSeriesBuffer {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        Self {
            capacity,
            buffer: VecDeque::with_capacity(capacity),
        }
    }

    /// Push a snapshot, evicting the oldest if at capacity.
    pub fn push(&mut self, snapshot: InstanceSnapshot) {
        if self.buffer.len() == self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(snapshot);
    }

    /// All snapshots in insertion order (oldest first).
    pub fn snapshots(&self) -> &VecDeque<InstanceSnapshot> {
        &self.buffer
    }

    /// Number of snapshots currently stored.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Average health score over stored snapshots.
    pub fn avg_health_score(&self) -> Option<f64> {
        if self.buffer.is_empty() {
            return None;
        }
        let sum: f64 = self.buffer.iter().map(|s| s.health_score).sum();
        Some(sum / self.buffer.len() as f64)
    }

    /// Average CPU % over stored snapshots.
    pub fn avg_cpu_pct(&self) -> Option<f64> {
        if self.buffer.is_empty() {
            return None;
        }
        let sum: f64 = self.buffer.iter().map(|s| s.cpu_pct).sum();
        Some(sum / self.buffer.len() as f64)
    }
}

// ─── Cost Tracker ─────────────────────────────────────────────────────────────

/// Per-instance cost record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceCost {
    pub instance_id: String,
    pub provider: String,
    pub monthly_cost_usd: f64,
    pub hours_active: f64,
    pub projected_monthly_usd: f64,
    pub actual_spend_usd: f64,
}

/// Fleet cost tracker: projected vs actual spend per provider and per instance.
pub struct CostTracker {
    instances: HashMap<String, InstanceCost>,
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
        }
    }

    /// Register or update an instance's cost record.
    pub fn track(&mut self, cost: InstanceCost) {
        self.instances.insert(cost.instance_id.clone(), cost);
    }

    /// Total projected monthly cost across all tracked instances.
    pub fn total_projected_monthly(&self) -> f64 {
        self.instances
            .values()
            .map(|c| c.projected_monthly_usd)
            .sum()
    }

    /// Total actual spend across all tracked instances.
    pub fn total_actual_spend(&self) -> f64 {
        self.instances.values().map(|c| c.actual_spend_usd).sum()
    }

    /// Per-provider cost summary.
    pub fn by_provider(&self) -> HashMap<String, f64> {
        let mut result: HashMap<String, f64> = HashMap::new();
        for cost in self.instances.values() {
            *result.entry(cost.provider.clone()).or_default() += cost.actual_spend_usd;
        }
        result
    }

    /// Get cost record for a specific instance.
    pub fn get(&self, instance_id: &str) -> Option<&InstanceCost> {
        self.instances.get(instance_id)
    }

    /// All tracked instances.
    pub fn all(&self) -> Vec<&InstanceCost> {
        self.instances.values().collect()
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum MetricsError {
        #[error("invalid metric name: {0}")]
        InvalidName(String),
        #[error("invalid retention duration")]
        InvalidRetention,
    }

    pub type Result<T> = std::result::Result<T, MetricsError>;
}

pub mod types {
    use super::*;

    /// A validated metric name (non-empty, alphanumeric + dots/underscores/hyphens).
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct MetricName(pub String);

    impl MetricName {
        pub fn new(name: &str) -> Result<Self> {
            if name.is_empty() {
                return Err(MetricsError::InvalidName("empty name".to_string()));
            }
            if !name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err(MetricsError::InvalidName(format!(
                    "invalid chars in '{name}'"
                )));
            }
            Ok(Self(name.to_string()))
        }
    }

    /// A single metric data point.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MetricPoint {
        pub timestamp: DateTime<Utc>,
        pub value: f64,
        pub labels: HashMap<String, String>,
    }

    impl MetricPoint {
        pub fn now(value: f64) -> Self {
            Self {
                timestamp: Utc::now(),
                value,
                labels: HashMap::new(),
            }
        }

        pub fn label(mut self, key: &str, value: &str) -> Self {
            self.labels.insert(key.to_string(), value.to_string());
            self
        }
    }

    /// A time range for querying metrics.
    #[derive(Debug, Clone)]
    pub struct TimeRange {
        pub start: DateTime<Utc>,
        pub end: DateTime<Utc>,
    }

    impl TimeRange {
        pub fn last_minutes(minutes: i64) -> Self {
            let end = Utc::now();
            let start = end - chrono::Duration::minutes(minutes);
            Self { start, end }
        }

        pub fn last_hours(hours: i64) -> Self {
            let end = Utc::now();
            let start = end - chrono::Duration::hours(hours);
            Self { start, end }
        }

        pub fn last_days(days: i64) -> Self {
            let end = Utc::now();
            let start = end - chrono::Duration::days(days);
            Self { start, end }
        }
    }

    pub use super::error::{MetricsError, Result};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_name_valid() {
        assert!(MetricName::new("cpu.usage").is_ok());
        assert!(MetricName::new("instance-1.mem_pct").is_ok());
    }

    #[test]
    fn test_metric_name_invalid() {
        assert!(MetricName::new("").is_err());
        assert!(MetricName::new("name with spaces").is_err());
    }

    #[test]
    fn test_push_and_query() {
        let store = MetricStore::new(Duration::from_secs(3600));
        let name = MetricName::new("test.metric").expect("valid name");

        store.push(&name, MetricPoint::now(42.0)).expect("push");
        store.push(&name, MetricPoint::now(50.0)).expect("push");

        let points = store
            .query(&name, TimeRange::last_minutes(5), None)
            .expect("query");
        assert_eq!(points.len(), 2);
    }

    #[test]
    fn test_last_value() {
        let store = MetricStore::new(Duration::from_secs(3600));
        let name = MetricName::new("test.last").expect("valid name");

        assert!(store.last_value(&name).is_none());

        store.push(&name, MetricPoint::now(10.0)).expect("push");
        store.push(&name, MetricPoint::now(20.0)).expect("push");

        assert_eq!(store.last_value(&name), Some(20.0));
    }

    #[test]
    fn test_average_over() {
        let store = MetricStore::new(Duration::from_secs(3600));
        let name = MetricName::new("test.avg").expect("valid name");

        store.push(&name, MetricPoint::now(10.0)).expect("push");
        store.push(&name, MetricPoint::now(20.0)).expect("push");
        store.push(&name, MetricPoint::now(30.0)).expect("push");

        let avg = store
            .average_over(&name, TimeRange::last_minutes(5))
            .expect("avg");
        assert!((avg - 20.0).abs() < 0.001);
    }

    // ─── FleetMetrics tests ───────────────────────────────────────────────────

    fn make_snapshot(
        instance_id: &str,
        provider: &str,
        cpu: f64,
        mem: f64,
        health: f64,
        cost: f64,
    ) -> InstanceSnapshot {
        InstanceSnapshot {
            instance_id: instance_id.to_string(),
            provider: provider.to_string(),
            cpu_pct: cpu,
            mem_pct: mem,
            disk_pct: 30.0,
            health_score: health,
            monthly_cost_usd: cost,
            recorded_at: Utc::now(),
        }
    }

    #[test]
    fn test_fleet_metrics_empty() {
        let fm = FleetMetrics::compute(&[]);
        assert_eq!(fm.total_instances, 0);
        assert_eq!(fm.avg_cpu_pct, 0.0);
        assert!(fm.by_provider.is_empty());
    }

    #[test]
    fn test_fleet_metrics_aggregation() {
        let snapshots = vec![
            make_snapshot("i-1", "hetzner", 20.0, 40.0, 90.0, 12.0),
            make_snapshot("i-2", "hetzner", 40.0, 60.0, 70.0, 12.0),
            make_snapshot("i-3", "vultr", 30.0, 50.0, 80.0, 15.0),
        ];
        let fm = FleetMetrics::compute(&snapshots);

        assert_eq!(fm.total_instances, 3);
        assert!((fm.avg_cpu_pct - 30.0).abs() < 0.001);
        assert!((fm.avg_mem_pct - 50.0).abs() < 0.001);
        assert!((fm.avg_health_score - 80.0).abs() < 0.001);
        assert!((fm.total_monthly_cost_usd - 39.0).abs() < 0.001);

        assert!(fm.by_provider.contains_key("hetzner"));
        assert!(fm.by_provider.contains_key("vultr"));
        assert_eq!(fm.by_provider["hetzner"].instance_count, 2);
        assert_eq!(fm.by_provider["vultr"].instance_count, 1);
    }

    #[test]
    fn test_fleet_metrics_provider_avg_health() {
        let snapshots = vec![
            make_snapshot("i-1", "hetzner", 10.0, 10.0, 80.0, 12.0),
            make_snapshot("i-2", "hetzner", 10.0, 10.0, 60.0, 12.0),
        ];
        let fm = FleetMetrics::compute(&snapshots);
        let hetzner = &fm.by_provider["hetzner"];
        assert!((hetzner.avg_health_score - 70.0).abs() < 0.001);
    }

    // ─── TimeSeriesBuffer tests ───────────────────────────────────────────────

    #[test]
    fn test_time_series_buffer_capacity() {
        let mut buf = TimeSeriesBuffer::new(3);
        for i in 0..5u32 {
            buf.push(make_snapshot(
                &format!("i-{i}"),
                "hetzner",
                10.0,
                10.0,
                90.0,
                12.0,
            ));
        }
        // Only last 3 kept
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn test_time_series_buffer_avg_health() {
        let mut buf = TimeSeriesBuffer::new(10);
        buf.push(make_snapshot("i-1", "hetzner", 10.0, 10.0, 80.0, 12.0));
        buf.push(make_snapshot("i-1", "hetzner", 10.0, 10.0, 60.0, 12.0));
        buf.push(make_snapshot("i-1", "hetzner", 10.0, 10.0, 70.0, 12.0));
        let avg = buf.avg_health_score().expect("has data");
        assert!((avg - 70.0).abs() < 0.001);
    }

    #[test]
    fn test_time_series_buffer_empty() {
        let buf = TimeSeriesBuffer::new(5);
        assert!(buf.is_empty());
        assert!(buf.avg_health_score().is_none());
        assert!(buf.avg_cpu_pct().is_none());
    }

    // ─── CostTracker tests ────────────────────────────────────────────────────

    #[test]
    fn test_cost_tracker_empty() {
        let ct = CostTracker::new();
        assert_eq!(ct.total_projected_monthly(), 0.0);
        assert_eq!(ct.total_actual_spend(), 0.0);
        assert!(ct.by_provider().is_empty());
    }

    #[test]
    fn test_cost_tracker_tracks_instances() {
        let mut ct = CostTracker::new();
        ct.track(InstanceCost {
            instance_id: "i-1".to_string(),
            provider: "hetzner".to_string(),
            monthly_cost_usd: 12.0,
            hours_active: 720.0,
            projected_monthly_usd: 12.0,
            actual_spend_usd: 6.0, // half month
        });
        ct.track(InstanceCost {
            instance_id: "i-2".to_string(),
            provider: "vultr".to_string(),
            monthly_cost_usd: 15.0,
            hours_active: 360.0,
            projected_monthly_usd: 15.0,
            actual_spend_usd: 7.5,
        });

        assert!((ct.total_projected_monthly() - 27.0).abs() < 0.001);
        assert!((ct.total_actual_spend() - 13.5).abs() < 0.001);
        let by_prov = ct.by_provider();
        assert!((by_prov["hetzner"] - 6.0).abs() < 0.001);
        assert!((by_prov["vultr"] - 7.5).abs() < 0.001);
    }

    #[test]
    fn test_cost_tracker_get_instance() {
        let mut ct = CostTracker::new();
        ct.track(InstanceCost {
            instance_id: "i-1".to_string(),
            provider: "hetzner".to_string(),
            monthly_cost_usd: 12.0,
            hours_active: 100.0,
            projected_monthly_usd: 12.0,
            actual_spend_usd: 1.67,
        });
        assert!(ct.get("i-1").is_some());
        assert!(ct.get("i-99").is_none());
    }
}
