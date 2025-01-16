use anyhow::Context;
use tokio_util::sync::CancellationToken;
use tower_server::Scheme;
use url::Url;

mod authentication;
mod config;
mod gateway;
mod http_client;
mod hyper;
mod k8s_routing;
mod layers;
mod local;
mod reverse_proxy;
mod route;

#[derive(clap::Parser)]
pub struct Config {
    /// Sets the log level
    #[clap(env, default_value = "INFO")]
    pub log_level: tracing::Level,

    /// The URL of the local Authly service
    #[clap(env, default_value = "https://authly")]
    pub authly_url: Url,
}

#[derive(Debug)]
enum ArxError {
    NotAuthenticated,

    Internal(anyhow::Error),
}

fn arx_anyhow(error: impl Into<anyhow::Error>) -> ArxError {
    ArxError::Internal(error.into())
}

pub async fn run(config: Config) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();

    let (authly_client, authly_http_client) = {
        let authly_client_builder = authly_client::Client::builder().from_environment().await?;

        let authly_http_client = reqwest::Client::builder()
            .add_root_certificate(reqwest::tls::Certificate::from_pem(
                &authly_client_builder.get_local_ca_pem()?,
            )?)
            .identity(reqwest::Identity::from_pem(
                &authly_client_builder.get_identity_pem()?,
            )?)
            .build()?;

        let authly_client = authly_client_builder.connect().await?;

        (authly_client, authly_http_client)
    };

    let server = tower_server::Builder::new("0.0.0.0:80".parse().unwrap())
        .with_scheme(Scheme::Http)
        .with_cancellation_token(cancel.clone())
        .bind()
        .await
        .context("failed to bind http server")?;

    // TODO: setup routes!

    Ok(())
}
