//! Bounded HTTPS media download with SSRF controls.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::{Client, StatusCode, Url};

use crate::error::ErrorClass;

use super::error::ZaloProviderError;
use super::redact::redact_value;
use super::types::{MediaDownloadPolicy, MediaDownloadResult};

const MAX_URL_CHARS: usize = 2048;
const MAX_CONTENT_TYPE_CHARS: usize = 256;

/// DNS resolution seam for media downloads (injectable in tests).
pub trait MediaHostResolver: Send + Sync {
    fn resolve_host(
        &self,
        host: &str,
    ) -> impl Future<Output = Result<Vec<IpAddr>, ZaloProviderError>> + Send;
}

/// Production resolver using nonblocking system DNS.
#[derive(Debug, Clone, Default)]
pub struct SystemMediaResolver;

impl MediaHostResolver for SystemMediaResolver {
    async fn resolve_host(&self, host: &str) -> Result<Vec<IpAddr>, ZaloProviderError> {
        let addrs = tokio::net::lookup_host((host, 443u16))
            .await
            .map_err(|_| validation("dns resolution failed"))?;
        let ips: Vec<IpAddr> = addrs.map(|addr| addr.ip()).collect();
        if ips.is_empty() {
            return Err(validation("dns resolution returned no addresses"));
        }
        Ok(ips)
    }
}

/// Injected resolver for loopback contract tests.
#[derive(Clone, Default)]
pub struct InjectedMediaResolver {
    hosts: HashMap<String, Vec<IpAddr>>,
}

impl InjectedMediaResolver {
    pub fn new() -> Self {
        Self {
            hosts: HashMap::new(),
        }
    }

    pub fn insert(&mut self, host: impl Into<String>, addrs: Vec<IpAddr>) {
        self.hosts.insert(host.into(), addrs);
    }
}

impl MediaHostResolver for InjectedMediaResolver {
    async fn resolve_host(&self, host: &str) -> Result<Vec<IpAddr>, ZaloProviderError> {
        self.hosts
            .get(host)
            .cloned()
            .filter(|addrs| !addrs.is_empty())
            .ok_or_else(|| validation("dns resolution failed"))
    }
}

/// Narrow media-download seam: policy plus resolver, no shared HTTP client.
pub struct ZaloMediaDownloader<R> {
    policy: MediaDownloadPolicy,
    resolver: R,
}

impl<R: MediaHostResolver> ZaloMediaDownloader<R> {
    pub fn new(policy: MediaDownloadPolicy, resolver: R) -> Self {
        Self { policy, resolver }
    }

    pub fn policy(&self) -> &MediaDownloadPolicy {
        &self.policy
    }

    pub async fn download(&self, url: &str) -> Result<MediaDownloadResult, ZaloProviderError> {
        download_media(&self.policy, &self.resolver, url).await
    }
}

async fn download_media<R: MediaHostResolver + ?Sized>(
    policy: &MediaDownloadPolicy,
    resolver: &R,
    url: &str,
) -> Result<MediaDownloadResult, ZaloProviderError> {
    if url.trim().is_empty() || url.chars().count() > MAX_URL_CHARS {
        return Err(validation("media url is invalid").attach_media_context(url));
    }

    let mut current = Url::parse(url)
        .map_err(|_| validation("media url is invalid").attach_media_context(url))?;
    let mut redirects = 0u32;
    let deadline = tokio::time::Instant::now() + policy.total_timeout;

    loop {
        remaining_until(deadline).map_err(|err| err.attach_media_context(url))?;
        validate_media_url(policy, &current, url)?;

        let host = current
            .host_str()
            .ok_or_else(|| validation("media url host is required").attach_media_context(url))?;
        let port = current.port_or_known_default().unwrap_or(443);

        let resolved = match tokio::time::timeout_at(deadline, resolver.resolve_host(host)).await {
            Ok(result) => result?,
            Err(_) => return Err(timeout_error().attach_media_context(url)),
        };
        let pinned = pin_resolved_addrs(policy, host, resolved)
            .map_err(|err| err.attach_media_context(url))?;
        let socket_addrs: Vec<SocketAddr> =
            pinned.iter().map(|ip| SocketAddr::new(*ip, port)).collect();

        let remaining = remaining_until(deadline).map_err(|err| err.attach_media_context(url))?;
        let hop_client = build_pinned_client(host, &socket_addrs, remaining)?;
        let response =
            match tokio::time::timeout_at(deadline, hop_client.get(current.clone()).send()).await {
                Ok(result) => result.map_err(|err| map_transport_error(url, err))?,
                Err(_) => return Err(timeout_error().attach_media_context(url)),
            };

        let status = response.status();
        if status.is_redirection() {
            if redirects >= policy.max_redirects {
                return Err(validation("media redirect limit exceeded").attach_media_context(url));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    validation("media redirect missing location").attach_media_context(url)
                })?;
            current = current.join(location).map_err(|_| {
                validation("media redirect location is invalid").attach_media_context(url)
            })?;
            redirects += 1;
            continue;
        }

        if !status.is_success() {
            return Err(classify_http_status(status, url));
        }

        if let Some(length) = response.content_length()
            && length > policy.max_bytes
        {
            return Err(validation("media content length exceeds limit").attach_media_context(url));
        }

        let header_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        if let Some(length) = header_length
            && length > policy.max_bytes
        {
            return Err(validation("media content length exceeds limit").attach_media_context(url));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                if value.chars().count() > MAX_CONTENT_TYPE_CHARS {
                    value.chars().take(MAX_CONTENT_TYPE_CHARS).collect()
                } else {
                    value.to_string()
                }
            });

        let max_bytes = usize_max_bytes(policy.max_bytes);
        let mut body = Vec::new();
        let mut response = response;
        loop {
            let chunk = match tokio::time::timeout_at(deadline, response.chunk()).await {
                Ok(result) => result.map_err(|err| map_transport_error(url, err))?,
                Err(_) => return Err(timeout_error().attach_media_context(url)),
            };
            let Some(chunk) = chunk else {
                break;
            };
            let next_len = match body.len().checked_add(chunk.len()) {
                Some(len) => len,
                None => {
                    return Err(
                        validation("media body exceeds size limit").attach_media_context(url)
                    );
                }
            };
            if next_len > max_bytes {
                return Err(validation("media body exceeds size limit").attach_media_context(url));
            }
            body.extend_from_slice(&chunk);
        }

        return Ok(MediaDownloadResult {
            bytes: body,
            content_type,
        });
    }
}

fn pin_resolved_addrs(
    policy: &MediaDownloadPolicy,
    host: &str,
    resolved: Vec<IpAddr>,
) -> Result<Vec<IpAddr>, ZaloProviderError> {
    if resolved.is_empty() {
        return Err(validation("dns resolution returned no addresses"));
    }
    let any_forbidden = resolved.iter().copied().any(is_forbidden_ip);
    if any_forbidden {
        let host_is_literal_ip = host.parse::<IpAddr>().is_ok();
        if !policy.permit_private_resolved_addresses || host_is_literal_ip {
            return Err(validation("media url resolves to forbidden addresses"));
        }
    }
    Ok(resolved)
}

fn remaining_until(deadline: tokio::time::Instant) -> Result<Duration, ZaloProviderError> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        Err(timeout_error())
    } else {
        Ok(remaining)
    }
}

fn build_pinned_client(
    host: &str,
    addrs: &[SocketAddr],
    timeout: Duration,
) -> Result<Client, ZaloProviderError> {
    Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|_| {
            ZaloProviderError::new(ErrorClass::Dependency, "build media HTTP client failed")
        })
}

fn validate_media_url(
    policy: &MediaDownloadPolicy,
    url: &Url,
    original: &str,
) -> Result<(), ZaloProviderError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            validation("media url must not contain userinfo").attach_media_context(original)
        );
    }
    if policy.require_https && url.scheme() != "https" {
        return Err(validation("media url must use https").attach_media_context(original));
    }
    if !policy.require_https && url.scheme() != "http" && url.scheme() != "https" {
        return Err(validation("media url must use http or https").attach_media_context(original));
    }
    if !port_allowed(policy, url) {
        return Err(validation("media url port is not allowed").attach_media_context(original));
    }

    let host = url
        .host_str()
        .ok_or_else(|| validation("media url host is required").attach_media_context(original))?;

    if let Ok(ip) = host.parse::<IpAddr>()
        && is_forbidden_ip(ip)
    {
        return Err(validation("media url host is forbidden").attach_media_context(original));
    }

    if !host_allowed(host, &policy.host_suffixes) {
        return Err(validation("media url host is not allowlisted").attach_media_context(original));
    }

    Ok(())
}

fn port_allowed(policy: &MediaDownloadPolicy, url: &Url) -> bool {
    match url.port() {
        None => true,
        Some(443) if url.scheme() == "https" => true,
        Some(port) => policy.allowed_explicit_port == Some(port),
    }
}

fn host_allowed(host: &str, suffixes: &[String]) -> bool {
    let host = host.to_ascii_lowercase();
    suffixes.iter().any(|suffix| {
        let suffix = suffix.to_ascii_lowercase();
        host == suffix || host.ends_with(&format!(".{suffix}"))
    })
}

pub(crate) fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => is_forbidden_v4(addr),
        IpAddr::V6(addr) => is_forbidden_v6(addr),
    }
}

fn is_forbidden_v4(addr: Ipv4Addr) -> bool {
    let bits = u32::from(addr);
    addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_multicast()
        || addr.is_unspecified()
        || addr.is_broadcast()
        || addr.octets() == [169, 254, 169, 254]
        || ipv4_prefix(bits, Ipv4Addr::new(0, 0, 0, 0), 8)
        || ipv4_prefix(bits, Ipv4Addr::new(100, 64, 0, 0), 10)
        || ipv4_prefix(bits, Ipv4Addr::new(192, 0, 0, 0), 24)
        || ipv4_prefix(bits, Ipv4Addr::new(192, 0, 2, 0), 24)
        || ipv4_prefix(bits, Ipv4Addr::new(198, 51, 100, 0), 24)
        || ipv4_prefix(bits, Ipv4Addr::new(203, 0, 113, 0), 24)
        || ipv4_prefix(bits, Ipv4Addr::new(198, 18, 0, 0), 15)
        || ipv4_prefix(bits, Ipv4Addr::new(240, 0, 0, 0), 4)
}

fn is_forbidden_v6(addr: Ipv6Addr) -> bool {
    if let Some(mapped) = addr.to_ipv4_mapped() {
        return is_forbidden_v4(mapped);
    }
    let bits = u128::from(addr);
    addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_multicast()
        || addr.is_unique_local()
        || addr.is_unicast_link_local()
        || ipv6_prefix(bits, Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10)
        || ipv6_prefix(bits, Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64)
        || ipv6_prefix(bits, Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32)
        || ipv6_prefix(bits, Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20)
        || ipv6_prefix(bits, Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23)
        || !ipv6_prefix(bits, Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3)
}

fn ipv4_prefix(addr_bits: u32, prefix: Ipv4Addr, prefix_len: u32) -> bool {
    let shift = 32 - prefix_len;
    (addr_bits >> shift) == (u32::from(prefix) >> shift)
}

fn ipv6_prefix(addr_bits: u128, prefix: Ipv6Addr, prefix_len: u32) -> bool {
    let shift = 128 - prefix_len;
    (addr_bits >> shift) == (u128::from(prefix) >> shift)
}

fn usize_max_bytes(max_bytes: u64) -> usize {
    usize::try_from(max_bytes).unwrap_or(usize::MAX)
}

fn classify_http_status(status: StatusCode, url: &str) -> ZaloProviderError {
    let message = format!("media download returned HTTP {}", status.as_u16());
    let class = match status.as_u16() {
        429 => ErrorClass::RateLimited,
        500..=599 => ErrorClass::ProviderError,
        400..=499 => ErrorClass::Validation,
        _ => ErrorClass::Validation,
    };
    ZaloProviderError::new(class, message).attach_media_context(url)
}

fn map_transport_error(url: &str, err: reqwest::Error) -> ZaloProviderError {
    let err = err.without_url();
    if err.is_timeout() {
        return timeout_error().attach_media_context(url);
    }
    let cause = err.to_string();
    let redacted = redact_value(&cause, "", "", "", url).into_owned();
    ZaloProviderError::new(
        ErrorClass::ProviderError,
        format!("media download failed: {redacted}"),
    )
    .attach_media_context(url)
}

fn timeout_error() -> ZaloProviderError {
    ZaloProviderError::new(ErrorClass::Timeout, "media download timed out")
}

fn validation(message: impl Into<String>) -> ZaloProviderError {
    ZaloProviderError::new(ErrorClass::Validation, message)
}
