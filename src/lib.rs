//! zl-expense library — walking-skeleton core for the Zalo expense bot.

pub mod account;
pub mod cli;
pub mod config;
pub mod conversation;
pub mod db;
pub mod error;
pub mod health;
pub mod http;
pub mod ingress;
pub mod insight;
pub mod metrics;
pub mod operator;
pub mod outbound;
pub mod provider;
pub mod quota;
pub mod receipt;
pub mod runtime;
pub mod schedule;
pub mod update;
pub mod work;

pub use cli::{Cli, execute};
pub use config::{Config, ConfigSource, ResolvedConfig};
pub use error::{AppError, ErrorClass, ExitCode};
pub use receipt::{
    AcceptSubmissionOutcome, AcceptSubmissionRequest, ConfirmOutcome, ConfirmRequest,
    EditDraftRequest, ExpenseDraftView, ExtractOutcome, ExtractedAttempt, ExtractionMeta,
    FakeExtractor, InMemoryObjectStore, JOB_TYPE_EXTRACT, JOB_TYPE_INGEST, MAX_IMAGE_BYTES,
    MAX_PIXEL_COUNT, ReceiptConfig, ReceiptError, ReceiptExtractor, ReceiptLifecycle,
    ReceiptObjectStore, ReceiptState, ReceiptStateView, RejectRequest, ValidatedImage,
    account_serialization_key, bytes_for_corpus_index, can_transition, corpus_index_for, extract,
    extract_dedupe_key, ingest_dedupe_key, object_key, validate_image,
};
pub use runtime::{Role, RuntimeOptions, run};
pub use work::{
    AttemptOutcome, AttemptSummary, ClaimOptions, ClaimedJob, EnqueueOutcome, EnqueueRequest,
    FailOutcome, JobListRow, JobState, JobSummary, VersionedPayload, WorkError, WorkStore,
    is_retryable, retry_delay_secs,
};
