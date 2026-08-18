mod credential_store;
mod payload;
mod target;

pub use credential_store::{PlatformSyncCredentialStore, SyncCredentialStore};
pub use credential_store::{SecretString, SyncError};
#[cfg(test)]
pub(crate) use payload::LegacySyncPayloadV1;
pub use payload::{DecodedSyncPayload, PortableConfigV2, SyncPayloadV2, SyncPreview};
pub use target::SyncTargetId;

use std::{fmt, time::Duration};

use crate::session::config::SyncConnectionSnapshot;

use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SYNC_FILE_NAME: &str = "jshell-sync.json";
const FORMAT_VERSION: u32 = 1;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_millis(100);
pub const MAX_SYNC_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

type SyncApiResult<T> = std::result::Result<T, SyncError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedEnvelope {
    format_version: u32,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    payload: String,
}

#[derive(Clone)]
pub struct SyncCredentials {
    pub backend: SyncBackendCredentials,
    pub encryption_password: String,
}

#[derive(Clone)]
pub enum SyncBackendCredentials {
    WebDav {
        endpoint: String,
        username: String,
        password: String,
    },
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        object_key: String,
        access_key: String,
        secret_key: String,
        session_token: String,
    },
    R2 {
        account_id: String,
        bucket: String,
        object_key: String,
        access_key_id: String,
        secret_access_key: SecretString,
    },
}

#[derive(Clone)]
pub struct SyncOperationSnapshot {
    pub(crate) credentials: SyncCredentials,
    pub(crate) target_id: SyncTargetId,
    pub(crate) connection: SyncConnectionSnapshot,
}

impl SyncOperationSnapshot {
    pub(crate) fn new(credentials: SyncCredentials, connection: SyncConnectionSnapshot) -> Self {
        let target_id = connection.target_id();
        Self {
            credentials,
            target_id,
            connection,
        }
    }
}

impl fmt::Debug for SyncOperationSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncOperationSnapshot")
            .field("target_id", &self.target_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteObjectState {
    Exists { etag: Option<String> },
    Missing,
}

#[derive(Clone)]
pub enum SyncResult {
    Tested {
        operation: SyncOperationSnapshot,
        result: Result<RemoteObjectState, SyncError>,
    },
    Uploaded {
        operation: SyncOperationSnapshot,
        payload: Box<SyncPayloadV2>,
        result: Result<Option<String>, SyncError>,
    },
    Downloaded {
        operation: SyncOperationSnapshot,
        result: Result<(DecodedSyncPayload, Option<String>), SyncError>,
    },
}

impl fmt::Debug for SyncResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tested { operation, result } => formatter
                .debug_struct("Tested")
                .field("operation", operation)
                .field("result", result)
                .finish(),
            Self::Uploaded {
                operation, result, ..
            } => formatter
                .debug_struct("Uploaded")
                .field("operation", operation)
                .field("result", result)
                .finish(),
            Self::Downloaded { operation, result } => formatter
                .debug_struct("Downloaded")
                .field("operation", operation)
                .field("result", result)
                .finish(),
        }
    }
}

pub async fn test_connection(credentials: SyncCredentials) -> SyncApiResult<RemoteObjectState> {
    validate_credentials(&credentials)?;
    let response = match &credentials.backend {
        SyncBackendCredentials::WebDav {
            endpoint,
            username,
            password,
        } => sync_http_client()?
            .head(sync_url(endpoint))
            .basic_auth(username, Some(password))
            .send()
            .await
            .map_err(map_reqwest_error)?,
        backend @ (SyncBackendCredentials::S3 { .. } | SyncBackendCredentials::R2 { .. }) => {
            let config = S3Config::from_backend(backend)?;
            let url = s3_url(&config)?;
            let headers = signed_s3_headers("HEAD", &url, &[], &config)?;
            sync_http_client()?
                .head(url)
                .headers(headers)
                .send()
                .await
                .map_err(map_reqwest_error)?
        }
    };

    match response.status() {
        StatusCode::OK => Ok(RemoteObjectState::Exists {
            etag: response_etag(&response),
        }),
        StatusCode::NOT_FOUND => Ok(RemoteObjectState::Missing),
        status => Err(map_http_status(status)),
    }
}

pub async fn confirm_overwrite(
    credentials: SyncCredentials,
    payload: SyncPayloadV2,
) -> SyncApiResult<Option<String>> {
    let (expected_etag, allow_unconditional_webdav) =
        match test_connection(credentials.clone()).await? {
            RemoteObjectState::Exists { etag: Some(etag) } => (Some(etag), false),
            RemoteObjectState::Exists { etag: None }
                if matches!(&credentials.backend, SyncBackendCredentials::WebDav { .. }) =>
            {
                // Some WebDAV servers do not expose validators. This path is reached only
                // after the user confirms the conflict, so permit the unavoidable
                // unconditional write while retaining conditional writes everywhere else.
                (None, true)
            }
            RemoteObjectState::Exists { etag: None } => return Err(SyncError::Conflict),
            RemoteObjectState::Missing => (None, false),
        };
    upload_with_webdav_policy(
        credentials,
        payload,
        expected_etag,
        allow_unconditional_webdav,
    )
    .await
}

pub async fn upload(
    credentials: SyncCredentials,
    payload: SyncPayloadV2,
    expected_etag: Option<String>,
) -> SyncApiResult<Option<String>> {
    upload_with_webdav_policy(credentials, payload, expected_etag, false).await
}

async fn upload_with_webdav_policy(
    credentials: SyncCredentials,
    payload: SyncPayloadV2,
    expected_etag: Option<String>,
    allow_unconditional_webdav: bool,
) -> SyncApiResult<Option<String>> {
    validate_credentials(&credentials)?;
    let encryption_password = SecretString::new(credentials.encryption_password);
    let body = encrypt_payload(&payload, encryption_password.expose_secret())?;
    if body.len() > MAX_SYNC_PAYLOAD_BYTES {
        return Err(SyncError::PayloadTooLarge {
            limit: MAX_SYNC_PAYLOAD_BYTES,
        });
    }
    match credentials.backend {
        SyncBackendCredentials::WebDav {
            endpoint,
            username,
            password,
        } => {
            let condition = match expected_etag {
                Some(etag) => WebDavWriteCondition::Match(etag),
                None if allow_unconditional_webdav => WebDavWriteCondition::Unconditional,
                None => WebDavWriteCondition::CreateOnly,
            };
            upload_webdav(&endpoint, &username, &password, body, condition).await
        }
        SyncBackendCredentials::S3 {
            endpoint,
            region,
            bucket,
            object_key,
            access_key,
            secret_key,
            session_token,
        } => {
            let config = S3Config {
                endpoint,
                region,
                bucket,
                object_key,
                access_key,
                secret_key: SecretString::new(secret_key),
                session_token,
            };
            upload_s3(&config, body, expected_etag).await
        }
        SyncBackendCredentials::R2 {
            account_id,
            bucket,
            object_key,
            access_key_id,
            secret_access_key,
        } => {
            let config = S3Config {
                endpoint: r2_endpoint(&account_id),
                region: "auto".to_string(),
                bucket,
                object_key,
                access_key: access_key_id,
                secret_key: secret_access_key,
                session_token: String::new(),
            };
            upload_s3(&config, body, expected_etag).await
        }
    }
}

enum WebDavWriteCondition {
    CreateOnly,
    Match(String),
    Unconditional,
}

async fn upload_webdav(
    endpoint: &str,
    username: &str,
    password: &str,
    body: Vec<u8>,
    condition: WebDavWriteCondition,
) -> SyncApiResult<Option<String>> {
    let client = sync_http_client()?;
    let mut request = client
        .put(sync_url(endpoint))
        .basic_auth(username, Some(password))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body);
    request = match condition {
        WebDavWriteCondition::Match(etag) => request.header(header::IF_MATCH, etag),
        WebDavWriteCondition::CreateOnly => {
            // An uninitialized client may only create a new remote file. This keeps
            // it from silently replacing configuration uploaded by another device.
            request.header(header::IF_NONE_MATCH, "*")
        }
        WebDavWriteCondition::Unconditional => request,
    };
    let response = request.send().await.map_err(map_reqwest_error)?;
    if !response.status().is_success() {
        return Err(map_http_status(response.status()));
    }
    Ok(response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string))
}

pub async fn download(
    credentials: SyncCredentials,
) -> SyncApiResult<(DecodedSyncPayload, Option<String>)> {
    validate_credentials(&credentials)?;
    let encryption_password = SecretString::new(credentials.encryption_password);
    let (body, etag) = match credentials.backend {
        SyncBackendCredentials::WebDav {
            endpoint,
            username,
            password,
        } => download_webdav(&endpoint, &username, &password).await?,
        SyncBackendCredentials::S3 {
            endpoint,
            region,
            bucket,
            object_key,
            access_key,
            secret_key,
            session_token,
        } => {
            let config = S3Config {
                endpoint,
                region,
                bucket,
                object_key,
                access_key,
                secret_key: SecretString::new(secret_key),
                session_token,
            };
            download_s3(&config).await?
        }
        SyncBackendCredentials::R2 {
            account_id,
            bucket,
            object_key,
            access_key_id,
            secret_access_key,
        } => {
            let config = S3Config {
                endpoint: r2_endpoint(&account_id),
                region: "auto".to_string(),
                bucket,
                object_key,
                access_key: access_key_id,
                secret_key: secret_access_key,
                session_token: String::new(),
            };
            download_s3(&config).await?
        }
    };
    let payload = decrypt_payload(&body, encryption_password.expose_secret())?;
    Ok((payload, etag))
}

async fn download_webdav(
    endpoint: &str,
    username: &str,
    password: &str,
) -> SyncApiResult<(Vec<u8>, Option<String>)> {
    let response = sync_http_client()?
        .get(sync_url(endpoint))
        .basic_auth(username, Some(password))
        .send()
        .await
        .map_err(map_reqwest_error)?;
    if !response.status().is_success() {
        return Err(map_http_status(response.status()));
    }
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = read_response_limited(response).await?;
    Ok((body, etag))
}

pub(crate) fn validate_credentials(
    credentials: &SyncCredentials,
) -> std::result::Result<(), SyncError> {
    if credentials.encryption_password.chars().count() < 8 {
        return Err(SyncError::InvalidInput(
            "encryption password must contain at least 8 characters".to_string(),
        ));
    }
    match &credentials.backend {
        SyncBackendCredentials::WebDav { endpoint, .. } => {
            if endpoint.trim().is_empty() {
                return Err(SyncError::InvalidInput(
                    "WebDAV endpoint is required".to_string(),
                ));
            }
            validate_https_endpoint(endpoint, "WebDAV")
        }
        SyncBackendCredentials::S3 {
            endpoint,
            region,
            bucket,
            access_key,
            secret_key,
            ..
        } => {
            if region.trim().is_empty()
                || bucket.trim().is_empty()
                || access_key.trim().is_empty()
                || secret_key.is_empty()
            {
                return Err(SyncError::InvalidInput(
                    "S3 region, bucket, access key and secret key are required".to_string(),
                ));
            }
            if endpoint.trim().is_empty() {
                Ok(())
            } else {
                validate_https_endpoint(endpoint, "S3")
            }
        }
        SyncBackendCredentials::R2 {
            account_id,
            bucket,
            object_key,
            access_key_id,
            secret_access_key,
        } => {
            if account_id.len() != 32
                || !account_id.bytes().all(|byte| byte.is_ascii_hexdigit())
                || bucket.trim().is_empty()
                || object_key.trim().is_empty()
                || access_key_id.trim().is_empty()
                || secret_access_key.expose_secret().is_empty()
            {
                return Err(SyncError::InvalidInput(
                    "R2 credentials are incomplete or invalid".to_string(),
                ));
            }
            Ok(())
        }
    }
}

fn validate_https_endpoint(endpoint: &str, backend: &str) -> SyncApiResult<()> {
    let url = reqwest::Url::parse(endpoint.trim()).map_err(|_| {
        SyncError::InvalidInput(format!("{backend} endpoint must be a valid HTTPS URL"))
    })?;
    if url.host_str().is_none() {
        return Err(SyncError::InvalidInput(format!(
            "{backend} endpoint must include a host"
        )));
    }
    if sync_transport_url_is_allowed(&url) {
        Ok(())
    } else {
        Err(SyncError::InvalidInput(format!(
            "{backend} endpoint must use HTTPS"
        )))
    }
}

fn sync_transport_url_is_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https" || is_test_loopback_http_endpoint(url)
}

fn sync_redirect_is_allowed(next: &reqwest::Url, previous: &[reqwest::Url]) -> bool {
    let Some(initial) = previous.first() else {
        return false;
    };
    sync_transport_url_is_allowed(next)
        && initial.scheme() == next.scheme()
        && initial.host_str() == next.host_str()
        && initial.port_or_known_default() == next.port_or_known_default()
}

fn is_test_loopback_http_endpoint(url: &reqwest::Url) -> bool {
    #[cfg(test)]
    {
        url.scheme() == "http"
            && url
                .host_str()
                .and_then(|host| {
                    host.trim_start_matches('[')
                        .trim_end_matches(']')
                        .parse::<std::net::IpAddr>()
                        .ok()
                })
                .is_some_and(|address| address.is_loopback())
    }
    #[cfg(not(test))]
    {
        let _ = url;
        false
    }
}

fn r2_endpoint(account_id: &str) -> String {
    format!("https://{}.r2.cloudflarestorage.com", account_id.trim())
}

struct S3Config {
    endpoint: String,
    region: String,
    bucket: String,
    object_key: String,
    access_key: String,
    secret_key: SecretString,
    session_token: String,
}

impl S3Config {
    fn from_backend(backend: &SyncBackendCredentials) -> std::result::Result<Self, SyncError> {
        match backend {
            SyncBackendCredentials::S3 {
                endpoint,
                region,
                bucket,
                object_key,
                access_key,
                secret_key,
                session_token,
            } => Ok(Self {
                endpoint: endpoint.clone(),
                region: region.clone(),
                bucket: bucket.clone(),
                object_key: object_key.clone(),
                access_key: access_key.clone(),
                secret_key: SecretString::new(secret_key.clone()),
                session_token: session_token.clone(),
            }),
            SyncBackendCredentials::R2 {
                account_id,
                bucket,
                object_key,
                access_key_id,
                secret_access_key,
            } => Ok(Self {
                endpoint: r2_endpoint(account_id),
                region: "auto".to_string(),
                bucket: bucket.clone(),
                object_key: object_key.clone(),
                access_key: access_key_id.clone(),
                secret_key: secret_access_key.clone(),
                session_token: String::new(),
            }),
            SyncBackendCredentials::WebDav { .. } => Err(SyncError::InvalidInput(
                "WebDAV credentials are not S3-compatible".to_string(),
            )),
        }
    }
}

async fn upload_s3(
    config: &S3Config,
    body: Vec<u8>,
    expected_etag: Option<String>,
) -> SyncApiResult<Option<String>> {
    let url = s3_url(config)?;
    let mut headers = signed_s3_headers("PUT", &url, &body, config)?;
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    if let Some(etag) = expected_etag {
        headers.insert(header::IF_MATCH, header_value(&etag, "S3 ETag")?);
    } else {
        headers.insert(header::IF_NONE_MATCH, header::HeaderValue::from_static("*"));
    }
    let response = sync_http_client()?
        .put(url)
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(map_reqwest_error)?;
    if !response.status().is_success() {
        return Err(map_http_status(response.status()));
    }
    Ok(response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string))
}

async fn download_s3(config: &S3Config) -> SyncApiResult<(Vec<u8>, Option<String>)> {
    let url = s3_url(config)?;
    let headers = signed_s3_headers("GET", &url, &[], config)?;
    let response = sync_http_client()?
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(map_reqwest_error)?;
    if !response.status().is_success() {
        return Err(map_http_status(response.status()));
    }
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = read_response_limited(response).await?;
    Ok((body, etag))
}

fn s3_url(config: &S3Config) -> SyncApiResult<reqwest::Url> {
    let endpoint = if config.endpoint.trim().is_empty() {
        format!("https://s3.{}.amazonaws.com", config.region.trim())
    } else {
        config.endpoint.trim().trim_end_matches('/').to_string()
    };
    let key = if config.object_key.trim().is_empty() {
        SYNC_FILE_NAME
    } else {
        config.object_key.trim().trim_start_matches('/')
    };
    let url = format!(
        "{}/{}/{}",
        endpoint,
        aws_uri_encode(config.bucket.trim(), true),
        aws_uri_encode(key, false)
    );
    reqwest::Url::parse(&url)
        .map_err(|_| SyncError::InvalidInput("invalid S3 object URL".to_string()))
}

fn signed_s3_headers(
    method: &str,
    url: &reqwest::Url,
    body: &[u8],
    config: &S3Config,
) -> SyncApiResult<header::HeaderMap> {
    signed_s3_headers_at(method, url, body, config, chrono::Utc::now())
}

struct S3SigningMaterial {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "retained for deterministic SigV4 vector verification"
        )
    )]
    canonical_request: String,
    amz_date: String,
    payload_hash: String,
    authorization: String,
}

fn signed_s3_headers_at(
    method: &str,
    url: &reqwest::Url,
    body: &[u8],
    config: &S3Config,
    now: chrono::DateTime<chrono::Utc>,
) -> SyncApiResult<header::HeaderMap> {
    let material = s3_signing_material(method, url, body, config, now)?;
    let mut headers = header::HeaderMap::new();
    headers.insert("x-amz-date", header_value(&material.amz_date, "S3 date")?);
    headers.insert(
        "x-amz-content-sha256",
        header_value(&material.payload_hash, "S3 payload hash")?,
    );
    headers.insert(
        header::AUTHORIZATION,
        header_value(&material.authorization, "S3 authorization")?,
    );
    if !config.session_token.is_empty() {
        headers.insert(
            "x-amz-security-token",
            header_value(config.session_token.trim(), "S3 session token")?,
        );
    }
    Ok(headers)
}

fn s3_signing_material(
    method: &str,
    url: &reqwest::Url,
    body: &[u8],
    config: &S3Config,
    now: chrono::DateTime<chrono::Utc>,
) -> SyncApiResult<S3SigningMaterial> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let host = url
        .host_str()
        .ok_or_else(|| SyncError::InvalidInput("S3 endpoint has no host".to_string()))?;
    let host = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let payload_hash = hex_sha256(body);
    let mut canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
    let mut signed_headers = "host;x-amz-content-sha256;x-amz-date".to_string();
    if !config.session_token.is_empty() {
        canonical_headers.push_str(&format!(
            "x-amz-security-token:{}\n",
            config.session_token.trim()
        ));
        signed_headers.push_str(";x-amz-security-token");
    }
    let canonical_request = format!(
        "{method}\n{}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        url.path()
    );
    let scope = format!("{date}/{}/s3/aws4_request", config.region.trim());
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex_sha256(canonical_request.as_bytes())
    );
    let mut signing_secret = zeroize::Zeroizing::new(String::from("AWS4"));
    signing_secret.push_str(config.secret_key.expose_secret());
    let date_key = hmac_sha256(signing_secret.as_bytes(), date.as_bytes())?;
    let region_key = hmac_sha256(date_key.as_slice(), config.region.trim().as_bytes())?;
    let service_key = hmac_sha256(region_key.as_slice(), b"s3")?;
    let signing_key = hmac_sha256(service_key.as_slice(), b"aws4_request")?;
    let signature_bytes = hmac_sha256(signing_key.as_slice(), string_to_sign.as_bytes())?;
    let signature = hex::encode(signature_bytes.as_slice());
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        config.access_key.trim()
    );
    Ok(S3SigningMaterial {
        canonical_request,
        amz_date,
        payload_hash,
        authorization,
    })
}

fn header_value(value: &str, _name: &str) -> SyncApiResult<header::HeaderValue> {
    header::HeaderValue::from_str(value)
        .map_err(|_| SyncError::InvalidInput("invalid sync request header".to_string()))
}

fn hmac_sha256(key: &[u8], value: &[u8]) -> SyncApiResult<zeroize::Zeroizing<Vec<u8>>> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .map_err(|_| SyncError::InvalidInput("unable to initialize S3 signer".to_string()))?;
    mac.update(value);
    Ok(zeroize::Zeroizing::new(
        mac.finalize().into_bytes().to_vec(),
    ))
}

fn hex_sha256(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn aws_uri_encode(value: &str, encode_slash: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (!encode_slash && byte == b'/')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn sync_url(endpoint: &str) -> String {
    let endpoint = endpoint.trim();
    if endpoint.ends_with('/') {
        format!("{endpoint}{SYNC_FILE_NAME}")
    } else if endpoint.ends_with(".json") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/{SYNC_FILE_NAME}")
    }
}

fn sync_http_client() -> SyncApiResult<Client> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if sync_redirect_is_allowed(attempt.url(), attempt.previous()) {
                reqwest::redirect::Policy::default().redirect(attempt)
            } else {
                attempt.error("sync redirects must stay on the original secure origin")
            }
        }))
        .build()
        .map_err(map_reqwest_error)
}

fn map_reqwest_error(error: reqwest::Error) -> SyncError {
    if error.is_timeout() {
        SyncError::Timeout
    } else {
        SyncError::Network("sync HTTP request failed".to_string())
    }
}

fn map_http_status(status: StatusCode) -> SyncError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => SyncError::Unauthorized,
        StatusCode::NOT_FOUND => SyncError::NotFound,
        StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => SyncError::Conflict,
        StatusCode::PAYLOAD_TOO_LARGE => SyncError::PayloadTooLarge {
            limit: MAX_SYNC_PAYLOAD_BYTES,
        },
        StatusCode::REQUEST_TIMEOUT => SyncError::Timeout,
        _ => SyncError::Network(format!("remote HTTP status {}", status.as_u16())),
    }
}

fn response_etag(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn read_response_limited(mut response: reqwest::Response) -> SyncApiResult<Vec<u8>> {
    if let Some(length) = response.content_length()
        && length > MAX_SYNC_PAYLOAD_BYTES as u64
    {
        return Err(SyncError::PayloadTooLarge {
            limit: MAX_SYNC_PAYLOAD_BYTES,
        });
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_reqwest_error)? {
        if body.len().saturating_add(chunk.len()) > MAX_SYNC_PAYLOAD_BYTES {
            return Err(SyncError::PayloadTooLarge {
                limit: MAX_SYNC_PAYLOAD_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn encrypt_payload<T>(payload: &T, password: &str) -> SyncApiResult<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let key = derive_key(password, &salt)?;
    let plaintext = serde_json::to_vec(payload)
        .map_err(|_| SyncError::InvalidPayload("serialize sync payload".to_string()))?;
    let ciphertext = XChaCha20Poly1305::new((&*key).into())
        .encrypt(XNonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| SyncError::InvalidPayload("encrypt sync payload".to_string()))?;
    serde_json::to_vec_pretty(&EncryptedEnvelope {
        format_version: FORMAT_VERSION,
        kdf: "argon2id".to_string(),
        cipher: "xchacha20poly1305".to_string(),
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        payload: STANDARD.encode(ciphertext),
    })
    .map_err(|_| SyncError::InvalidPayload("serialize sync envelope".to_string()))
}

fn decrypt_payload(raw: &[u8], password: &str) -> SyncApiResult<DecodedSyncPayload> {
    let envelope: EncryptedEnvelope = serde_json::from_slice(raw)
        .map_err(|_| SyncError::InvalidPayload("invalid encrypted sync envelope".to_string()))?;
    if envelope.format_version != FORMAT_VERSION
        || envelope.kdf != "argon2id"
        || envelope.cipher != "xchacha20poly1305"
    {
        return Err(SyncError::InvalidPayload(
            "unsupported remote sync format".to_string(),
        ));
    }
    let salt = STANDARD
        .decode(envelope.salt)
        .map_err(|_| SyncError::InvalidPayload("invalid sync salt".to_string()))?;
    let nonce = STANDARD
        .decode(envelope.nonce)
        .map_err(|_| SyncError::InvalidPayload("invalid sync nonce".to_string()))?;
    if nonce.len() != 24 {
        return Err(SyncError::InvalidPayload("invalid sync nonce".to_string()));
    }
    let ciphertext = STANDARD
        .decode(envelope.payload)
        .map_err(|_| SyncError::InvalidPayload("invalid encrypted sync payload".to_string()))?;
    let key = derive_key(password, &salt)?;
    let plaintext = XChaCha20Poly1305::new((&*key).into())
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| SyncError::DecryptFailed)?;
    payload::decode_payload(&plaintext)
        .map_err(|_| SyncError::InvalidPayload("invalid synchronized configuration".to_string()))
}

fn derive_key(password: &str, salt: &[u8]) -> SyncApiResult<zeroize::Zeroizing<[u8; 32]>> {
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|_| SyncError::InvalidPayload("unable to derive encryption key".to_string()))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use super::*;
    use crate::session::config::{ConfigFile, Session};
    use crate::sync::payload::LegacySyncPayloadV1;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    struct TestResponse {
        head: String,
        chunks: Vec<Vec<u8>>,
        delay: Duration,
    }

    impl TestResponse {
        fn empty(status: &str, headers: &[(&str, &str)]) -> Self {
            let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n");
            for (name, value) in headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("Connection: close\r\n\r\n");
            Self {
                head,
                chunks: Vec::new(),
                delay: Duration::ZERO,
            }
        }

        fn body(status: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Self {
            let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
            for (name, value) in headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("Connection: close\r\n\r\n");
            Self {
                head,
                chunks: vec![body],
                delay: Duration::ZERO,
            }
        }
    }

    async fn spawn_http_server(
        responses: Vec<TestResponse>,
    ) -> (String, JoinHandle<Vec<CapturedRequest>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut captured = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut raw = Vec::new();
                let header_end = loop {
                    let mut buffer = [0_u8; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "connection closed before request headers");
                    raw.extend_from_slice(&buffer[..count]);
                    if let Some(position) = raw.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break position + 4;
                    }
                };
                let header_text = String::from_utf8(raw[..header_end].to_vec()).unwrap();
                let mut lines = header_text.split("\r\n");
                let request_line = lines.next().unwrap();
                let mut request_parts = request_line.split_whitespace();
                let method = request_parts.next().unwrap().to_string();
                let path = request_parts.next().unwrap().to_string();
                let headers: HashMap<String, String> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
                    .collect();
                let content_length = headers
                    .get("content-length")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or_default();
                while raw.len() - header_end < content_length {
                    let mut buffer = [0_u8; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "connection closed before request body");
                    raw.extend_from_slice(&buffer[..count]);
                }
                captured.push(CapturedRequest {
                    method,
                    path,
                    headers,
                    body: raw[header_end..header_end + content_length].to_vec(),
                });

                tokio::time::sleep(response.delay).await;
                if socket.write_all(response.head.as_bytes()).await.is_err() {
                    continue;
                }
                for chunk in response.chunks {
                    if socket.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
            }
            captured
        });
        (format!("http://{address}"), task)
    }

    fn secret_portable_config() -> PortableConfigV2 {
        let mut session = Session::key(
            "secret-host.example".to_string(),
            22,
            "secret-user".to_string(),
            "Z:/external/key/path-sentinel".to_string(),
            "inline-private-key-secret-sentinel".to_string(),
            "private-key-passphrase-secret-sentinel".to_string(),
        );
        session.id = "session-id-sentinel".to_string();
        session.password = "session-password-secret-sentinel".to_string();
        session.proxy_type = "https".to_string();
        session.proxy_host = "session-proxy.example".to_string();
        session.proxy_port = Some(443);
        session.proxy_user = "session-proxy-user-secret-sentinel".to_string();
        session.proxy_password = "session-proxy-password-secret-sentinel".to_string();
        let config = ConfigFile {
            sessions: vec![session],
            use_proxy: true,
            global_proxy_type: "socks5".to_string(),
            global_proxy_host: "global-proxy.example".to_string(),
            global_proxy_port: Some(1080),
            global_proxy_user: "global-proxy-user-secret-sentinel".to_string(),
            global_proxy_password: "global-proxy-password-secret-sentinel".to_string(),
            ..Default::default()
        };
        PortableConfigV2::from(&config)
    }

    fn r2_credentials(account_id: &str) -> SyncCredentials {
        SyncCredentials {
            backend: SyncBackendCredentials::R2 {
                account_id: account_id.to_string(),
                bucket: "sync-bucket".to_string(),
                object_key: "configs/jshell-sync.json".to_string(),
                access_key_id: "r2-access-key-sentinel".to_string(),
                secret_access_key: SecretString::new("r2-secret-key-sentinel".to_string()),
            },
            encryption_password: "correct horse battery staple".to_string(),
        }
    }

    fn s3_credentials(endpoint: String) -> SyncCredentials {
        SyncCredentials {
            backend: SyncBackendCredentials::S3 {
                endpoint,
                region: "test-region".to_string(),
                bucket: "sync-bucket".to_string(),
                object_key: "configs/jshell-sync.json".to_string(),
                access_key: "s3-access-key-sentinel".to_string(),
                secret_key: "s3-secret-key-sentinel".to_string(),
                session_token: String::new(),
            },
            encryption_password: "correct horse battery staple".to_string(),
        }
    }

    fn webdav_credentials(endpoint: String) -> SyncCredentials {
        SyncCredentials {
            backend: SyncBackendCredentials::WebDav {
                endpoint,
                username: "webdav-user".to_string(),
                password: "webdav-password".to_string(),
            },
            encryption_password: "correct horse battery staple".to_string(),
        }
    }

    fn assert_secret_string(_: &SecretString) {}

    #[test]
    fn s3_transport_keeps_secret_key_in_zeroizing_storage() {
        let credentials = s3_credentials("https://s3.example.test".to_string());
        let config = S3Config::from_backend(&credentials.backend).unwrap();

        assert_secret_string(&config.secret_key);
    }

    fn assert_zeroizing_vec(_: &zeroize::Zeroizing<Vec<u8>>) {}

    fn assert_zeroizing_key(_: &zeroize::Zeroizing<[u8; 32]>) {}

    #[test]
    fn derived_signing_and_encryption_keys_use_zeroizing_storage() {
        let hmac_key = hmac_sha256(b"key", b"value").unwrap();
        let encryption_key = derive_key("password", b"0123456789abcdef").unwrap();

        assert_zeroizing_vec(&hmac_key);
        assert_zeroizing_key(&encryption_key);
    }

    #[tokio::test]
    async fn connection_test_sends_head_and_never_put() {
        let (endpoint, server) = spawn_http_server(vec![TestResponse::empty(
            "200 OK",
            &[("ETag", "\"connection-etag\"")],
        )])
        .await;

        let state = test_connection(s3_credentials(endpoint)).await.unwrap();
        let requests = server.await.unwrap();

        assert_eq!(
            state,
            RemoteObjectState::Exists {
                etag: Some("\"connection-etag\"".to_string())
            }
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "HEAD");
        assert_eq!(requests[0].path, "/sync-bucket/configs/jshell-sync.json");
        assert!(requests[0].body.is_empty());
        assert!(requests[0].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn confirm_overwrite_heads_then_puts_with_the_observed_etag() {
        let (endpoint, server) = spawn_http_server(vec![
            TestResponse::empty("200 OK", &[("ETag", "\"observed-etag\"")]),
            TestResponse::empty("200 OK", &[("ETag", "\"new-etag\"")]),
        ])
        .await;

        let etag = confirm_overwrite(
            s3_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(etag.as_deref(), Some("\"new-etag\""));
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "HEAD");
        assert_eq!(requests[1].method, "PUT");
        assert_eq!(
            requests[1].headers.get("if-match").map(String::as_str),
            Some("\"observed-etag\"")
        );
        assert!(!requests[1].headers.contains_key("if-none-match"));
        assert!(!requests[1].body.is_empty());
    }

    #[tokio::test]
    async fn confirmed_webdav_overwrite_without_etag_uses_unconditional_put() {
        let (endpoint, server) = spawn_http_server(vec![
            TestResponse::empty("200 OK", &[]),
            TestResponse::empty("200 OK", &[]),
        ])
        .await;

        let etag = confirm_overwrite(
            webdav_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(etag, None);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "HEAD");
        assert_eq!(requests[1].method, "PUT");
        assert!(!requests[1].headers.contains_key("if-match"));
        assert!(!requests[1].headers.contains_key("if-none-match"));
        assert!(!requests[1].body.is_empty());
    }

    #[tokio::test]
    async fn unconfirmed_webdav_upload_remains_create_only() {
        let (endpoint, server) = spawn_http_server(vec![TestResponse::empty("200 OK", &[])]).await;

        upload(
            webdav_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
            None,
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "PUT");
        assert_eq!(
            requests[0].headers.get("if-none-match").map(String::as_str),
            Some("*")
        );
        assert!(!requests[0].headers.contains_key("if-match"));
    }

    #[tokio::test]
    async fn confirm_overwrite_returns_conflict_after_the_second_request_without_retrying() {
        let (endpoint, server) = spawn_http_server(vec![
            TestResponse::empty("200 OK", &[("ETag", "\"observed-etag\"")]),
            TestResponse::empty("412 Precondition Failed", &[]),
        ])
        .await;

        let result = confirm_overwrite(
            s3_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
        )
        .await;
        let requests = server.await.unwrap();

        assert!(matches!(result, Err(SyncError::Conflict)));
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "HEAD");
        assert_eq!(requests[1].method, "PUT");
        assert_eq!(
            requests[1].headers.get("if-match").map(String::as_str),
            Some("\"observed-etag\"")
        );
        assert!(!requests[1].headers.contains_key("if-none-match"));
    }

    #[tokio::test]
    async fn first_upload_uses_if_none_match() {
        let (endpoint, server) = spawn_http_server(vec![TestResponse::empty("200 OK", &[])]).await;

        let etag = upload(
            s3_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
            None,
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(etag, None);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "PUT");
        assert_eq!(
            requests[0].headers.get("if-none-match").map(String::as_str),
            Some("*")
        );
        assert!(!requests[0].headers.contains_key("if-match"));
    }

    #[tokio::test]
    async fn known_upload_uses_if_match() {
        let (endpoint, server) = spawn_http_server(vec![TestResponse::empty("200 OK", &[])]).await;

        upload(
            s3_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
            Some("\"known-etag\"".to_string()),
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "PUT");
        assert_eq!(
            requests[0].headers.get("if-match").map(String::as_str),
            Some("\"known-etag\"")
        );
        assert!(!requests[0].headers.contains_key("if-none-match"));
    }

    #[tokio::test]
    async fn oversized_encrypted_upload_is_rejected_before_transport() {
        let mut portable = secret_portable_config();
        portable.sessions[0].private_key_inline = "x".repeat(MAX_SYNC_PAYLOAD_BYTES);

        let result = upload(
            s3_credentials("http://127.0.0.1:0".to_string()),
            SyncPayloadV2::new(portable),
            None,
        )
        .await;

        assert!(matches!(
            result,
            Err(SyncError::PayloadTooLarge {
                limit: MAX_SYNC_PAYLOAD_BYTES
            })
        ));
    }

    #[tokio::test]
    async fn oversized_chunked_download_is_stopped_at_eight_mib() {
        let response = TestResponse {
            head: "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string(),
            chunks: vec![
                vec![0_u8; 4 * 1024 * 1024],
                vec![0_u8; 4 * 1024 * 1024],
                vec![0_u8; 1],
            ],
            delay: Duration::ZERO,
        };
        let (endpoint, server) = spawn_http_server(vec![response]).await;

        let result = download(s3_credentials(endpoint)).await;
        let requests = server.await.unwrap();

        assert!(matches!(
            result,
            Err(SyncError::PayloadTooLarge {
                limit: MAX_SYNC_PAYLOAD_BYTES
            })
        ));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
    }

    #[tokio::test]
    async fn conflict_is_never_retried_without_confirmation() {
        let (endpoint, server) =
            spawn_http_server(vec![TestResponse::empty("412 Precondition Failed", &[])]).await;

        let result = upload(
            s3_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
            None,
        )
        .await;
        let requests = server.await.unwrap();

        assert!(matches!(result, Err(SyncError::Conflict)));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "PUT");
    }

    #[tokio::test]
    async fn confirm_overwrite_missing_object_uses_if_none_match() {
        let (endpoint, server) = spawn_http_server(vec![
            TestResponse::empty("404 Not Found", &[]),
            TestResponse::empty("200 OK", &[]),
        ])
        .await;

        confirm_overwrite(
            s3_credentials(endpoint),
            SyncPayloadV2::new(secret_portable_config()),
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "HEAD");
        assert_eq!(requests[1].method, "PUT");
        assert_eq!(
            requests[1].headers.get("if-none-match").map(String::as_str),
            Some("*")
        );
        assert!(!requests[1].headers.contains_key("if-match"));
    }

    #[tokio::test]
    async fn unauthorized_http_status_is_typed() {
        for status in ["401 Unauthorized", "403 Forbidden"] {
            let (endpoint, server) =
                spawn_http_server(vec![TestResponse::empty(status, &[])]).await;

            let result = test_connection(s3_credentials(endpoint)).await;
            let requests = server.await.unwrap();

            assert!(matches!(result, Err(SyncError::Unauthorized)));
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].method, "HEAD");
        }
    }

    #[tokio::test]
    async fn missing_connection_test_is_remote_missing() {
        let (endpoint, server) =
            spawn_http_server(vec![TestResponse::empty("404 Not Found", &[])]).await;

        let result = test_connection(s3_credentials(endpoint)).await.unwrap();
        let requests = server.await.unwrap();

        assert_eq!(result, RemoteObjectState::Missing);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "HEAD");
    }

    #[tokio::test]
    async fn missing_download_is_typed_not_found() {
        let (endpoint, server) =
            spawn_http_server(vec![TestResponse::empty("404 Not Found", &[])]).await;

        let result = download(s3_credentials(endpoint)).await;
        let requests = server.await.unwrap();

        assert!(matches!(result, Err(SyncError::NotFound)));
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn slow_response_is_typed_timeout() {
        let response = TestResponse {
            head: "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            chunks: Vec::new(),
            delay: Duration::from_millis(300),
        };
        let (endpoint, server) = spawn_http_server(vec![response]).await;

        let result = download(s3_credentials(endpoint)).await;
        let requests = server.await.unwrap();

        assert!(matches!(result, Err(SyncError::Timeout)));
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn wrong_password_is_typed_decrypt_failed() {
        let encrypted = encrypt_payload(
            &SyncPayloadV2::new(secret_portable_config()),
            "server encryption password",
        )
        .unwrap();
        let (endpoint, server) =
            spawn_http_server(vec![TestResponse::body("200 OK", &[], encrypted)]).await;

        let result = download(s3_credentials(endpoint)).await;
        let requests = server.await.unwrap();

        assert!(matches!(result, Err(SyncError::DecryptFailed)));
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn malformed_envelope_is_typed_invalid_payload() {
        let sentinel = "response-secret-must-not-leak";
        let (endpoint, server) = spawn_http_server(vec![TestResponse::body(
            "200 OK",
            &[],
            format!("{{\"{sentinel}\":").into_bytes(),
        )])
        .await;

        let result = download(s3_credentials(endpoint)).await;
        let requests = server.await.unwrap();
        let error = result.unwrap_err();

        assert!(matches!(error, SyncError::InvalidPayload(_)));
        assert!(!format!("{error} {error:?}").contains(sentinel));
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn decryptable_unknown_schema_is_typed_invalid_payload() {
        let encrypted = encrypt_payload(
            &serde_json::json!({
                "schema_version": 99,
                "secret": "decrypted-secret-must-not-leak"
            }),
            "correct horse battery staple",
        )
        .unwrap();
        let (endpoint, server) =
            spawn_http_server(vec![TestResponse::body("200 OK", &[], encrypted)]).await;

        let result = download(s3_credentials(endpoint)).await;
        let requests = server.await.unwrap();
        let error = result.unwrap_err();

        assert!(matches!(error, SyncError::InvalidPayload(_)));
        assert!(!format!("{error} {error:?}").contains("decrypted-secret-must-not-leak"));
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn r2_endpoint_is_https_and_uses_region_auto() {
        let credentials = r2_credentials("0123456789abcdef0123456789abcdef");
        validate_credentials(&credentials).unwrap();
        let config = S3Config::from_backend(&credentials.backend).unwrap();
        let url = s3_url(&config).unwrap();
        let headers = signed_s3_headers("HEAD", &url, &[], &config).unwrap();
        let authorization = headers
            .get(header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap();

        assert_eq!(url.scheme(), "https");
        assert_eq!(
            url.as_str(),
            "https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com/\
             sync-bucket/configs/jshell-sync.json"
                .replace(char::is_whitespace, "")
        );
        assert!(authorization.contains("/auto/s3/aws4_request"));
        assert!(headers.get("x-amz-security-token").is_none());
    }

    #[test]
    fn fixed_aws_sigv4_vector_matches_canonical_request_hash_and_authorization() {
        let config = S3Config {
            endpoint: "https://examplebucket.s3.amazonaws.com".to_string(),
            region: "us-east-1".to_string(),
            bucket: "examplebucket".to_string(),
            object_key: "test.txt".to_string(),
            access_key: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_key: SecretString::new("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string()),
            session_token: String::new(),
        };
        let url = reqwest::Url::parse("https://examplebucket.s3.amazonaws.com/test.txt").unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let material = s3_signing_material("GET", &url, &[], &config, now).unwrap();
        let headers = signed_s3_headers_at("GET", &url, &[], &config, now).unwrap();

        let expected_canonical_request = concat!(
            "GET\n",
            "/test.txt\n",
            "\n",
            "host:examplebucket.s3.amazonaws.com\n",
            "x-amz-content-sha256:",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
            "x-amz-date:20130524T000000Z\n",
            "\n",
            "host;x-amz-content-sha256;x-amz-date\n",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(material.canonical_request, expected_canonical_request);
        assert_eq!(
            hex_sha256(material.canonical_request.as_bytes()),
            "e155673fa5bcd4b855a77a15b98fce3d10f286f93a203d6d98d2eb51f885f9b7"
        );
        assert_eq!(
            headers
                .get(header::AUTHORIZATION)
                .unwrap()
                .to_str()
                .unwrap(),
            concat!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/",
                "20130524/us-east-1/s3/aws4_request, ",
                "SignedHeaders=host;x-amz-content-sha256;x-amz-date, ",
                "Signature=df548e2ce037944d03f3e68682813b093763996d597cf890ca3d9037fd231eb4"
            )
        );
        assert_eq!(
            headers.get("x-amz-content-sha256").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(headers.get("x-amz-date").unwrap(), "20130524T000000Z");
    }

    #[test]
    fn r2_account_id_must_be_exactly_32_ascii_hex_characters() {
        for invalid in [
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdef0123456789abcdeg",
            "0123456789abcdef0123456789abcdeé",
        ] {
            assert!(matches!(
                validate_credentials(&r2_credentials(invalid)),
                Err(SyncError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn encryption_password_minimum_length_counts_unicode_characters() {
        let mut credentials = r2_credentials("0123456789abcdef0123456789abcdef");
        credentials.encryption_password = "密码只有七位吗".to_string();

        assert!(matches!(
            validate_credentials(&credentials),
            Err(SyncError::InvalidInput(_))
        ));
    }

    #[test]
    fn sync_endpoints_require_https() {
        for mut credentials in [
            webdav_credentials("http://sync.example.test/config".to_string()),
            s3_credentials("http://s3.example.test".to_string()),
            webdav_credentials("ftp://sync.example.test/config".to_string()),
        ] {
            assert!(matches!(
                validate_credentials(&credentials),
                Err(SyncError::InvalidInput(_))
            ));

            match &mut credentials.backend {
                SyncBackendCredentials::WebDav { endpoint, .. }
                | SyncBackendCredentials::S3 { endpoint, .. } => {
                    *endpoint = "https://sync.example.test/config".to_string();
                }
                SyncBackendCredentials::R2 { .. } => unreachable!(),
            }
            validate_credentials(&credentials).unwrap();
        }
    }

    #[test]
    fn tests_only_allow_http_on_ip_loopback() {
        validate_credentials(&webdav_credentials("http://127.0.0.1:8080".to_string())).unwrap();
        validate_credentials(&s3_credentials("http://[::1]:8080".to_string())).unwrap();

        for endpoint in [
            "http://localhost:8080",
            "http://126.0.0.1:8080",
            "http://192.0.2.1:8080",
        ] {
            assert!(matches!(
                validate_credentials(&webdav_credentials(endpoint.to_string())),
                Err(SyncError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn sync_redirects_must_stay_on_the_original_origin() {
        let initial = reqwest::Url::parse("https://sync.example.test/config").unwrap();
        let same_origin =
            reqwest::Url::parse("https://sync.example.test/redirected-config").unwrap();
        let other_host = reqwest::Url::parse("https://other.example.test/config").unwrap();
        let other_port = reqwest::Url::parse("https://sync.example.test:8443/config").unwrap();
        let downgraded = reqwest::Url::parse("http://sync.example.test/config").unwrap();

        let previous = std::slice::from_ref(&initial);
        assert!(sync_redirect_is_allowed(&same_origin, previous));
        assert!(!sync_redirect_is_allowed(&other_host, previous));
        assert!(!sync_redirect_is_allowed(&other_port, previous));
        assert!(!sync_redirect_is_allowed(&downgraded, previous));
    }

    #[test]
    fn v2_encrypted_payload_round_trip_is_decoded_as_v2() {
        let payload = SyncPayloadV2::new(secret_portable_config());
        let encrypted = encrypt_payload(&payload, "correct horse battery staple").unwrap();
        let decrypted = decrypt_payload(&encrypted, "correct horse battery staple").unwrap();

        let DecodedSyncPayload::V2(decrypted) = decrypted else {
            panic!("expected schema v2 payload");
        };
        assert_eq!(decrypted.revision, payload.revision);
        assert_eq!(decrypted.schema_version, 2);
        assert_eq!(decrypted.portable_config.sessions.len(), 1);
    }

    #[test]
    fn legacy_v1_envelope_is_decoded_as_legacy_v1() {
        let payload = LegacySyncPayloadV1 {
            schema_version: 1,
            revision: "legacy-revision".to_string(),
            updated_at: "2026-08-01T00:00:00Z".to_string(),
            device_id: "legacy-device-id".to_string(),
            sessions: secret_portable_config().sessions,
        };
        let encrypted = encrypt_payload(&payload, "correct horse battery staple").unwrap();

        let decrypted = decrypt_payload(&encrypted, "correct horse battery staple").unwrap();

        let DecodedSyncPayload::LegacyV1(decrypted) = decrypted else {
            panic!("expected legacy schema v1 payload");
        };
        assert_eq!(decrypted.revision, "legacy-revision");
        assert_eq!(decrypted.device_id, "legacy-device-id");
        assert_eq!(decrypted.sessions.len(), 1);
    }

    #[test]
    fn encrypted_envelope_keeps_format_v1_and_contains_no_plaintext_secrets() {
        let payload = SyncPayloadV2::new(secret_portable_config());
        let encrypted = encrypt_payload(&payload, "correct horse battery staple").unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&encrypted).unwrap();

        assert_eq!(envelope["format_version"], 1);
        assert_eq!(envelope["kdf"], "argon2id");
        assert_eq!(envelope["cipher"], "xchacha20poly1305");
        let serialized = String::from_utf8(encrypted).unwrap();
        for secret in [
            "session-password-secret-sentinel",
            "inline-private-key-secret-sentinel",
            "private-key-passphrase-secret-sentinel",
            "session-proxy-user-secret-sentinel",
            "session-proxy-password-secret-sentinel",
            "global-proxy-user-secret-sentinel",
            "global-proxy-password-secret-sentinel",
        ] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn wrong_password_is_rejected() {
        let payload = SyncPayloadV2::new(secret_portable_config());
        let encrypted = encrypt_payload(&payload, "correct horse battery staple").unwrap();
        assert!(decrypt_payload(&encrypted, "incorrect password").is_err());
    }

    #[test]
    fn endpoint_can_be_a_collection_or_file() {
        assert_eq!(
            sync_url("https://example.test/dav/"),
            "https://example.test/dav/jshell-sync.json"
        );
        assert_eq!(
            sync_url("https://example.test/config.json"),
            "https://example.test/config.json"
        );
    }

    #[test]
    fn s3_url_uses_path_style_and_encodes_object_key() {
        let config = S3Config {
            endpoint: "https://s3.example.test".into(),
            region: "us-east-1".into(),
            bucket: "my-bucket".into(),
            object_key: "configs/my file.json".into(),
            access_key: "access".into(),
            secret_key: SecretString::new("secret".into()),
            session_token: String::new(),
        };
        assert_eq!(
            s3_url(&config).unwrap().as_str(),
            "https://s3.example.test/my-bucket/configs/my%20file.json"
        );
    }

    #[test]
    fn aws_uri_encoding_preserves_only_object_key_slashes() {
        assert_eq!(aws_uri_encode("a b/c", false), "a%20b/c");
        assert_eq!(aws_uri_encode("a/b", true), "a%2Fb");
    }
}
