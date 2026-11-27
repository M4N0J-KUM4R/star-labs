//! The S3-compatible backend.
//!
//! Canvas can keep attachment bytes in S3
//! (`gems/attachment_fu/lib/attachment_fu/backends/s3_backend.rb`,
//! `config/file_store.yml` -> `storage: s3`). The port can too, behind the
//! same [`ObjectStore`] trait, with the same key layout: the account
//! namespace, the `attachments` path prefix, the attachment id and the file
//! name.
//!
//! Requests are signed with AWS Signature Version 4 (`AWS4-HMAC-SHA256`),
//! which is what the AWS SDKs use for S3.

use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::key::canvas_s3_key;
use crate::{to_hex, BoxFuture, ObjectStore, StorageError, StoredObject};

type HmacSha256 = Hmac<Sha256>;

/// How to reach the bucket.
#[derive(Debug, Clone)]
pub struct S3Config {
    /// The bucket that holds the bytes.
    pub bucket: String,
    /// The region the bucket lives in.
    pub region: String,
    /// The endpoint of an S3-compatible service (`https://minio.example.com`).
    /// When it is set the requests are path-style (`/<bucket>/<key>`).
    pub endpoint: Option<String>,
    /// The access key id.
    pub access_key_id: String,
    /// The secret access key.
    pub secret_access_key: String,
    /// Canvas' account namespace; it prefixes the key.
    pub namespace: Option<String>,
}

impl S3Config {
    /// Read the configuration out of the environment.
    pub fn from_env() -> Result<Self, StorageError> {
        let bucket = std::env::var("STAR_S3_BUCKET")
            .map_err(|_| StorageError::Config("STAR_S3_BUCKET is not set".to_string()))?;
        let access_key_id = std::env::var("STAR_S3_ACCESS_KEY_ID")
            .map_err(|_| StorageError::Config("STAR_S3_ACCESS_KEY_ID is not set".to_string()))?;
        let secret_access_key = std::env::var("STAR_S3_SECRET_ACCESS_KEY").map_err(|_| {
            StorageError::Config("STAR_S3_SECRET_ACCESS_KEY is not set".to_string())
        })?;
        Ok(S3Config {
            bucket,
            region: std::env::var("STAR_S3_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
            endpoint: std::env::var("STAR_S3_ENDPOINT").ok(),
            access_key_id,
            secret_access_key,
            namespace: std::env::var("STAR_S3_NAMESPACE").ok(),
        })
    }
}

/// A byte store in an S3-compatible bucket.
#[derive(Debug, Clone)]
pub struct S3Store {
    config: S3Config,
    client: reqwest::Client,
}

impl S3Store {
    /// Build a store for `config`.
    pub fn new(config: S3Config) -> Result<Self, StorageError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| StorageError::Http(format!("build the http client: {e}")))?;
        Ok(Self { config, client })
    }

    /// The configuration of this store.
    pub fn config(&self) -> &S3Config {
        &self.config
    }

    /// The host the requests go to.
    fn host(&self) -> String {
        match &self.config.endpoint {
            Some(endpoint) => endpoint
                .split("://")
                .nth(1)
                .unwrap_or(endpoint)
                .trim_end_matches('/')
                .to_string(),
            None => format!(
                "{}.s3.{}.amazonaws.com",
                self.config.bucket, self.config.region
            ),
        }
    }

    /// The canonical (percent-encoded) path of a key.
    fn canonical_path(&self, key: &str) -> String {
        let mut path = String::from("/");
        let segments: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
        for (index, segment) in segments.iter().enumerate() {
            if index > 0 {
                path.push('/');
            }
            path.push_str(&percent_encode(segment));
        }
        path
    }

    /// The URL a key is served from.
    fn object_url(&self, key: &str) -> String {
        let path = self.canonical_path(key);
        match &self.config.endpoint {
            Some(endpoint) => format!(
                "{}/{}{}",
                endpoint.trim_end_matches('/'),
                self.config.bucket,
                path
            ),
            None => format!("https://{}{}", self.host(), path),
        }
    }

    /// Sign a request with AWS Signature Version 4 and send it.
    async fn send(
        &self,
        method: reqwest::Method,
        key: &str,
        payload: Option<Vec<u8>>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response, StorageError> {
        let payload = payload.unwrap_or_default();
        let payload_hash = to_hex(&Sha256::digest(&payload));

        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        let host = self.host();
        let mut headers: Vec<(String, String)> = vec![
            ("host".to_string(), host.clone()),
            ("x-amz-content-sha256".to_string(), payload_hash.clone()),
            ("x-amz-date".to_string(), amz_date.clone()),
        ];
        if let Some(content_type) = content_type {
            headers.push(("content-type".to_string(), content_type.to_string()));
        }
        headers.sort_by(|a, b| a.0.cmp(&b.0));

        let signed_headers = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_headers = headers
            .iter()
            .map(|(name, value)| format!("{name}:{}\n", value.trim()))
            .collect::<String>();

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            self.canonical_path(key),
            "",
            canonical_headers,
            signed_headers,
            payload_hash
        );

        let scope = format!("{date_stamp}/{}/s3/aws4_request", self.config.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            to_hex(&Sha256::digest(canonical_request.as_bytes()))
        );

        let signature = hex_hmac(&self.signing_key(&date_stamp), string_to_sign.as_bytes());
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.config.access_key_id
        );

        let url = self.object_url(key);
        let mut request = self
            .client
            .request(method.clone(), &url)
            .header("authorization", authorization)
            .header("x-amz-date", amz_date)
            .header("x-amz-content-sha256", payload_hash);
        for (name, value) in headers {
            if name == "host" {
                continue;
            }
            request = request.header(name, value);
        }
        if !payload.is_empty() {
            request = request.body(payload);
        }
        request
            .send()
            .await
            .map_err(|e| StorageError::Http(format!("{method} {url}: {e}")))
    }

    /// The SigV4 signing key for a day.
    fn signing_key(&self, date_stamp: &str) -> Vec<u8> {
        let secret = format!("AWS4{}", self.config.secret_access_key);
        let key = hmac_bytes(secret.as_bytes(), date_stamp.as_bytes());
        let key = hmac_bytes(&key, self.config.region.as_bytes());
        let key = hmac_bytes(&key, b"s3");
        hmac_bytes(&key, b"aws4_request")
    }
}

impl ObjectStore for S3Store {
    fn attachment_key(&self, attachment_id: i64, filename: &str) -> String {
        canvas_s3_key(attachment_id, filename, self.config.namespace.as_deref())
    }

    fn put<'a>(
        &'a self,
        key: &'a str,
        bytes: &'a [u8],
        content_type: Option<&'a str>,
    ) -> BoxFuture<'a, Result<StoredObject, StorageError>> {
        Box::pin(async move {
            let response = self
                .send(
                    reqwest::Method::PUT,
                    key,
                    Some(bytes.to_vec()),
                    content_type,
                )
                .await?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(StorageError::Http(format!("PUT {key}: {status} {body}")));
            }
            let etag = response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                .map(|value| value.trim_matches('"').to_string());
            Ok(StoredObject {
                key: key.to_string(),
                size: bytes.len() as u64,
                digest: etag,
            })
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Vec<u8>, StorageError>> {
        Box::pin(async move {
            let response = self.send(reqwest::Method::GET, key, None, None).await?;
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(StorageError::NotFound(key.to_string()));
            }
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(StorageError::Http(format!("GET {key}: {status} {body}")));
            }
            response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|e| StorageError::Http(format!("GET {key}: {e}")))
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            let response = self.send(reqwest::Method::DELETE, key, None, None).await?;
            let status = response.status();
            if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
                return Ok(());
            }
            let body = response.text().await.unwrap_or_default();
            Err(StorageError::Http(format!("DELETE {key}: {status} {body}")))
        })
    }

    fn exists<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<bool, StorageError>> {
        Box::pin(async move {
            let response = self.send(reqwest::Method::HEAD, key, None, None).await?;
            let status = response.status();
            if status.is_success() {
                return Ok(true);
            }
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(false);
            }
            Err(StorageError::Http(format!("HEAD {key}: {status}")))
        })
    }

    fn size<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<u64, StorageError>> {
        Box::pin(async move {
            let response = self.send(reqwest::Method::HEAD, key, None, None).await?;
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(StorageError::NotFound(key.to_string()));
            }
            if !status.is_success() {
                return Err(StorageError::Http(format!("HEAD {key}: {status}")));
            }
            response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| StorageError::Http(format!("HEAD {key}: no content-length")))
        })
    }
}

/// HMAC-SHA256 of `data` with `key`.
fn hmac_bytes(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Lower-case hex of HMAC-SHA256.
fn hex_hmac(key: &[u8], data: &[u8]) -> String {
    to_hex(&hmac_bytes(key, data))
}

/// Percent-encode a path segment (RFC 3986 unreserved set).
fn percent_encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> S3Store {
        S3Store::new(S3Config {
            bucket: "canvas".to_string(),
            region: "us-east-1".to_string(),
            endpoint: None,
            access_key_id: "AKID".to_string(),
            secret_access_key: "SECRET".to_string(),
            namespace: Some("account_1".to_string()),
        })
        .unwrap()
    }

    #[test]
    fn keys_match_canvas() {
        let store = store();
        assert_eq!(
            store.attachment_key(569, "report.pdf"),
            "account_1/attachments/569/report.pdf"
        );
    }

    #[test]
    fn urls_are_built_per_style() {
        let mut config = store().config().clone();
        config.endpoint = Some("https://minio.example.com".to_string());
        let endpoint_store = S3Store::new(config).unwrap();
        assert_eq!(
            endpoint_store.object_url("account_1/attachments/569/report.pdf"),
            "https://minio.example.com/canvas/account_1/attachments/569/report.pdf"
        );

        let plain = store();
        assert_eq!(
            plain.object_url("account_1/attachments/569/report.pdf"),
            "https://canvas.s3.us-east-1.amazonaws.com/account_1/attachments/569/report.pdf"
        );
    }

    #[test]
    fn signing_is_deterministic() {
        let store = store();
        let key = store.signing_key("20240101");
        assert_eq!(key.len(), 32);
        assert_eq!(
            hex_hmac(&key, b"payload"),
            hex_hmac(&store.signing_key("20240101"), b"payload")
        );
    }
}
