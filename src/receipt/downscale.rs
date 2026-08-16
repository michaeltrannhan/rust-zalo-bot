//! Downscale extraction input to a 2048-pixel max edge JPEG.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{GenericImageView, ImageFormat};

use super::error::ReceiptError;

/// Longest edge allowed on Gemini extraction input.
pub const MAX_EXTRACTION_EDGE: u32 = 2048;

/// Decode `bytes` and return a JPEG whose longest edge is at most 2048 pixels.
pub fn downscale_to_jpeg(bytes: &[u8]) -> Result<Vec<u8>, ReceiptError> {
    let image = image::load_from_memory(bytes)
        .map_err(|_| ReceiptError::validation("unable to decode image for extraction"))?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err(ReceiptError::validation(
            "image dimensions must be positive",
        ));
    }
    let max_edge = width.max(height);
    let image = if max_edge > MAX_EXTRACTION_EDGE {
        let scale = f64::from(MAX_EXTRACTION_EDGE) / f64::from(max_edge);
        let next_width = ((f64::from(width) * scale).round() as u32).max(1);
        let next_height = ((f64::from(height) * scale).round() as u32).max(1);
        image.resize(next_width, next_height, FilterType::Triangle)
    } else {
        image
    };
    let mut encoded = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Jpeg)
        .map_err(|_| ReceiptError::dependency("failed to encode extraction jpeg"))?;
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn downscales_long_edge_to_2048() {
        let buffer = ImageBuffer::from_pixel(3000, 1000, Rgba([10_u8, 20, 30, 255]));
        let image = image::DynamicImage::ImageRgba8(buffer);
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .expect("png");
        let jpeg = downscale_to_jpeg(&png).expect("downscale");
        let decoded = image::load_from_memory(&jpeg).expect("decode jpeg");
        let (width, height) = decoded.dimensions();
        assert_eq!(width.max(height), MAX_EXTRACTION_EDGE);
        assert!(width.min(height) <= 683);
    }
}
