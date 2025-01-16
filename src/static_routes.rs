use std::sync::Arc;

use crate::{local, route::Route};

/// Static/local routes that are always present
pub fn static_routes(client: reqwest::Client) -> anyhow::Result<matchit::Router<Route>> {
    let mut routes = matchit::Router::new();
    routes.insert("/health", Route::Local(Arc::new(local::Health { client })))?;
    routes.insert(
        "/favicon.ico",
        // deliberate redirect to .png
        Route::TemporaryRedirect("/static/favicon.png".parse()?),
    )?;
    routes.insert(
        "/favicon.svg",
        // deliberate redirect to .png
        Route::TemporaryRedirect("/static/favicon.png".parse()?),
    )?;
    routes.insert(
        "/favicon.png",
        Route::TemporaryRedirect("/static/favicon.png".parse()?),
    )?;

    {
        let onto = Route::Local(Arc::new(local::Onto));
        routes.insert("/", Route::TemporaryRedirect("/onto/".parse()?))?;
        routes.insert("/onto", Route::TemporaryRedirect("/onto/".parse()?))?;
        routes.insert("/onto/", onto.clone())?;
        routes.insert("/onto/{*path}", onto)?;
    }

    {
        let docs = Route::Local(Arc::new(local::Docs));
        routes.insert("/docs", Route::TemporaryRedirect("/docs/".parse()?))?;
        routes.insert("/docs/", docs.clone())?;
        routes.insert("/docs/{*path}", docs)?;
    }

    routes.insert("/static/{*path}", Route::Local(Arc::new(local::Static)))?;

    Ok(routes)
}

#[cfg(test)]
mod tests {
    use http::Uri;

    use crate::{gateway::rewrite_proxied_uri, route::Proxy};

    use super::{static_routes, Route};

    #[tokio::test]
    async fn routes_smoke_test() {
        let mut routes = static_routes(reqwest::Client::new()).unwrap();

        routes
            .insert(
                "/stripped/{*path}",
                Proxy::from_service_url(&"http://stripped/".parse().unwrap())
                    .unwrap()
                    .with_replace_prefix("/")
                    .into(),
            )
            .unwrap();

        routes
            .insert(
                "/unstripped/{*path}",
                Proxy::from_service_url(&"http://unstripped/".parse().unwrap())
                    .unwrap()
                    .into(),
            )
            .unwrap();

        // docs subpath
        {
            let matchit = routes.at("/docs/yo").unwrap();
            let Route::Local(_) = &matchit.value else {
                panic!("{:?}", matchit.value);
            };
        }

        // docs root
        {
            let docs_uri: Uri = "/docs/".parse().unwrap();
            let matchit = routes.at(docs_uri.path()).unwrap();
            let Route::Local(_) = &matchit.value else {
                panic!("{:?}", matchit.value);
            };

            let rewritten =
                rewrite_proxied_uri(docs_uri.clone(), None, &matchit, Some("/")).unwrap();
            assert_eq!("/", rewritten.path(), "prefix should be stripped");
        }

        // strip prefix
        {
            let authly_uri: Uri = "/stripped/some/path".parse().unwrap();

            let matchit = routes.at(authly_uri.path()).unwrap();
            let Route::Proxy(_) = &matchit.value else {
                panic!("{:?}", matchit.value);
            };

            let rewritten =
                rewrite_proxied_uri(authly_uri.clone(), None, &matchit, Some("/")).unwrap();
            assert_eq!("/some/path", rewritten.path(), "prefix should be stripped");
        }

        // keep prefix
        {
            let storage_uri: Uri = "/unstripped/some/path".parse().unwrap();

            let matchit = routes.at(storage_uri.path()).unwrap();
            let Route::Proxy(_) = &matchit.value else {
                panic!("{:?}", matchit.value);
            };

            let rewritten = rewrite_proxied_uri(storage_uri.clone(), None, &matchit, None).unwrap();
            assert_eq!(
                "/unstripped/some/path",
                rewritten.path(),
                "prefix should not be stripped"
            );
        }
    }
}
