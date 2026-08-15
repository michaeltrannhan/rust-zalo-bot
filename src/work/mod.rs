//! PostgreSQL durable work queue: enqueue, claim, lease, retry, and recovery.

mod error;
mod store;
mod types;

pub use error::WorkError;
pub use store::WorkStore;
pub use types::{
    AttemptOutcome, AttemptSummary, ClaimOptions, ClaimedJob, EnqueueOutcome, EnqueueRequest,
    FailOutcome, JobState, JobSummary, VersionedPayload, is_retryable, retry_delay_secs,
};
