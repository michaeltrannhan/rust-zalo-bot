//! Receipt submission lifecycle, assets, drafts, and retention.

mod downscale;
mod error;
mod extractor;
mod fs_store;
mod gemini;
mod lifecycle;
mod object_store;
mod s3_store;
mod types;
mod validate;

pub use downscale::{MAX_EXTRACTION_EDGE, downscale_to_jpeg};
pub use error::ReceiptError;
pub use extractor::{
    FakeExtractor, ReceiptExtractor, bytes_for_corpus_index, corpus_index_for, extract,
};
pub use fs_store::FilesystemObjectStore;
pub use gemini::{EXTRACTION_PROMPT_VERSION, GeminiExtractorConfig, GeminiHttpExtractor};
pub use lifecycle::ReceiptLifecycle;
pub use object_store::{InMemoryObjectStore, ReceiptObjectStore};
pub use s3_store::{S3CompatibleObjectStore, S3ObjectStoreConfig};
pub use types::{
    AcceptSubmissionOutcome, AcceptSubmissionRequest, ConfirmOutcome, ConfirmRequest,
    EditDraftRequest, ExpenseDraftView, ExtractOutcome, ExtractedAttempt, ExtractionMeta,
    ExtractionResult, IngestOutcome, JOB_TYPE_EXTRACT, JOB_TYPE_INGEST, ReceiptConfig,
    ReceiptJobPayload, ReceiptState, ReceiptStateView, RejectRequest, ValidatedImage,
    account_serialization_key, can_transition, extract_dedupe_key, ingest_dedupe_key,
    receipt_serialization_key,
};
pub use validate::{MAX_IMAGE_BYTES, MAX_PIXEL_COUNT, object_key, validate_image};

use std::sync::Arc;
use std::time::Duration;

use crate::config::{ExtractionBackend, ResolvedConfig};
use crate::error::AppError;

/// Construct the configured receipt object store.
pub fn build_object_store(
    config: &ResolvedConfig,
) -> Result<Arc<dyn ReceiptObjectStore>, AppError> {
    match config.storage_backend {
        crate::config::StorageBackend::Memory => Ok(InMemoryObjectStore::new()),
        crate::config::StorageBackend::Filesystem => {
            let store =
                FilesystemObjectStore::new(config.storage_directory.clone()).map_err(|error| {
                    AppError::config(format!("filesystem object store: {}", error.message))
                })?;
            Ok(Arc::new(store))
        }
        crate::config::StorageBackend::S3 => {
            let endpoint = config
                .storage_endpoint
                .clone()
                .ok_or_else(|| AppError::config("storage.endpoint is required for s3 backend"))?;
            let bucket = config
                .storage_bucket
                .clone()
                .ok_or_else(|| AppError::config("storage.bucket is required for s3 backend"))?;
            let access_key = config.read_storage_access_key()?;
            let secret_key = config.read_storage_secret_key()?;
            let store = S3CompatibleObjectStore::new(S3ObjectStoreConfig {
                endpoint,
                bucket,
                region: config.storage_region.clone(),
                access_key,
                secret_key,
                force_path_style: config.storage_force_path_style,
            })
            .map_err(|error| AppError::config(format!("s3 object store: {}", error.message)))?;
            Ok(Arc::new(store))
        }
    }
}

/// Construct the configured receipt extractor.
pub fn build_extractor(config: &ResolvedConfig) -> Result<Arc<dyn ReceiptExtractor>, AppError> {
    match config.extraction_backend {
        ExtractionBackend::Fake => Ok(Arc::new(FakeExtractor)),
        ExtractionBackend::Gemini => {
            let profile = config
                .extraction_profile(&config.extraction_default_profile)
                .ok_or_else(|| {
                    AppError::config("extraction.default_profile is not a loaded AI profile")
                })?;
            let api_key = config.read_named_credential(&profile.credential)?;
            let extractor = GeminiHttpExtractor::new(GeminiExtractorConfig {
                api_base: config.gemini_api_base.clone(),
                api_key,
                model: profile.model.clone(),
                profile_name: profile.name.clone(),
                timeout: Duration::from_secs(profile.timeout_seconds),
                max_input_bytes: profile.max_input_bytes,
                max_output_tokens: profile.max_output_tokens,
                thinking_effort: profile.thinking_effort.clone(),
                schema_version: profile.schema_version.clone(),
            })
            .map_err(|error| AppError::config(format!("gemini extractor: {}", error.message)))?;
            Ok(Arc::new(extractor))
        }
    }
}
