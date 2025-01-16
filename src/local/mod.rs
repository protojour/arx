//! poor-man's low-level HTTP service system used within arx

use async_trait::async_trait;
use bytes::Bytes;
use http::{header, HeaderName, HeaderValue};
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use health::health;

use crate::hyper::{DynHttpError, HttpError, HyperResponse};

mod health;

type Res = Result<HyperResponse, HttpError>;

fn match_get(req: &http::Request<Incoming>) -> Result<(), HttpError> {
    match req.method() {
        &Method::GET => Ok(()),
        _ => Err(HttpError::Static(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed",
        )),
    }
}

/// HTTP services implemented by the gateway itself
#[async_trait]
pub trait LocalService {
    async fn handle(&self, req: http::Request<Incoming>) -> Res;
    fn replace_prefix(&self) -> Option<&str> {
        Some("/")
    }
}

#[derive(Clone)]
pub struct Onto;

#[async_trait]
impl LocalService for Onto {
    async fn handle(&self, req: http::Request<Incoming>) -> Res {
        self.handle_inner(req).await
    }
}

impl Onto {
    async fn handle_inner(&self, req: http::Request<Incoming>) -> Res {
        let service = ServeDir::new("onto").fallback(ServeFile::new("onto/index.html"));
        let mut oneshot = service.oneshot(req).await.unwrap();
        let headers = oneshot.headers_mut();
        headers.append(
            HeaderName::from_static("cross-origin-embedder-policy"),
            HeaderValue::from_static("credentialless"),
        );
        headers.append(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        );
        headers.append(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("cross-origin"),
        );
        Ok(oneshot.map(|body| {
            body.map_err(|err| -> DynHttpError { Box::new(err) })
                .boxed_unsync()
        }))
    }
}

pub struct Docs;

#[async_trait]
impl LocalService for Docs {
    async fn handle(&self, req: http::Request<Incoming>) -> Res {
        let service = ServeDir::new("docs").fallback(ServeFile::new("docs/index.html"));

        Ok(service.oneshot(req).await.unwrap().map(|body| {
            body.map_err(|err| -> DynHttpError { Box::new(err) })
                .boxed_unsync()
        }))
    }
}

pub struct Static;

#[async_trait]
impl LocalService for Static {
    async fn handle(&self, req: http::Request<Incoming>) -> Res {
        let service = ServeDir::new("static");

        Ok(service.oneshot(req).await.unwrap().map(|body| {
            body.map_err(|err| -> DynHttpError { Box::new(err) })
                .boxed_unsync()
        }))
    }
}

pub struct Health {
    pub client: reqwest::Client,
}

#[async_trait]
impl LocalService for Health {
    async fn handle(&self, req: http::Request<Incoming>) -> Res {
        match_get(&req)?;
        let health_data = health(&self.client).await;
        let json: Bytes = serde_json::to_vec(&health_data).unwrap().into();

        Ok(http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::new(json).map_err(|err| match err {}).boxed_unsync())
            .unwrap())
    }
}

pub struct Services {}

#[async_trait]
impl LocalService for Services {
    async fn handle(&self, req: http::Request<Incoming>) -> Res {
        match_get(&req)?;
        let services: Vec<()> = vec![];
        let json: Bytes = serde_json::to_vec(&services).unwrap().into();

        Ok(http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::new(json).map_err(|err| match err {}).boxed_unsync())
            .unwrap())
    }
}
