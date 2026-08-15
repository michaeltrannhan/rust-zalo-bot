//! Deterministic fake extractor for tests and local runs.

use chrono::{TimeZone, Utc};
use sha2::{Digest, Sha256};

use super::error::ReceiptError;
use super::types::ExtractionResult;

/// Narrow extractor seam so tests can force transient or permanent failure.
pub trait ReceiptExtractor: Send + Sync {
    fn extract(&self, bytes: &[u8]) -> Result<ExtractionResult, ReceiptError>;
}

/// Deterministic corpus extractor used in M4 tests and local runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeExtractor;

impl ReceiptExtractor for FakeExtractor {
    fn extract(&self, bytes: &[u8]) -> Result<ExtractionResult, ReceiptError> {
        extract(bytes)
    }
}

const MOCK_PREFIX: &str = "MOCK-FIXTURE:";

struct CorpusEntry {
    id: &'static str,
    merchant: &'static str,
    amount_minor: i64,
    currency: &'static str,
    category_key: &'static str,
    transaction_type: &'static str,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    confidence: f32,
    unsupported: bool,
}

const CORPUS: &[CorpusEntry] = &[
    CorpusEntry {
        id: "coopmart-clean",
        merchant: "Co.opmart",
        amount_minor: 325_000,
        currency: "VND",
        category_key: "thuc-pham",
        transaction_type: "expense",
        year: 2026,
        month: 7,
        day: 15,
        hour: 9,
        minute: 24,
        confidence: 0.95,
        unsupported: false,
    },
    CorpusEntry {
        id: "highlands-clean",
        merchant: "Highlands Coffee",
        amount_minor: 65_000,
        currency: "VND",
        category_key: "an-uong",
        transaction_type: "expense",
        year: 2026,
        month: 7,
        day: 16,
        hour: 14,
        minute: 5,
        confidence: 0.93,
        unsupported: false,
    },
    CorpusEntry {
        id: "petrolimex-clean",
        merchant: "Petrolimex",
        amount_minor: 500_000,
        currency: "VND",
        category_key: "di-lai",
        transaction_type: "expense",
        year: 2026,
        month: 7,
        day: 17,
        hour: 8,
        minute: 12,
        confidence: 0.90,
        unsupported: false,
    },
    CorpusEntry {
        id: "guardian-low-total",
        merchant: "Guardian",
        amount_minor: 142_000,
        currency: "VND",
        category_key: "suc-khoe",
        transaction_type: "expense",
        year: 2026,
        month: 7,
        day: 14,
        hour: 19,
        minute: 41,
        confidence: 0.72,
        unsupported: false,
    },
    CorpusEntry {
        id: "unsupported-image",
        merchant: "Unsupported",
        amount_minor: 0,
        currency: "VND",
        category_key: "khac",
        transaction_type: "expense",
        year: 2026,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        confidence: 0.0,
        unsupported: true,
    },
];

fn index_for(bytes: &[u8]) -> usize {
    let digest = Sha256::digest(bytes);
    let value = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    (value % CORPUS.len() as u64) as usize
}

/// Map arbitrary bytes to the fake extractor corpus index.
pub fn corpus_index_for(bytes: &[u8]) -> usize {
    index_for(bytes)
}

fn corpus_entry(bytes: &[u8]) -> Result<&'static CorpusEntry, ReceiptError> {
    if let Some(rest) = bytes.strip_prefix(MOCK_PREFIX.as_bytes()) {
        let id = std::str::from_utf8(rest)
            .map_err(|_| ReceiptError::validation("invalid mock fixture id"))?
            .trim();
        return CORPUS
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| ReceiptError::validation("unknown mock fixture id"));
    }
    Ok(&CORPUS[index_for(bytes)])
}

/// Extract structured fields deterministically from receipt bytes.
pub fn extract(bytes: &[u8]) -> Result<ExtractionResult, ReceiptError> {
    let entry = corpus_entry(bytes)?;
    if entry.unsupported {
        return Ok(ExtractionResult {
            merchant: entry.merchant.to_string(),
            amount_minor: entry.amount_minor,
            currency: entry.currency.to_string(),
            category_key: entry.category_key.to_string(),
            transaction_type: entry.transaction_type.to_string(),
            occurred_at: Utc
                .with_ymd_and_hms(
                    entry.year,
                    entry.month,
                    entry.day,
                    entry.hour,
                    entry.minute,
                    0,
                )
                .unwrap(),
            confidence: entry.confidence,
            unsupported: true,
        });
    }

    Ok(ExtractionResult {
        merchant: entry.merchant.to_string(),
        amount_minor: entry.amount_minor,
        currency: entry.currency.to_string(),
        category_key: entry.category_key.to_string(),
        transaction_type: entry.transaction_type.to_string(),
        occurred_at: Utc
            .with_ymd_and_hms(
                entry.year,
                entry.month,
                entry.day,
                entry.hour,
                entry.minute,
                0,
            )
            .unwrap(),
        confidence: entry.confidence,
        unsupported: false,
    })
}

/// Return bytes that deterministically select corpus index `index`.
pub fn bytes_for_corpus_index(index: usize) -> Option<Vec<u8>> {
    if index >= CORPUS.len() {
        return None;
    }
    let mut nonce = 0_u32;
    loop {
        let seed = format!("mock-fixture-{index}-{nonce}");
        let digest = Sha256::digest(seed.as_bytes());
        if index_for(&digest) == index {
            return Some(digest.to_vec());
        }
        nonce += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_is_deterministic_for_same_bytes() {
        let bytes = bytes_for_corpus_index(0).expect("fixture bytes");
        let first = extract(&bytes).expect("extract");
        let second = extract(&bytes).expect("extract");
        assert_eq!(first, second);
    }
}
