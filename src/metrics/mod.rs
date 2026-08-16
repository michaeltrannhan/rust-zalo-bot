//! Prometheus-compatible metrics with allowlisted labels only.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::PgPool;

/// Known job types for labeled series.
pub const KNOWN_JOB_TYPES: &[&str] = &[
    "outbound.deliver",
    "receipt.ingest",
    "receipt.extract",
    "schedule.emit",
    "account.delete",
    "account.export",
    "insight.narrate",
];

/// In-process counters updated by handlers (no PII labels).
#[derive(Default)]
pub struct Metrics {
    webhook_accepted_total: AtomicU64,
    webhook_duplicate_total: AtomicU64,
    webhook_unauthorized_total: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn inc_webhook_accepted(&self) {
        self.webhook_accepted_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_webhook_duplicate(&self) {
        self.webhook_duplicate_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_webhook_unauthorized(&self) {
        self.webhook_unauthorized_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Render Prometheus text exposition (gauges from SQL + in-process counters).
    pub async fn render(&self, pool: &PgPool) -> String {
        let mut body = String::new();

        if let Ok(rows) = sqlx::query_as::<_, JobStateCount>(
            "SELECT state, COUNT(*)::BIGINT AS count FROM jobs GROUP BY state",
        )
        .fetch_all(pool)
        .await
        {
            let mut queued = 0_i64;
            let mut leased = 0_i64;
            let mut dead = 0_i64;
            for row in rows {
                match row.state.as_str() {
                    "queued" => queued = row.count,
                    "leased" => leased = row.count,
                    "dead" => dead = row.count,
                    _ => {}
                }
            }
            append_gauge(&mut body, "jobs_queued", queued);
            append_gauge(&mut body, "jobs_leased", leased);
            append_gauge(&mut body, "jobs_dead", dead);
        }

        if let Ok(rows) = sqlx::query_as::<_, JobTypeCount>(
            r#"
            SELECT job_type, COUNT(*)::BIGINT AS count
            FROM jobs
            WHERE state IN ('queued', 'leased')
            GROUP BY job_type
            "#,
        )
        .fetch_all(pool)
        .await
        {
            let labeled: Vec<_> = rows
                .into_iter()
                .filter(|row| KNOWN_JOB_TYPES.contains(&row.job_type.as_str()))
                .collect();
            if !labeled.is_empty() {
                body.push_str("# TYPE jobs_active gauge\n");
                for row in labeled {
                    use std::fmt::Write;
                    let _ = writeln!(
                        body,
                        "jobs_active{{job_type=\"{}\"}} {}",
                        row.job_type, row.count
                    );
                }
            }
        }

        append_counter(
            &mut body,
            "webhook_accepted_total",
            self.webhook_accepted_total.load(Ordering::Relaxed),
        );
        append_counter(
            &mut body,
            "webhook_duplicate_total",
            self.webhook_duplicate_total.load(Ordering::Relaxed),
        );
        append_counter(
            &mut body,
            "webhook_unauthorized_total",
            self.webhook_unauthorized_total.load(Ordering::Relaxed),
        );
        if let Ok(ambiguous) = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM outbound_messages WHERE state = 'ambiguous'",
        )
        .fetch_one(pool)
        .await
        {
            append_gauge(&mut body, "outbound_ambiguous", ambiguous);
        }

        body
    }
}

#[derive(Debug, sqlx::FromRow)]
struct JobStateCount {
    state: String,
    count: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct JobTypeCount {
    job_type: String,
    count: i64,
}

fn append_gauge(body: &mut String, name: &str, value: i64) {
    use std::fmt::Write;
    let _ = writeln!(body, "# TYPE {name} gauge");
    let _ = writeln!(body, "{name} {value}");
}

fn append_counter(body: &mut String, name: &str, value: u64) {
    use std::fmt::Write;
    let _ = writeln!(body, "# TYPE {name} counter");
    let _ = writeln!(body, "{name} {value}");
}
