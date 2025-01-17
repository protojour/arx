use std::sync::Arc;

use anyhow::Context;
use arc_swap::ArcSwap;
use config::ArxConfig;
use gateway::{serve_gateway, Backends, Gateway, GatewayState};
use http_client::HttpClient;
use k8s::k8s_routing::{self, spawn_k8s_watchers};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tower_server::Scheme;

pub mod config;

mod authentication;
mod gateway;
mod http_client;
mod hyper;
mod k8s;
mod layers;
mod local;
mod reverse_proxy;
mod route;
mod static_routes;

#[derive(Error, Debug)]
enum ArxError {
    #[error("not authenticated")]
    NotAuthenticated,

    #[error("internal: {0}")]
    Internal(anyhow::Error),
}

fn arx_anyhow(error: impl Into<anyhow::Error>) -> ArxError {
    ArxError::Internal(error.into())
}

pub async fn run(cfg: ArxConfig) -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // just leak the config, it's a singleton
    let cfg = Box::leak(Box::new(cfg));

    let cancel = termination_signal();

    let default_http_client = HttpClient::new(cfg).map_err(arx_anyhow)?;

    let (authly_client, authly_http_client) = {
        let authly_client_builder = authly_client::Client::builder()
            .with_url(cfg.authly_url.clone())
            .from_environment()
            .await?;

        let authly_http_client = HttpClient::with_tls_configured_builder(
            reqwest::Client::builder()
                .add_root_certificate(reqwest::tls::Certificate::from_pem(
                    &authly_client_builder.get_local_ca_pem()?,
                )?)
                .identity(reqwest::Identity::from_pem(
                    &authly_client_builder.get_identity_pem()?,
                )?),
            cfg,
        )?;

        let authly_client = authly_client_builder.connect().await?;

        (authly_client, authly_http_client)
    };

    let http_server = tower_server::Builder::new("0.0.0.0:80".parse().unwrap())
        .with_scheme(Scheme::Http)
        .with_cancellation_token(cancel.clone())
        .bind()
        .await
        .context("failed to bind http server")?;

    let routes = Arc::new(ArcSwap::new(Arc::new(k8s_routing::rebuild_routing_table(
        &Default::default(),
        default_http_client.reqwest_client().clone(),
    )?)));

    let gateway = Gateway::new(GatewayState {
        routes: routes.clone(),
        backends: Backends {
            default: default_http_client.clone(),
            authly: authly_http_client,
        },
        authly_client: Some(authly_client),
        cfg,
    });

    spawn_k8s_watchers(
        routes,
        default_http_client.reqwest_client().clone(),
        cancel.clone(),
    )
    .await?;

    tokio::spawn(serve_gateway(gateway, http_server));

    cancel.cancelled().await;

    Ok(())
}

fn termination_signal() -> CancellationToken {
    let cancel = CancellationToken::new();
    tokio::spawn({
        let cancel = cancel.clone();
        async move {
            let terminate = async {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to install signal handler")
                    .recv()
                    .await;
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    cancel.cancel();
                }
                _ = terminate => {
                    cancel.cancel();
                }
            }
        }
    });

    cancel
}
