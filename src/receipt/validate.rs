//! Bounded image validation before storage.

use sha2::{Digest, Sha256};

use super::error::ReceiptError;
use super::types::ValidatedImage;

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_PIXEL_COUNT: u64 = 25_000_000;

const ALLOWED_MIME_TYPES: &[&str] = &["image/jpeg", "image/png", "image/webp"];

/// Validate bytes and MIME, compute SHA-256, and read dimensions without full decode.
///
/// Declared Content-Type is advisory: Zalo CDN often sends non-standard
/// `image/jpg` (and sometimes `application/octet-stream`). Acceptance is driven
/// by magic-byte sniff, matching the legacy Go validator.
pub fn validate_image(bytes: &[u8], mime_type: &str) -> Result<ValidatedImage, ReceiptError> {
    if bytes.is_empty() {
        return Err(ReceiptError::validation("image bytes must not be empty"));
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ReceiptError::validation("image exceeds maximum size"));
    }

    let sniffed = sniffed_image_mime(bytes)
        .ok_or_else(|| ReceiptError::validation("image content does not match a supported type"))?;
    let declared = normalize_declared_image_mime(mime_type);
    if let Some(declared) = declared
        && declared != sniffed
    {
        return Err(ReceiptError::validation(
            "image content does not match declared mime type",
        ));
    }
    let normalized_mime = sniffed.to_string();

    let dimensions = imagesize::blob_size(bytes)
        .map_err(|_| ReceiptError::validation("unable to read image dimensions"))?;
    let width_px = i32::try_from(dimensions.width)
        .map_err(|_| ReceiptError::validation("image width out of range"))?;
    let height_px = i32::try_from(dimensions.height)
        .map_err(|_| ReceiptError::validation("image height out of range"))?;
    if width_px <= 0 || height_px <= 0 {
        return Err(ReceiptError::validation(
            "image dimensions must be positive",
        ));
    }
    let pixel_count = u64::from(width_px as u32) * u64::from(height_px as u32);
    if pixel_count > MAX_PIXEL_COUNT {
        return Err(ReceiptError::validation(
            "image exceeds maximum pixel count",
        ));
    }

    let digest = Sha256::digest(bytes);
    let content_sha256 = hex::encode(digest);

    Ok(ValidatedImage {
        content_sha256,
        mime_type: normalized_mime,
        size_bytes: i64::try_from(bytes.len())
            .map_err(|_| ReceiptError::validation("image size out of range"))?,
        width_px,
        height_px,
    })
}

fn sniffed_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some("image/jpeg");
    }
    if bytes.len() >= 8
        && bytes[0] == 0x89
        && bytes[1] == b'P'
        && bytes[2] == b'N'
        && bytes[3] == b'G'
        && bytes[4] == 0x0D
        && bytes[5] == 0x0A
        && bytes[6] == 0x1A
        && bytes[7] == 0x0A
    {
        return Some("image/png");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Normalize a provider Content-Type into a canonical image MIME, or `None` when
/// the header is missing/generic so sniff can decide.
fn normalize_declared_image_mime(mime_type: &str) -> Option<&'static str> {
    let primary = mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase();
    match primary.as_str() {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/webp" => Some("image/webp"),
        "" | "application/octet-stream" | "binary/octet-stream" | "application/binary" => None,
        other if ALLOWED_MIME_TYPES.contains(&other) => {
            // Keep allowlist exhaustive even if match arms drift.
            ALLOWED_MIME_TYPES
                .iter()
                .copied()
                .find(|allowed| *allowed == other)
        }
        _ => None,
    }
}

/// Deterministic object key for a stored receipt asset.
pub fn object_key(
    account_id: uuid::Uuid,
    submission_id: uuid::Uuid,
    content_sha256: &str,
) -> String {
    format!("receipts/{account_id}/{submission_id}/{content_sha256}")
}

const MAX_MERCHANT_CHARS: usize = 200;

/// Validate a draft amount in minor units.
pub fn validate_amount_minor(amount_minor: i64) -> Result<(), ReceiptError> {
    if amount_minor <= 0 {
        return Err(ReceiptError::validation("amount must be positive"));
    }
    Ok(())
}

/// Validate an ISO-4217 alphabetic currency code.
pub fn validate_currency(currency: &str) -> Result<(), ReceiptError> {
    if currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Ok(());
    }
    Err(ReceiptError::validation(
        "currency must be a 3-letter ISO code",
    ))
}

/// Validate a merchant string and return the trimmed value.
pub fn validate_merchant(merchant: &str) -> Result<String, ReceiptError> {
    let trimmed = merchant.trim();
    let chars = trimmed.chars().count();
    if chars == 0 || chars > MAX_MERCHANT_CHARS {
        return Err(ReceiptError::validation(
            "merchant must be between 1 and 200 characters",
        ));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_JPEG: &[u8] = include_bytes!("testdata/tiny.jpg");
    const TINY_PNG: &[u8] = include_bytes!("testdata/tiny.png");
    const TINY_WEBP: &[u8] = include_bytes!("testdata/tiny.webp");

    fn png_crc(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
                crc = (crc >> 1) ^ mask;
            }
        }
        !crc
    }

    fn png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let crc = png_crc(&ihdr);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13_u32.to_be_bytes());
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&crc.to_be_bytes());
        png
    }

    #[test]
    fn rejects_oversized_payload_before_dimension_parse() {
        let bytes = vec![0_u8; MAX_IMAGE_BYTES + 1];
        let error = validate_image(&bytes, "image/png").expect_err("oversize");
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
    }

    #[test]
    fn rejects_unsupported_payload_bytes() {
        let error = validate_image(&[1, 2, 3], "text/plain").expect_err("sniff");
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
    }

    #[test]
    fn sniff_agrees_with_jpeg_png_webp() {
        let jpeg = validate_image(TINY_JPEG, "image/jpeg").expect("jpeg");
        assert_eq!(jpeg.mime_type, "image/jpeg");
        let png = validate_image(TINY_PNG, "image/png").expect("png");
        assert_eq!(png.mime_type, "image/png");
        let webp = validate_image(TINY_WEBP, "image/webp").expect("webp");
        assert_eq!(webp.mime_type, "image/webp");
    }

    #[test]
    fn sniff_rejects_mime_mismatch() {
        let error = validate_image(TINY_PNG, "image/jpeg").expect_err("mismatch");
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
        let error = validate_image(TINY_JPEG, "image/png").expect_err("mismatch");
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
        let error = validate_image(TINY_WEBP, "image/png").expect_err("mismatch");
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
    }

    #[test]
    fn accepts_zalo_image_jpg_alias_and_octet_stream() {
        let jpeg = validate_image(TINY_JPEG, "image/jpg").expect("jpg alias");
        assert_eq!(jpeg.mime_type, "image/jpeg");
        let jpeg = validate_image(TINY_JPEG, "image/jpg; charset=binary").expect("jpg params");
        assert_eq!(jpeg.mime_type, "image/jpeg");
        let jpeg = validate_image(TINY_JPEG, "application/octet-stream").expect("octet-stream");
        assert_eq!(jpeg.mime_type, "image/jpeg");
        let jpeg = validate_image(TINY_JPEG, "").expect("empty declared");
        assert_eq!(jpeg.mime_type, "image/jpeg");
    }

    #[test]
    fn rejects_unsupported_sniff_even_with_declared_jpeg() {
        let error = validate_image(&[1, 2, 3, 4], "image/jpeg").expect_err("sniff");
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
    }

    #[test]
    fn rejects_pixel_budget_without_full_decode() {
        let bytes = png_with_dimensions(5_001, 5_001);
        let error = validate_image(&bytes, "image/png").expect_err("pixels");
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
        assert!(error.message.contains("pixel"));
    }
}
