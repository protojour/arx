use http::header;
use tower_http::compression::Predicate;

use crate::config::ArxConfig;

#[derive(Clone)]
pub struct CompressionPredicate<'a> {
    pub cfg: &'a ArxConfig,
}

impl Predicate for CompressionPredicate<'_> {
    fn should_compress<B: http_body::Body>(&self, response: &http::Response<B>) -> bool {
        let response_content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        let response_content_size = response.body().size_hint().exact().or_else(|| {
            response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|h| h.to_str().ok())
                .and_then(|val| val.parse().ok())
        });

        // do not compress if content type is in the exempt list
        for content_type in &self.cfg.http_compression_exempt_content_types {
            if content_type == response_content_type {
                return false;
            }
        }

        // compress images?
        if response_content_type != "image/svg+xml"
            && response_content_type.starts_with("image/")
            && !self.cfg.http_compression_compress_images
        {
            return false;
        }

        // only compress when the size of the response is above the minimum
        if let Some(response_content_size) = response_content_size {
            if response_content_size < self.cfg.http_compression_min_size.as_u64() {
                return false;
            }
        }

        // default
        true
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;
    use figment::providers::{Format, Serialized, Yaml};
    use figment::Figment;
    use http::header::CONTENT_TYPE;
    use tower_http::compression::Predicate;

    use crate::config::ArxConfig;

    use super::CompressionPredicate;

    fn config_from_yaml(yaml: &str) -> Result<ArxConfig, figment::Error> {
        Figment::from(Serialized::defaults(ArxConfig::default()))
            .merge(Yaml::string(yaml))
            .extract()
    }
    fn default_config() -> Result<ArxConfig, figment::Error> {
        Figment::from(Serialized::defaults(ArxConfig::default())).extract()
    }

    #[test]
    fn http_should_compress_when_bigger_than_default_min() {
        let cfg = default_config().unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..64).map(|_| 'A').collect();
        let mock_response = axum::http::Response::new(mock_body);
        assert!(compression_predicate.should_compress(&mock_response));
    }

    #[test]
    fn http_should_not_compress_when_smaller_than_default_min() {
        let cfg = default_config().unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..22).map(|_| 'A').collect();
        let mock_response = axum::http::Response::new(mock_body);
        assert!(!compression_predicate.should_compress(&mock_response));
    }

    #[test]
    fn http_should_compress_when_bigger_than_custom_size() {
        let cfg = config_from_yaml("http_compression_min_size: 64b").unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..82).map(|_| 'A').collect();
        let mock_response = axum::http::Response::new(mock_body);
        assert!(compression_predicate.should_compress(&mock_response));
    }

    #[test]
    fn http_should_not_compress_when_smaller_than_custom_size() {
        let cfg = config_from_yaml("http_compression_min_size: 64b").unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..34).map(|_| 'A').collect();
        let mock_response = axum::http::Response::new(mock_body);
        assert!(!compression_predicate.should_compress(&mock_response));
    }

    #[test]
    fn http_should_not_compress_exempt_content_type() {
        let cfg = config_from_yaml(
            "http_compression_exempt_content_types: [\"audio/mpeg\", \"video/mp4\"]",
        )
        .unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..34).map(|_| 'A').collect();
        let mut mock_response = axum::http::Response::new(mock_body);
        mock_response
            .headers_mut()
            .append(CONTENT_TYPE, HeaderValue::try_from("audio/mpeg").unwrap());
        assert!(!compression_predicate.should_compress(&mock_response));
    }

    #[test]
    fn http_should_not_compress_image_by_default() {
        let cfg = default_config().unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..34).map(|_| 'A').collect();
        let mut mock_response = axum::http::Response::new(mock_body);
        mock_response
            .headers_mut()
            .append(CONTENT_TYPE, HeaderValue::try_from("image/jpeg").unwrap());
        assert!(!compression_predicate.should_compress(&mock_response));
    }

    #[test]
    fn http_should_compress_svg_by_default() {
        let cfg = default_config().unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..34).map(|_| 'A').collect();
        let mut mock_response = axum::http::Response::new(mock_body);
        mock_response.headers_mut().append(
            CONTENT_TYPE,
            HeaderValue::try_from("image/svg+xml").unwrap(),
        );
        assert!(compression_predicate.should_compress(&mock_response));
    }

    #[test]
    fn http_should_compress_image() {
        let cfg = config_from_yaml("http_compression_compress_images: true").unwrap();
        let compression_predicate = CompressionPredicate { cfg: &cfg };
        let mock_body: String = (0..34).map(|_| 'A').collect();
        let mut mock_response = axum::http::Response::new(mock_body);
        mock_response
            .headers_mut()
            .append(CONTENT_TYPE, HeaderValue::try_from("image/jpeg").unwrap());
        assert!(compression_predicate.should_compress(&mock_response));
    }
}
