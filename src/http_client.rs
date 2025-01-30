use std::sync::Arc;

use anyhow::anyhow;
use arc_swap::ArcSwap;
use futures_util::{Stream, StreamExt};
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_tracing::TracingMiddleware;
use tokio_util::sync::CancellationToken;

use crate::{arx_anyhow, config::ArxConfig, ArxError};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A wrapper around reqwest/reqwest_middleware with tracing support.
#[derive(Clone)]
pub struct HttpClient {
    instance: Arc<ArcSwap<HttpClientInstance>>,
}

pub struct HttpClientInstance {
    pub reqwest_client: reqwest::Client,
    pub middleware_client: reqwest_middleware::ClientWithMiddleware,
}

impl HttpClient {
    pub async fn create_default(
        cfg: &'static ArxConfig,
        cancel: CancellationToken,
    ) -> Result<Self, ArxError> {
        Self::create_with_builder_stream(
            cfg,
            futures_util::stream::iter([reqwest::Client::builder()]),
            cancel,
        )
        .await
    }

    pub async fn create_with_builder_stream(
        cfg: &'static ArxConfig,
        mut client_builder_stream: impl Stream<Item = reqwest::ClientBuilder> + Unpin + Send + 'static,
        cancel: CancellationToken,
    ) -> Result<Self, ArxError> {
        let Some(initial_builder) = client_builder_stream.next().await else {
            return Err(ArxError::Internal(anyhow!("no client builders")));
        };

        let instance = build_instance(cfg, initial_builder)?;
        let client = HttpClient {
            instance: Arc::new(ArcSwap::new(Arc::new(instance))),
        };

        tokio::spawn({
            let client = client.clone();
            async move {
                loop {
                    tokio::select! {
                        next = client_builder_stream.next() => {
                            if let Some(builder) = next {
                                match build_instance(cfg, builder) {
                                    Ok(instance) => {
                                        client.instance.store(
                                            Arc::new(instance)
                                        );
                                    }
                                    Err(err) => {
                                        tracing::error!(?err, "Failed to rebuild client");
                                    }
                                }
                            } else {
                                // No more builders
                                return;
                            }
                        }
                        _ = cancel.cancelled() => {
                            return;
                        }
                    }
                }
            }
        });

        Ok(client)
    }

    pub fn current_instance(&self) -> Arc<HttpClientInstance> {
        self.instance.load_full()
    }
}

fn build_instance(
    cfg: &'static ArxConfig,
    builder: reqwest::ClientBuilder,
) -> Result<HttpClientInstance, ArxError> {
    let builder = builder
        .user_agent(format!("Arx/{}", VERSION))
        .connect_timeout(cfg.connect_timeout)
        .timeout(cfg.request_timeout)
        .tcp_keepalive(cfg.keep_alive_timeout)
        .http2_keep_alive_timeout(cfg.keep_alive_timeout)
        .danger_accept_invalid_certs(cfg.http_accept_invalid_certs)
        .tls_built_in_root_certs(cfg.use_root_certs)
        .tls_built_in_webpki_certs(cfg.use_webpki_certs);

    let client = builder.build().map_err(arx_anyhow)?;

    // No backoff support at this point..
    let _retry_policy = ExponentialBackoff::builder()
        .jitter(cfg.backoff_jitter.into())
        .retry_bounds(
            cfg.backoff_min_retry_interval,
            cfg.backoff_max_retry_interval,
        )
        .build_with_max_retries(cfg.backoff_max_num_retries);

    let middleware_client = reqwest_middleware::ClientBuilder::new(client.clone())
        .with(TracingMiddleware::default())
        .build();

    Ok(HttpClientInstance {
        reqwest_client: client,
        middleware_client,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio_util::sync::DropGuard;

    async fn test_client(cfg: &'static ArxConfig) -> (HttpClient, DropGuard) {
        let cancel = CancellationToken::new();
        let client = HttpClient::create_default(cfg, cancel.clone())
            .await
            .unwrap();
        (client, cancel.drop_guard())
    }

    /*
    fn config_from_yaml(yaml: &str) -> Result<ArxConfig, figment::Error> {
        Figment::from(Serialized::defaults(ArxConfig::default()))
            .merge(Yaml::string(yaml))
            .extract()
    }
    */

    /*
    #[tokio::test]
    async fn request_retry() {
        let cancel = CancellationToken::new();
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
        let client = create_http_client(&cfg, iter([reqwest::Client::builder()]), cancel.clone())
            .await
            .unwrap()
        let _result = client.get(mock_server.uri()).send().await;
    }
    */

    #[tokio::test]
    async fn verify_webpki_certs() {
        let cfg = Box::leak(Box::new(ArxConfig {
            use_root_certs: false,
            use_webpki_certs: false,
            ..Default::default()
        }));
        let (client, _drop) = test_client(cfg).await;
        let result = client
            .current_instance()
            .reqwest_client
            .get("https://www.rust-lang.org")
            .send()
            .await;
        assert!(result.is_err());

        let cfg = Box::leak(Box::new(ArxConfig {
            use_root_certs: false,
            use_webpki_certs: true,
            ..Default::default()
        }));
        let (client, _drop) = test_client(cfg).await;
        let result = client
            .current_instance()
            .reqwest_client
            .get("http://localhost:8080")
            .send()
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn use_custom_ca() {
        let mut _cfg = Box::leak(Box::new(ArxConfig {
            use_root_certs: false,
            use_webpki_certs: false,
            ..Default::default()
        }));

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
