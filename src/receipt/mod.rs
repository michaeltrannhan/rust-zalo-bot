//! Receipt submission lifecycle, assets, drafts, and retention.

mod error;
mod extractor;
mod lifecycle;
mod object_store;
mod types;
mod validate;

pub use error::ReceiptError;
pub use extractor::{
    FakeExtractor, ReceiptExtractor, bytes_for_corpus_index, corpus_index_for, extract,
};
pub use lifecycle::ReceiptLifecycle;
pub use object_store::{InMemoryObjectStore, ReceiptObjectStore};
pub use types::{
    AcceptSubmissionOutcome, AcceptSubmissionRequest, ConfirmOutcome, ConfirmRequest,
    EditDraftRequest, ExpenseDraftView, ExtractOutcome, ExtractionResult, IngestOutcome,
    JOB_TYPE_EXTRACT, JOB_TYPE_INGEST, ReceiptConfig, ReceiptJobPayload, ReceiptState,
    ReceiptStateView, RejectRequest, ValidatedImage, account_serialization_key, can_transition,
    extract_dedupe_key, ingest_dedupe_key,
};
pub use validate::{MAX_IMAGE_BYTES, MAX_PIXEL_COUNT, object_key, validate_image};
