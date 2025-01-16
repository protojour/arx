use std::sync::Arc;

use arc_swap::ArcSwap;
use http::{header, HeaderValue, Request, StatusCode, Uri};
use tower::ServiceBuilder;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{error, info, trace, Level};

use crate::{
    authentication::authenticate,
    config::ArxConfig,
    http_client::HttpClient,
    hyper::{empty_body, HttpError, HyperResponse},
    layers::{compression_layer, cors_layer},
    local::LocalService,
    reverse_proxy::reverse_proxy,
    route::Route,
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

enum RouteMatch<'a> {
    Proxy {
        // The HTTP client to use when proxying
        http_client: &'a reqwest::Client,
        req: Request<hyper::body::Incoming>,
        must_authenticate: bool,
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
                http_client: client,
                mut req,
                must_authenticate,
            } => {
                if must_authenticate {
                    authenticate(req.headers_mut(), self.state.authly_client.as_ref())
                        .await
                        .map_err(|_| HttpError::Static(StatusCode::UNAUTHORIZED, "unauthorized"))?;
                }

                reverse_proxy(req, client).await
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
                info!(
                    "original URI: `{}` match: `{}`",
                    req.uri(),
                    proxy.service_uri()
                );

                let original_uri = req.uri().clone();
                let rewritten_uri = rewrite_proxied_uri(
                    req.uri().clone(),
                    Some(proxy.service_uri()),
                    &matchit,
                    proxy.replace_prefix(),
                )?;
                let scheme = original_uri.scheme_str();
                let prefix = original_uri.path().strip_suffix(rewritten_uri.path());

                (*req.uri_mut()) = rewritten_uri;
                info!("rewritten URI: `{}`", req.uri());

                let headers = req.headers_mut();
                if let Some(scheme) = scheme {
                    headers.insert(
                        "x-forwarded-proto",
                        HeaderValue::from_str(scheme).map_err(|_| {
                            error!("invalid scheme: {}", scheme);
                            HttpError::Static(StatusCode::BAD_REQUEST, "")
                        })?,
                    );
                }
                if let Some(prefix) = prefix {
                    headers.insert(
                        "x-forwarded-prefix",
                        HeaderValue::from_str(prefix).map_err(|_| {
                            error!("invalid prefix: {}", prefix);
                            HttpError::Static(StatusCode::BAD_REQUEST, "")
                        })?,
                    );
                }

                let must_authenticate = proxy.must_authenticate(&req);

                // determine which http client to use (mTLS-related)
                let http_client =
                    if proxy.service_uri().host() == self.state.cfg.authly_url.host_str() {
                        self.state.backends.authly.reqwest_client()
                    } else {
                        self.state.backends.default.reqwest_client()
                    };

                Ok(RouteMatch::Proxy {
                    http_client,
                    req,
                    must_authenticate,
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
