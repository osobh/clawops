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
        let cutoff = Utc::now() - chrono::Duration::from_std(self.retention)
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
    pub fn query(&self, name: &MetricName, range: TimeRange, limit: Option<usize>) -> Result<Vec<MetricPoint>> {
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
        if let Err(e) = store.push(&name, MetricPoint { timestamp: now, value, labels: HashMap::new() }) {
            warn!(metric = %name_str, error = %e, "failed to push metric");
        }
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
            if !name.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-')) {
                return Err(MetricsError::InvalidName(format!("invalid chars in '{name}'")));
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

        let points = store.query(&name, TimeRange::last_minutes(5), None).expect("query");
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

        let avg = store.average_over(&name, TimeRange::last_minutes(5)).expect("avg");
        assert!((avg - 20.0).abs() < 0.001);
    }
}
