use std::env;

use arx::config::ArxConfig;
use clap::Parser;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::OTEL_EXPORTER_OTLP_ENDPOINT;
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    runtime,
    trace::{RandomIdGenerator, Sampler, TracerProvider},
    Resource,
};
use tracing::{info, level_filters::LevelFilter, Level};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(clap::Parser)]
pub struct Cli {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    let cfg = ArxConfig::from_env();

    let tracing_layer = tracing_subscriber::registry()
        // coarse-grained filtering
        .with(LevelFilter::from(cfg.log_level.parse::<Level>()?))
        .with(tracing_subscriber::fmt::layer().with_target(false));

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    if env::var(OTEL_EXPORTER_OTLP_ENDPOINT).is_ok() {
        let resource = Resource::from_schema_url(
            [
                KeyValue::new("service.name", "arx"),
                KeyValue::new("service.version", VERSION),
                // KeyValue::new("deployment.environment.name", "develop"),
            ],
            "https://opentelemetry.io/schemas/1.27.0",
        );

        let provider = TracerProvider::builder()
            .with_resource(resource)
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
                1.0,
            ))))
            // If export trace to AWS X-Ray, you can use XrayIdGenerator
            .with_id_generator(RandomIdGenerator::default())
            .with_batch_exporter(
                opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .build()
                    .unwrap(),
                runtime::Tokio,
            )
            .build();

        opentelemetry::global::set_tracer_provider(provider.clone());
        let tracer = provider.tracer("tracing-otel-subscriber");

        tracing_layer.with(OpenTelemetryLayer::new(tracer)).init();
    } else {
        let provider = TracerProvider::builder().build();
        let tracer = provider.tracer("noop");
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_layer.with(telemetry).init();
    }

    info!("🏰 Arx v{VERSION}");

    arx::run(cfg).await?;

    Ok(())
}
