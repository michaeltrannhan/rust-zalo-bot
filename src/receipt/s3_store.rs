//! S3-compatible object store using path-style PUT/GET/DELETE with AWS SigV4.

use std::fmt;
use std::time::Duration;

use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use sha2::{Digest, Sha256};

use super::error::ReceiptError;
use super::object_store::{ReceiptObjectStore, validate_object_key};

type HmacSha256 = Hmac<Sha256>;

const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration for an S3-compatible object store client.
pub struct S3ObjectStoreConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub force_path_style: bool,
}

/// Path-style MinIO/S3 object store backed by HTTP.
pub struct S3CompatibleObjectStore {
    endpoint: Url,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    force_path_style: bool,
    client: reqwest::Client,
}

impl S3CompatibleObjectStore {
    pub fn new(config: S3ObjectStoreConfig) -> Result<Self, ReceiptError> {
        let endpoint = Url::parse(config.endpoint.trim_end_matches('/'))
            .map_err(|_| ReceiptError::validation("storage endpoint must be a valid URL"))?;
        if endpoint.host_str().is_none() {
            return Err(ReceiptError::validation(
                "storage endpoint must include a host",
            ));
        }
        if config.bucket.trim().is_empty() {
            return Err(ReceiptError::validation("storage bucket must not be empty"));
        }
        if config.access_key.is_empty() || config.secret_key.is_empty() {
            return Err(ReceiptError::validation(
                "storage credentials must not be empty",
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| ReceiptError::dependency("failed to build object store HTTP client"))?;
        Ok(Self {
            endpoint,
            bucket: config.bucket,
            region: config.region,
            access_key: config.access_key,
            secret_key: config.secret_key,
            force_path_style: config.force_path_style,
            client,
        })
    }

    fn object_url(&self, key: &str) -> Result<Url, ReceiptError> {
        validate_object_key(key)?;
        let mut url = self.endpoint.clone();
        let path = if self.force_path_style {
            format!("/{}/{}", self.bucket, key)
        } else {
            format!("/{}", key)
        };
        url.set_path(&path);
        Ok(url)
    }

    fn execute(
        &self,
        method: Method,
        key: &str,
        body: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, ReceiptError> {
        let url = self.object_url(key)?;
        // The receipt object store trait is synchronous; block_in_place avoids stalling
        // the multi-thread runtime when other tasks hold the worker thread.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.execute_async(method, url, body))
        })
    }

    async fn execute_async(
        &self,
        method: Method,
        url: Url,
        body: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, ReceiptError> {
        let mut headers = HeaderMap::new();
        let amz_date = amz_date();
        let date_stamp = amz_date[..8].to_string();
        headers.insert(
            HeaderName::from_static("x-amz-date"),
            HeaderValue::from_str(&amz_date)
                .map_err(|_| ReceiptError::dependency("invalid request timestamp"))?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-content-sha256"),
            HeaderValue::from_static(UNSIGNED_PAYLOAD),
        );
        let authorization = sign_request(&SignContext {
            method: method.as_str(),
            url: &url,
            headers: &headers,
            payload_hash: UNSIGNED_PAYLOAD,
            access_key: &self.access_key,
            secret_key: &self.secret_key,
            date_stamp: &date_stamp,
            amz_date: &amz_date,
            region: &self.region,
        })?;
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&authorization)
                .map_err(|_| ReceiptError::dependency("invalid authorization header"))?,
        );

        let mut request = self.client.request(method.clone(), url).headers(headers);
        if let Some(bytes) = body {
            request = request.body(bytes.to_vec());
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Err(ReceiptError::new(
                    crate::error::ErrorClass::Timeout,
                    "object store request timed out",
                ));
            }
            Err(_) => {
                return Err(ReceiptError::dependency("object store request failed"));
            }
        };

        let status = response.status();
        if status == StatusCode::FORBIDDEN {
            return Err(ReceiptError::new(
                crate::error::ErrorClass::Forbidden,
                "object store access denied",
            ));
        }
        if status == StatusCode::NOT_FOUND {
            return match method {
                Method::GET | Method::DELETE => Ok(None),
                _ => Err(ReceiptError::dependency("object store object missing")),
            };
        }
        if status.is_server_error() {
            return Err(ReceiptError::dependency("object store upstream error"));
        }
        if !status.is_success() {
            return Err(ReceiptError::dependency("object store request rejected"));
        }

        if status == StatusCode::NO_CONTENT || method == Method::DELETE {
            return Ok(None);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| ReceiptError::dependency("failed to read object store response"))?;
        Ok(Some(bytes.to_vec()))
    }
}

impl fmt::Debug for S3CompatibleObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3CompatibleObjectStore")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("force_path_style", &self.force_path_style)
            .finish()
    }
}

impl ReceiptObjectStore for S3CompatibleObjectStore {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ReceiptError> {
        self.execute(Method::PUT, key, Some(bytes))?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ReceiptError> {
        self.execute(Method::GET, key, None)
    }

    fn delete(&self, key: &str) -> Result<(), ReceiptError> {
        match self.execute(Method::DELETE, key, None)? {
            Some(_) => Ok(()),
            None => Ok(()),
        }
    }
}

fn amz_date() -> String {
    let now = chrono::Utc::now();
    now.format("%Y%m%dT%H%M%SZ").to_string()
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(
        format!("AWS4{}", secret_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn canonical_uri(url: &Url) -> String {
    let path = url.path();
    if path.is_empty() {
        return "/".to_string();
    }
    path.split('/')
        .map(|segment| {
            if segment.is_empty() {
                String::new()
            } else {
                percent_encode_path_segment(segment)
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_path_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{:02X}", byte));
            }
        }
    }
    encoded
}

fn canonical_query(url: &Url) -> String {
    url.query().unwrap_or("").to_string()
}

fn canonical_headers(headers: &HeaderMap, host: &str) -> (String, String) {
    let mut pairs = vec![("host".to_string(), host.to_string())];
    for (name, value) in headers.iter() {
        let key = name.as_str().to_ascii_lowercase();
        if key == "authorization" {
            continue;
        }
        let value = value.to_str().unwrap_or("").trim();
        pairs.push((key, value.to_string()));
    }
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    let canonical = pairs
        .iter()
        .map(|(key, value)| format!("{}:{}", key, value))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let signed = pairs
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>()
        .join(";");
    (canonical, signed)
}

struct SignContext<'a> {
    method: &'a str,
    url: &'a Url,
    headers: &'a HeaderMap,
    payload_hash: &'a str,
    access_key: &'a str,
    secret_key: &'a str,
    date_stamp: &'a str,
    amz_date: &'a str,
    region: &'a str,
}

fn sign_request(context: &SignContext<'_>) -> Result<String, ReceiptError> {
    let host = context
        .url
        .host_str()
        .map(|value| {
            if let Some(port) = context.url.port() {
                format!("{}:{}", value, port)
            } else {
                value.to_string()
            }
        })
        .unwrap_or_default();
    let (canonical_headers, signed_headers) = canonical_headers(context.headers, &host);
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        context.method,
        canonical_uri(context.url),
        canonical_query(context.url),
        canonical_headers,
        signed_headers,
        context.payload_hash
    );
    let scope = format!("{}/{}/s3/aws4_request", context.date_stamp, context.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        context.amz_date,
        scope,
        sha256_hex(canonical_request.as_bytes())
    );
    let signature_key = signing_key(context.secret_key, context.date_stamp, context.region, "s3");
    let signature = hex::encode(hmac_sha256(&signature_key, string_to_sign.as_bytes()));
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        context.access_key, scope, signed_headers, signature
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_omits_endpoint_and_credentials() {
        let store = S3CompatibleObjectStore::new(S3ObjectStoreConfig {
            endpoint: "http://127.0.0.1:9000".to_string(),
            bucket: "receipts".to_string(),
            region: "us-east-1".to_string(),
            access_key: "access-key-secret".to_string(),
            secret_key: "secret-key-secret".to_string(),
            force_path_style: true,
        })
        .expect("store");
        let debug = format!("{store:?}");
        assert!(debug.contains("bucket"));
        assert!(!debug.contains("access-key-secret"));
        assert!(!debug.contains("secret-key-secret"));
        assert!(!debug.contains("127.0.0.1"));
    }
}
