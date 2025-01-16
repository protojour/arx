use std::{fmt::Debug, sync::Arc};

use http::Uri;
use hyper::body::Incoming;
use url::Url;

use crate::local::LocalService;

/// A route that can be handled by the gateway
#[derive(Clone)]
pub enum Route {
    /// Proxy to another networked service
    Proxy(Proxy),
    /// A locally-implemented service/endpoint
    Local(Arc<dyn LocalService + Send + Sync>),
    /// Redirect to another URI
    TemporaryRedirect(Uri),
}

impl Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Route::Local(_) => write!(f, "Service"),
            Route::TemporaryRedirect(_) => write!(f, "Temporary redirect"),
            Route::Proxy(proxy) => write!(f, "Proxy to `{}`", proxy.service_uri),
        }
    }
}

/// A network service the gateway might proxy to
#[derive(Clone)]
pub struct Proxy {
    service_uri: Uri,
    replace_prefix: Option<String>,
    must_authenticate_predicate: fn(&http::Request<Incoming>) -> bool,
}

impl Proxy {
    /// Make a proxy to some service based on the base Url that reaches that service over HTTP.
    ///
    /// By default, the proxy service is `must_authenticate`.
    pub fn from_service_url(url: &Url) -> anyhow::Result<Self> {
        Ok(Self {
            service_uri: url.as_str().parse()?,
            replace_prefix: None,
            must_authenticate_predicate: |_| true,
        })
    }

    /// set a predicate determining whether requests must be authenticated first
    /// (default is true!)
    /// note: The request has its URL rewritten before this predicate is called
    pub fn with_must_authenticate_predicate(
        self,
        predicate: fn(&http::Request<Incoming>) -> bool,
    ) -> Self {
        Self {
            must_authenticate_predicate: predicate,
            ..self
        }
    }

    pub fn with_replace_prefix(self, replacement: impl Into<String>) -> Self {
        Self {
            replace_prefix: Some(replacement.into()),
            ..self
        }
    }

    pub fn service_uri(&self) -> &Uri {
        &self.service_uri
    }

    pub fn replace_prefix(&self) -> Option<&str> {
        self.replace_prefix.as_deref()
    }

    pub fn must_authenticate(&self, req: &http::Request<Incoming>) -> bool {
        (self.must_authenticate_predicate)(req)
    }
}

impl From<Proxy> for Route {
    fn from(value: Proxy) -> Self {
        Route::Proxy(value)
    }
}
