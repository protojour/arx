use std::sync::Arc;

use anyhow::Context;
use arc_swap::ArcSwap;
use config::ArxConfig;
use gateway::{serve_gateway, Backends, Gateway, GatewayState};
use http_client::HttpClient;
use k8s::k8s_routing::{self, spawn_k8s_watchers};
use thiserror::Error;
use tower_server::Scheme;

pub mod config;

mod authentication;
mod gateway;
mod headers;
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

    let cancel = tower_server::signal::termination_signal();

    let default_http_client = HttpClient::create_default(cfg, cancel.clone()).await?;

    let (authly_client, authly_http_client) = {
        let authly_client_builder = authly_client::Client::builder()
            .with_url(cfg.authly_url.clone())
            .from_environment()
            .await?;

        let authly_client = authly_client_builder.connect().await?;

        let authly_http_client = HttpClient::create_with_builder_stream(
            cfg,
            authly_client.request_client_builder_stream()?,
            cancel.clone(),
        )
        .await?;

        (authly_client, authly_http_client)
    };

    let http_server = tower_server::Builder::new("0.0.0.0:80".parse().unwrap())
        .with_scheme(Scheme::Http)
        .with_graceful_shutdown(cancel.clone())
        .bind()
        .await
        .context("failed to bind http server")?;

    let routes = Arc::new(ArcSwap::new(Arc::new(k8s_routing::rebuild_routing_table(
        &Default::default(),
        default_http_client
            .current_instance()
            .reqwest_client
            .clone(),
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
        default_http_client
            .current_instance()
            .reqwest_client
            .clone(),
        cancel.clone(),
    )
    .await?;

    tokio::spawn(serve_gateway(gateway, http_server));

    cancel.cancelled().await;

    Ok(())
}
