//! Schedule frequency and period types.

use chrono::{DateTime, Utc};

/// Supported automatic summary cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
}

impl Frequency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "daily" | "ngay" => Some(Self::Daily),
            "weekly" | "tuan" => Some(Self::Weekly),
            "monthly" | "thang" => Some(Self::Monthly),
            _ => None,
        }
    }
}

/// Half-open spending interval `[start, end)` in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}
