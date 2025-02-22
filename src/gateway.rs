use std::sync::Arc;

use arc_swap::ArcSwap;
use http::{header, HeaderValue, Request, StatusCode, Uri};
use tower::ServiceBuilder;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{debug, error, trace, Level};

use crate::{
    authentication::process_auth_directive,
    config::ArxConfig,
    headers::set_proxy_headers,
    http_client::{HttpClient, HttpClientInstance},
    hyper::{empty_body, HttpError, HyperResponse},
    layers::{compression_layer, cors_layer},
    local::LocalService,
    reverse_proxy::reverse_proxy,
    route::{AuthDirective, BackendClass, Route},
};

#[derive(Clone)]
pub struct Gateway {
    state: Arc<GatewayState>,
}

pub struct GatewayState {
    pub routes: Arc<ArcSwap<matchit::Router<Route>>>,
    pub backends: Backends,
    pub authly_client: Option<authly_client::Client>,
    pub cfg: &'static ArxConfig,
}

pub struct Backends {
    pub default: HttpClient,
    /// A HTTP client mTLS-configured for Authly
    pub authly: HttpClient,
}

/// serve the gateway on a bound HttpServer
pub async fn serve_gateway(
    gateway: Gateway,
    http_server: tower_server::TowerServer,
) -> anyhow::Result<()> {
    let tower_layer = ServiceBuilder::new()
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(
                    DefaultMakeSpan::new()
                        .level(Level::INFO)
                        .include_headers(false),
                )
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(compression_layer(gateway.state.cfg))
        .layer(cors_layer(gateway.state.cfg));

    http_server
        .serve(tower_layer.service_fn(move |req| {
            let gateway = gateway.clone();
            async move { gateway.serve_request(req).await }
        }))
        .await;

    Ok(())
}

enum RouteMatch {
    Proxy {
        // The HTTP client to use when proxying
        http_client_instance: Arc<HttpClientInstance>,
        req: Request<hyper::body::Incoming>,
        auth_directive: AuthDirective,
    },
    LocalService {
        req: Request<hyper::body::Incoming>,
        service: Arc<dyn LocalService + Send + Sync>,
    },
    TemporaryRedirect(Uri),
}

impl Gateway {
    pub fn new(state: GatewayState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }

    async fn serve_request(
        &self,
        req: Request<hyper::body::Incoming>,
    ) -> Result<HyperResponse, hyper::Error> {
        match self.serve_request_inner(req).await {
            Ok(response) => Ok(response),
            Err(error) => Ok(error.into_hyper_response()),
        }
    }

    async fn serve_request_inner(
        &self,
        req: Request<hyper::body::Incoming>,
    ) -> Result<HyperResponse, HttpError> {
        match self.match_route(req)? {
            RouteMatch::Proxy {
                http_client_instance,
                mut req,
                auth_directive,
            } => {
                process_auth_directive(
                    auth_directive,
                    req.headers_mut(),
                    self.state.authly_client.as_ref(),
                )
                .await
                .map_err(|_| HttpError::Static(StatusCode::UNAUTHORIZED, "unauthorized"))?;

                reverse_proxy(req, &http_client_instance).await
            }
            RouteMatch::TemporaryRedirect(uri) => Ok(http::Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header(
                    header::LOCATION,
                    HeaderValue::from_str(&uri.to_string()).unwrap(),
                )
                .body(empty_body())
                .unwrap()),
            RouteMatch::LocalService {
                req,
                service: endpoint,
            } => endpoint.handle(req).await,
        }
    }

    /// match_route is synchronous, to avoid contention on the ArcSwap Guard (if accidentally held across `await` points).
    /// i.e. this function can't do any networking stuff.
    fn match_route(
        &self,
        mut req: Request<hyper::body::Incoming>,
    ) -> Result<RouteMatch, HttpError> {
        let routes = self.state.routes.load();

        let matchit = routes.at(req.uri().path()).map_err(|_| {
            trace!("did not match any routes");
            HttpError::Static(StatusCode::NOT_FOUND, "Not found")
        })?;

        match matchit.value {
            Route::Proxy(proxy) => {
                trace!(
                    "original URI: `{}` match: `{}`",
                    req.uri(),
                    proxy.backend_uri()
                );

                let original_uri = req.uri().clone();
                let rewritten_uri = rewrite_proxied_uri(
                    req.uri().clone(),
                    Some(proxy.backend_uri()),
                    &matchit,
                    proxy.replace_prefix(),
                )?;

                (*req.uri_mut()) = rewritten_uri;
                debug!("rewritten URI: `{}`", req.uri());

                set_proxy_headers(&mut req, &original_uri)?;

                let auth_directive = proxy.get_auth_directive(&req);

                let http_client = match proxy.backend_class() {
                    BackendClass::Plain => &self.state.backends.default,
                    BackendClass::AuthlyMesh => &self.state.backends.authly,
                };

                Ok(RouteMatch::Proxy {
                    http_client_instance: http_client.current_instance(),
                    req,
                    auth_directive,
                })
            }
            Route::TemporaryRedirect(uri) => Ok(RouteMatch::TemporaryRedirect(uri.clone())),
            Route::Local(local_service) => {
                let rewritten_uri = rewrite_proxied_uri(
                    req.uri().clone(),
                    None,
                    &matchit,
                    local_service.replace_prefix(),
                )?;
                (*req.uri_mut()) = rewritten_uri;

                Ok(RouteMatch::LocalService {
                    req,
                    service: local_service.clone(),
                })
            }
        }
    }
}

/// Rewrite the original Uri for proxying.
///
/// scheme and authority are rewritten based on `target_uri`.
///
/// the Uri path is stripped to `/` by default,
/// and rewritten based on `matchit` "path" parameter, if present.
pub(crate) fn rewrite_proxied_uri(
    original: Uri,
    target_uri: Option<&Uri>,
    matchit: &matchit::Match<&Route>,
    replace_prefix: Option<&str>,
) -> Result<Uri, HttpError> {
    let mut parts = original.into_parts();

    if let Some(target_uri) = target_uri {
        parts.scheme = target_uri.scheme().cloned();
        parts.authority = target_uri.authority().cloned();
    }

    if let Some(replace_prefix) = replace_prefix {
        // "path" is magic, for now. It matches the URI path that's forwarded
        // to the proxied service
        let rewrite_path = matchit.params.get("path");
        let query = parts.path_and_query.as_ref().and_then(|pq| pq.query());

        let mut new_path_query = {
            let mut cap = 1;
            if let Some(path) = rewrite_path {
                cap += path.len();
            }
            if let Some(query) = query {
                cap += 1 + query.len();
            }
            String::with_capacity(cap)
        };

        new_path_query.push_str(replace_prefix);

        if let Some(path) = rewrite_path {
            new_path_query.push_str(path);
        }
        if let Some(query) = query {
            new_path_query.push('?');
            new_path_query.push_str(query);
        }

        parts.path_and_query =
            Some(new_path_query.parse().map_err(|_| {
                HttpError::Static(StatusCode::INTERNAL_SERVER_ERROR, "uri problem")
            })?);
    }

    Uri::from_parts(parts).map_err(|err| {
        error!(?err, "URI rewrite failed");
        HttpError::Static(StatusCode::INTERNAL_SERVER_ERROR, "invalid uri")
    })
}
