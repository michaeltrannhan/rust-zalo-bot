//! Narrow external provider adapters.

mod error;
mod media;
mod parse;
mod redact;
mod send;
mod types;
mod zalo_http;

pub use error::ZaloProviderError;
pub use media::{
    InjectedMediaResolver, MediaHostResolver, SystemMediaResolver, ZaloMediaDownloader,
};
pub use types::{
    DEFAULT_API_BASE, DEFAULT_MEDIA_HOST_SUFFIXES, DEFAULT_MEDIA_MAX_BYTES,
    DEFAULT_MEDIA_MAX_REDIRECTS, DEFAULT_MEDIA_TIMEOUT_SECS, EVENT_IMAGE_RECEIVED,
    EVENT_TEXT_RECEIVED, InboundEventKind, MediaDownloadPolicy, MediaDownloadResult,
    NormalizedInboundImage, NormalizedInboundText, SECRET_HEADER, SendMessageResult,
    ZaloHttpConfig,
};
pub use zalo_http::ZaloHttpAdapter;
