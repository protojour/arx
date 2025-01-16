use http::HeaderValue;
use http_compression::CompressionPredicate;
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowHeaders, AllowMethods, Any, CorsLayer, ExposeHeaders},
};

use crate::config::{to_allow_methods, to_headernames, ArxConfig, OrAny};

pub mod http_compression;

pub fn compression_layer(cfg: &ArxConfig) -> CompressionLayer<CompressionPredicate> {
    CompressionLayer::new()
        .quality(cfg.http_compression_level.0)
        .compress_when(CompressionPredicate { cfg })
}

pub fn cors_layer(cfg: &'static ArxConfig) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(HeaderValue::from_static(&cfg.cors_allow_origin))
        .allow_methods(match to_allow_methods(&cfg.cors_allow_methods) {
            Ok(methods) => AllowMethods::from(methods),
            Err(any) => AllowMethods::from(any),
        })
        .allow_headers(match to_headernames(&cfg.cors_allow_headers) {
            OrAny::Any => AllowHeaders::from(Any),
            OrAny::Given(headers) => AllowHeaders::from(headers),
        })
        .allow_credentials(cfg.cors_allow_credentials)
        .allow_private_network(cfg.cors_allow_private_network)
        .expose_headers(match to_headernames(&cfg.cors_expose_headers) {
            OrAny::Any => ExposeHeaders::from(Any),
            OrAny::Given(headers) => ExposeHeaders::from(headers),
        })
        .max_age(cfg.cors_max_age)
}
