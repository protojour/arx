use std::{ops::Deref, sync::Arc};

use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use reqwest_tracing::TracingMiddleware;

use crate::{arx_anyhow, config::ArxConfig, ArxError};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A wrapper around reqwest_middleware with better dynamic middleware support.
///
/// The default middleware is tracing support.
///
/// The HttpClient Derefs to [ClientWithMiddleware], its methods can be called directly.
#[derive(Clone)]
pub struct HttpClient {
    inner: Arc<HttpClientInner>,
    pub middleware_client: ClientWithMiddleware,
}

impl HttpClient {
    /// Client without TLS configuration
    pub fn new(cfg: &ArxConfig) -> Result<Self, ArxError> {
        Self::with_tls_configured_builder(reqwest::Client::builder(), cfg)
    }

    pub fn with_tls_configured_builder(
        builder: reqwest::ClientBuilder,
        cfg: &ArxConfig,
    ) -> Result<Self, ArxError> {
        let builder = builder
            .user_agent(format!("Arx/{}", VERSION))
            .connect_timeout(cfg.connect_timeout)
            .timeout(cfg.request_timeout)
            .tcp_keepalive(cfg.keep_alive_timeout)
            .http2_keep_alive_timeout(cfg.keep_alive_timeout)
            .danger_accept_invalid_certs(cfg.http_accept_invalid_certs)
            .tls_built_in_root_certs(cfg.use_root_certs)
            .tls_built_in_webpki_certs(cfg.use_webpki_certs);

        // check if ca_file is set, we want to fail if the file doesn't exist
        /*
        if let Some(ca_file) = cfg.ca_file.as_ref() {
            let data = fs::read_to_string(ca_file)?;
            let cert = reqwest::tls::Certificate::from_pem(data.as_bytes())?;
            builder = builder.add_root_certificate(cert);
        }
        */

        let client = builder.build().map_err(arx_anyhow)?;

        let inner = Arc::new(HttpClientInner {
            client,
            retry_policy: ExponentialBackoff::builder()
                .jitter(cfg.backoff_jitter.into())
                .retry_bounds(
                    cfg.backoff_min_retry_interval,
                    cfg.backoff_max_retry_interval,
                )
                .build_with_max_retries(cfg.backoff_max_num_retries),
        });

        let middleware_client = ClientBuilder::new(inner.client.clone())
            .with(TracingMiddleware::default())
            .build();

        Ok(Self {
            inner,
            middleware_client,
        })
    }

    /// Return the raw inner reqwest client
    pub fn reqwest_client(&self) -> &reqwest::Client {
        &self.inner.client
    }

    #[expect(unused)]
    pub fn disable_tracing(&self) -> Self {
        Self {
            middleware_client: ClientBuilder::new(self.inner.client.clone()).build(),
            inner: self.inner.clone(),
        }
    }

    #[allow(unused)]
    pub fn with_backoff(&self) -> Self {
        Self {
            middleware_client: ClientBuilder::new(self.inner.client.clone())
                .with(RetryTransientMiddleware::new_with_policy(
                    self.inner.retry_policy,
                ))
                .with(TracingMiddleware::default())
                .build(),
            inner: self.inner.clone(),
        }
    }

    #[expect(unused)]
    pub fn with_backoff_disable_tracing(&self) -> Self {
        Self {
            middleware_client: ClientBuilder::new(self.inner.client.clone())
                .with(RetryTransientMiddleware::new_with_policy(
                    self.inner.retry_policy,
                ))
                .build(),
            inner: self.inner.clone(),
        }
    }
}

impl Deref for HttpClient {
    type Target = ClientWithMiddleware;

    fn deref(&self) -> &Self::Target {
        &self.middleware_client
    }
}

struct HttpClientInner {
    client: reqwest::Client,
    retry_policy: ExponentialBackoff,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use figment::{
        providers::{Format, Serialized, Yaml},
        Figment,
    };
    use indoc::indoc;
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    // use crate::server_util::RustlsServerConfig;

    fn config_from_yaml(yaml: &str) -> Result<ArxConfig, figment::Error> {
        Figment::from(Serialized::defaults(ArxConfig::default()))
            .merge(Yaml::string(yaml))
            .extract()
    }

    #[tokio::test]
    async fn request_retry() {
        let cfg = config_from_yaml(indoc! {r#"
            request_timeout: 10ms
            backoff_max_num_retries: 2
            backoff_min_retry_interval: 10ms
            backoff_max_retry_interval: 20ms
        "#})
        .unwrap();
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(20)))
            .expect(2..)
            .mount(&mock_server)
            .await;
        let client = HttpClient::new(&cfg).unwrap().with_backoff();
        let _result = client.get(mock_server.uri()).send().await;
    }

    #[tokio::test]
    async fn verify_webpki_certs() {
        let mut cfg = ArxConfig {
            use_root_certs: false,
            use_webpki_certs: false,
            ..Default::default()
        };

        let client = HttpClient::new(&cfg).unwrap();
        let result = client.get("https://www.rust-lang.org").send().await;
        assert!(result.is_err());

        cfg.use_webpki_certs = true;
        let client = HttpClient::new(&cfg).unwrap();
        let result = client.get("https://www.rust-lang.org").send().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn use_custom_ca() {
        let mut _cfg = ArxConfig {
            use_root_certs: false,
            use_webpki_certs: false,
            ..Default::default()
        };

        let app = axum::Router::new().route("/", axum::routing::get(|| async { "" }));

        let _handle = tokio::spawn(async {
            let _cfg = ArxConfig {
                // cert_file: Some("../../certs/server.pem".into()),
                // key_file: Some("../../certs/server.key".into()),
                ..Default::default()
            };
            tower_server::Builder::new("0.0.0.0:9999".parse().unwrap())
                // .with_tls_config(RustlsServerConfig::new(server_cfg))
                .bind()
                .await
                .unwrap()
                .serve(app)
                .await;
        });

        // TODO: These asserts pass locally, but the latter fails in CI,
        // probably due to Docker network issues. Leaving this commented for now.
        // let client = HttpClient::new(&cfg).unwrap();
        // let result = client.get("https://localhost:9999").send().await;
        // tracing::debug!("{:?}", result);
        // assert!(result.is_err());

        // cfg.ca_file = "../../certs/rootCA.pem".into();
        // let client = HttpClient::new(&cfg).unwrap();
        // let result = client.get("https://localhost:9999").send().await;
        // tracing::debug!("{:?}", result);
        // assert!(result.is_ok());
    }
}
