use std::{fmt::Debug, sync::Arc};

use http::Uri;
use hyper::body::Incoming;

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
            Route::Proxy(proxy) => write!(f, "Proxy to `{}`", proxy.backend_uri),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AuthDirective {
    /// The must be a valid session, and access token must be forwarded.
    Mandatory,
    /// Access token is optionally forwarded if the a session is present.
    Opportunistic,
    /// No access token is forwarded, and no session is needed.
    Disabled,
}

#[derive(Clone, Copy, Debug)]
pub enum BackendClass {
    Plain,
    AuthlyMesh,
}

/// A network service the gateway might proxy to
#[derive(Clone)]
pub struct Proxy {
    backend_uri: Uri,
    backend_class: BackendClass,
    replace_prefix: Option<String>,
    auth_directive_fn: fn(&http::Request<Incoming>) -> AuthDirective,
}

impl Proxy {
    /// Make a proxy to some service based on the base Uri that reaches that service over HTTP.
    ///
    /// By default, the proxy service is `must_authenticate`.
    pub fn from_backend_uri(uri: Uri) -> anyhow::Result<Self> {
        Ok(Self {
            backend_uri: uri,
            backend_class: BackendClass::Plain,
            replace_prefix: None,
            auth_directive_fn: |_| AuthDirective::Disabled,
        })
    }

    pub fn with_backend_class(mut self, class: BackendClass) -> Self {
        self.backend_class = class;
        self
    }

    /// set a predicate determining whether requests must be authenticated first
    /// (default is true!)
    /// note: The request has its URL rewritten before this predicate is called
    pub fn with_auth_directive_fn(self, f: fn(&http::Request<Incoming>) -> AuthDirective) -> Self {
        Self {
            auth_directive_fn: f,
            ..self
        }
    }

    pub fn with_replace_prefix(self, replacement: impl Into<String>) -> Self {
        Self {
            replace_prefix: Some(replacement.into()),
            ..self
        }
    }

    pub fn backend_uri(&self) -> &Uri {
        &self.backend_uri
    }

    pub fn backend_class(&self) -> BackendClass {
        self.backend_class
    }

    pub fn replace_prefix(&self) -> Option<&str> {
        self.replace_prefix.as_deref()
    }

    pub fn get_auth_directive(&self, req: &http::Request<Incoming>) -> AuthDirective {
        (self.auth_directive_fn)(req)
    }
}

impl From<Proxy> for Route {
    fn from(value: Proxy) -> Self {
        Route::Proxy(value)
    }
}
