use std::borrow::Cow;

use http::{header::HOST, HeaderName, HeaderValue, StatusCode, Uri};
use hyper::body::Incoming;
use tracing::error;

use crate::hyper::HttpError;

const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
const X_FORWARDED_PORT: HeaderName = HeaderName::from_static("x-forwarded-port");
const X_FORWARDED_PREFIX: HeaderName = HeaderName::from_static("x-forwarded-prefix");

pub fn set_proxy_headers(
    req: &mut http::Request<Incoming>,
    original_uri: &Uri,
) -> Result<(), HttpError> {
    let prefix = original_uri.path().strip_suffix(req.uri().path());
    let headers = req.headers_mut();

    let host_header = headers.remove(HOST);
    let host_port = host_header
        .as_ref()
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.split_once(':'));

    if !headers.contains_key(X_FORWARDED_PROTO) {
        // for now, Arx always runs plain HTTP.
        // FIXME: Support HTTPS natively
        headers.insert(X_FORWARDED_PROTO, HeaderValue::from_static("http"));
    }

    // if headers already contain x-forwarded-host from another proxy, don't touch it
    if !headers.contains_key(X_FORWARDED_HOST) {
        if let Some((host, _port)) = host_port.as_ref() {
            headers.insert(
                X_FORWARDED_HOST,
                HeaderValue::from_str(host).map_err(|_| {
                    error!("invalid host: {}", host);
                    HttpError::Static(StatusCode::BAD_REQUEST, "")
                })?,
            );
        }
    }

    if !headers.contains_key(X_FORWARDED_PORT) {
        if let Some((_host, port)) = host_port.as_ref() {
            headers.insert(
                X_FORWARDED_PORT,
                HeaderValue::from_str(port).map_err(|_| {
                    error!("invalid port: {}", port);
                    HttpError::Static(StatusCode::BAD_REQUEST, "")
                })?,
            );
        }
    }

    if let Some(prefix) = prefix {
        let new_prefix = match headers.get(X_FORWARDED_PREFIX) {
            Some(prev_prefix) => prev_prefix
                .to_str()
                .map(|prev_prefix| Cow::Owned(format!("{prev_prefix}{prefix}")))
                .unwrap_or(Cow::Borrowed(prefix)),
            None => Cow::Borrowed(prefix),
        };

        headers.insert(
            X_FORWARDED_PREFIX,
            HeaderValue::from_str(&new_prefix).map_err(|_| {
                error!("invalid prefix: {}", new_prefix);
                HttpError::Static(StatusCode::BAD_REQUEST, "")
            })?,
        );
    }

    Ok(())
}
