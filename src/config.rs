use std::{fmt::Display, str::FromStr, time::Duration};

use bytesize::ByteSize;
use http::HeaderName;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use url::Url;

#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ArxConfig {
    /// Overrides the default log level.
    pub log_level: String,
    /// Enables logging of HTTP requests.
    pub access_log: bool,

    /// Url for connecting to the Authly service.
    pub authly_url: Url,

    /// Maximum size of a request.
    pub request_max_size: ByteSize,
    /// Timeout waiting for a request to complete.
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
    /// Timeout waiting for a request to complete.
    #[serde(with = "humantime_serde")]
    pub request_timeout: Duration,
    /// Timeout for processing and returning a response.
    #[serde(with = "humantime_serde")]
    pub response_timeout: Duration,
    /// Timeout for keeping a TCP connection open when using the `keep-alive` header.
    #[serde(with = "humantime_serde")]
    pub keep_alive_timeout: Duration,
    /// Whether the HTTP client accepts invalid certificates. Should remain false unless you're debugging.
    pub http_accept_invalid_certs: bool,
    /// Use system root CA certs. Not available in the official Memoriam Docker images.
    pub use_root_certs: bool,
    /// Use bundled Mozilla CA certs. Should be true when using official Memoriam Docker images.
    pub use_webpki_certs: bool,

    /// Minimum retry interval for cross-service requests using exponential backoff.
    #[serde(with = "humantime_serde")]
    pub backoff_min_retry_interval: Duration,
    /// Maximum retry interval for cross-service requests using exponential backoff. [G,D,S]
    #[serde(with = "humantime_serde")]
    pub backoff_max_retry_interval: Duration,
    /// Maximum number of retries for cross-service requests using exponential backoff. [G,D,S]
    pub backoff_max_num_retries: u32,
    /// How to apply jitter when retrying cross-service requests with exponential backoff.
    /// Valid options are "none", "full" or "bounded"
    /// (documented [here](https://docs.rs/retry-policies/0.2.0/retry_policies/enum.Jitter.html)). [G,D,S]
    pub backoff_jitter: Jitter,

    /// HTTP compression level. Valid options are "fastest", "best", "default",
    /// or a number (as a string) that sets a precise level for a specific compression algorithm.
    /// Memoriam supports brotli, gzip and deflate compression. [G]
    #[serde_as(as = "DisplayFromStr")]
    pub http_compression_level: CompressionLevel,
    /// Minimum size of an HTTP response for compression. Responses below this size are not compressed. [G]
    pub http_compression_min_size: ByteSize,
    /// Whether HTTP responses with an image content type should be compressed. [G]
    pub http_compression_compress_images: bool,
    /// Comma-separated list of content types for which compression should be disabled. [G]
    pub http_compression_exempt_content_types: Vec<String>,

    /// Value of the CORS header `access-control-allow-origin`. [G]
    pub cors_allow_origin: String,
    /// Value of the CORS header `access-control-allow-methods`. [G]
    pub cors_allow_methods: Vec<Method>,
    /// Value of the CORS header `access-control-allow-headers`. [G]
    pub cors_allow_headers: Vec<String>,
    /// Value of the CORS header `access-control-expose-headers`. [G]
    pub cors_expose_headers: Vec<String>,
    /// Value of the CORS header `access-control-allow-credentials`. [G]
    pub cors_allow_credentials: bool,
    /// Value of the CORS header `access-control-allow-private-network`. [G]
    pub cors_allow_private_network: bool,
    /// Value of the CORS header `access-control-max-age`. [G]
    #[serde(with = "humantime_serde")]
    pub cors_max_age: Duration,
}

impl Default for ArxConfig {
    fn default() -> ArxConfig {
        ArxConfig {
            log_level: "INFO".into(),
            access_log: false,

            authly_url: "https://authly".parse().unwrap(),

            request_max_size: ByteSize::gb(20),
            connect_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(60),
            response_timeout: Duration::from_secs(60),
            keep_alive_timeout: Duration::from_secs(15),
            http_accept_invalid_certs: false,
            use_root_certs: true,
            use_webpki_certs: true,

            backoff_min_retry_interval: Duration::from_secs(1),
            backoff_max_retry_interval: Duration::from_secs(30 * 60),
            backoff_max_num_retries: 30,
            backoff_jitter: Jitter::Full,

            http_compression_level: CompressionLevel::from_str("default").unwrap(),
            http_compression_min_size: ByteSize::b(32),
            http_compression_compress_images: false,
            http_compression_exempt_content_types: vec![],

            cors_allow_origin: "*".into(),
            cors_allow_methods: vec![Method::Any],
            cors_allow_headers: vec!["*".into()],
            cors_expose_headers: vec![
                "cache-control".into(),
                "content-language".into(),
                "content-length".into(),
                "content-type".into(),
                "expires".into(),
                "last-modified".into(),
                "pragma".into(),
            ],
            cors_allow_credentials: false,
            cors_allow_private_network: true,
            cors_max_age: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Jitter {
    None,
    Full,
    Bounded,
}

impl From<Jitter> for retry_policies::Jitter {
    fn from(value: Jitter) -> Self {
        match value {
            Jitter::None => retry_policies::Jitter::None,
            Jitter::Full => retry_policies::Jitter::Full,
            Jitter::Bounded => retry_policies::Jitter::Bounded,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Options,
    Get,
    Post,
    Put,
    Delete,
    Head,
    Trace,
    Connect,
    Patch,
    #[serde(rename = "*")]
    Any,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompressionLevel(pub tower_http::compression::CompressionLevel);

impl FromStr for CompressionLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fastest" => Ok(Self(tower_http::CompressionLevel::Fastest)),
            "best" => Ok(Self(tower_http::CompressionLevel::Best)),
            "default" => Ok(Self(tower_http::CompressionLevel::Default)),
            other if !other.is_empty() && other.chars().next().unwrap().is_ascii_digit() => {
                i32::from_str(other)
                    .map(|precise| Self(tower_http::CompressionLevel::Precise(precise)))
                    .map_err(|err| format!("ParseIntError: {err}"))
            }
            _ => Err("Unrecognized compression level".into()),
        }
    }
}
impl Display for CompressionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            tower_http::CompressionLevel::Fastest => write!(f, "fastest"),
            tower_http::CompressionLevel::Best => write!(f, "best"),
            tower_http::CompressionLevel::Default => write!(f, "default"),
            tower_http::CompressionLevel::Precise(number) => write!(f, "{number}"),
            _ => unreachable!("Unknown CompressionLevel"),
        }
    }
}

pub fn to_allow_methods(methods: &[Method]) -> Result<Vec<http::Method>, tower_http::cors::Any> {
    if methods.iter().any(|method| matches!(method, Method::Any)) {
        Err(tower_http::cors::Any)
    } else {
        Ok(methods
            .iter()
            .map(|method| match method {
                Method::Options => http::Method::OPTIONS,
                Method::Get => http::Method::GET,
                Method::Post => http::Method::POST,
                Method::Put => http::Method::PUT,
                Method::Delete => http::Method::DELETE,
                Method::Head => http::Method::HEAD,
                Method::Trace => http::Method::TRACE,
                Method::Connect => http::Method::CONNECT,
                Method::Patch => http::Method::PATCH,
                Method::Any => unreachable!(),
            })
            .collect())
    }
}

pub fn to_headernames(headers: &[String]) -> OrAny<Vec<HeaderName>> {
    if headers.iter().any(|header| header == "*") {
        OrAny::Any
    } else {
        OrAny::Given(
            headers
                .iter()
                .map(|h| http::header::HeaderName::from_str(&h.to_lowercase()).unwrap())
                .collect(),
        )
    }
}

pub enum OrAny<T> {
    Given(T),
    Any,
}
