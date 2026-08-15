//! Narrow external provider adapters.

mod error;
mod parse;
mod redact;
mod send;
mod types;
mod zalo_http;

pub use error::ZaloProviderError;
pub use types::{
    DEFAULT_API_BASE, InboundEventKind, NormalizedInboundText, SECRET_HEADER, SendMessageResult,
    ZaloHttpConfig,
};
pub use zalo_http::ZaloHttpAdapter;
