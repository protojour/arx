use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use arc_swap::ArcSwap;
use gateway_api::apis::standard::httproutes::{HTTPRoute, HTTPRouteRulesMatchesPathType};
use kube::{runtime::reflector::Lookup, Api};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, info_span, warn};
use url::Url;

use crate::{
    route::{Proxy, Route},
    static_routes::static_routes,
    // static_routes::static_routes,
};

use super::k8s_util::{api_watcher, ApiWatcherCallbacks};

pub async fn spawn_k8s_watchers(
    gateway_routes: Arc<ArcSwap<matchit::Router<Route>>>,
    client: reqwest::Client,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let kube_client = kube::Client::try_default().await?;

    tokio::spawn(api_watcher(
        Api::<HTTPRoute>::all(kube_client.clone()),
        HttpRouteWatcher {
            gateway_routes,
            k8s_routes: Mutex::new(Default::default()),
            client,
        },
        cancel,
    ));

    Ok(())
}

struct HttpRouteWatcher {
    gateway_routes: Arc<ArcSwap<matchit::Router<Route>>>,
    k8s_routes: Mutex<HashMap<String, HTTPRoute>>,
    client: reqwest::Client,
}

impl ApiWatcherCallbacks<HTTPRoute> for HttpRouteWatcher {
    async fn apply(&self, objs: Vec<HTTPRoute>) -> anyhow::Result<()> {
        let mut k8s_lock = self.k8s_routes.lock().unwrap();

        for obj in objs {
            let Some((name, route)) = filter_k8s_http_route(obj) else {
                continue;
            };
            k8s_lock.insert(name, route);
        }

        update_routing_table(&k8s_lock, self.gateway_routes.clone(), self.client.clone());

        Ok(())
    }

    async fn delete(&self, objs: Vec<HTTPRoute>) -> anyhow::Result<()> {
        let mut k8s_lock = self.k8s_routes.lock().unwrap();

        for obj in objs {
            let Some((name, _route)) = filter_k8s_http_route(obj) else {
                continue;
            };
            k8s_lock.remove(&name);
        }

        update_routing_table(&k8s_lock, self.gateway_routes.clone(), self.client.clone());

        Ok(())
    }
}

fn filter_k8s_http_route(http_route: HTTPRoute) -> Option<(String, HTTPRoute)> {
    let name = http_route.name()?;
    let parent_refs = http_route.spec.parent_refs.as_ref()?;

    if !parent_refs
        .iter()
        .any(|parent_ref| parent_ref.name == "arx")
    {
        return None;
    }

    Some((name.to_string(), http_route))
}

fn update_routing_table(
    k8s_routes: &HashMap<String, HTTPRoute>,
    gateway_routes: Arc<ArcSwap<matchit::Router<Route>>>,
    client: reqwest::Client,
) {
    match rebuild_routing_table(k8s_routes, client) {
        Ok(new_routes) => {
            gateway_routes.store(Arc::new(new_routes));
        }
        Err(err) => {
            error!(?err, "could not build new routing table");
        }
    }
}

pub fn rebuild_routing_table(
    k8s_routes: &HashMap<String, HTTPRoute>,
    client: reqwest::Client,
) -> anyhow::Result<matchit::Router<Route>> {
    let mut output = static_routes(client)?;

    for (name, http_route) in k8s_routes {
        let _entered = info_span!("route", name = name).entered();

        if let Err(err) = try_add_http_route(&mut output, name, http_route) {
            warn!(?err, "invalid HTTPRoute, ignoring");
        }
    }

    Ok(output)
}

pub fn try_add_http_route(
    output: &mut matchit::Router<Route>,
    name: &str,
    http_route: &HTTPRoute,
) -> anyhow::Result<()> {
    let spec = &http_route.spec;

    if let Some(_hostnames) = &spec.hostnames {
        // TODO: hostnames
    }

    if let Some(rules) = &spec.rules {
        for rule in rules {
            let Some(backend_refs) = &rule.backend_refs else {
                continue;
            };

            let backend_ref = match backend_refs.len() {
                0 => continue,
                1 => backend_refs.iter().next().unwrap(),
                _ => {
                    warn!("no support for multiple backend refs yet, using just the first one");
                    backend_refs.iter().next().unwrap()
                }
            };

            let Some(backend_port) = backend_ref.port else {
                continue;
            };
            let backend_url = match backend_port {
                443 => format!("https://{name}", name = backend_ref.name),
                _ => format!(
                    "http://{name}:{port}",
                    name = backend_ref.name,
                    port = backend_port
                ),
            };
            let backend_url = Url::parse(&backend_url)?;

            let Some(matches) = &rule.matches else {
                continue;
            };

            for route_match in matches {
                if let Some(_method) = &route_match.method {
                    warn!(name, "no support for method match");
                }
                if let Some(_q) = &route_match.query_params {
                    warn!(name, "no support for query_params match");
                }

                let mut url_rewrite = None;
                let mut must_authenticate = false;

                if let Some(filters) = &rule.filters {
                    for filter in filters {
                        if let Some(rw) = &filter.url_rewrite {
                            url_rewrite = Some(rw);
                        }

                        if let Some(ext) = &filter.extension_ref {
                            if ext.group == "authly.id" && ext.name == "authn" {
                                must_authenticate = true;
                            }
                        }
                    }
                }

                if let Some(path) = &route_match.path {
                    let Some(value) = &path.value else {
                        continue;
                    };

                    let mut proxy = Proxy::from_service_url(&backend_url)?;
                    proxy = if must_authenticate {
                        proxy.with_must_authenticate_predicate(|_| true)
                    } else {
                        proxy.with_must_authenticate_predicate(|_| false)
                    };

                    match path.r#type {
                        None | Some(HTTPRouteRulesMatchesPathType::PathPrefix) => {
                            let prefix = if !value.ends_with('/') {
                                // append a slash
                                let terminated = format!("{value}/");
                                try_insert_route(
                                    output,
                                    value,
                                    Route::TemporaryRedirect(terminated.parse()?),
                                );
                                terminated
                            } else {
                                // insert a redirect for missing slash
                                let mut unterminated = value.as_str();
                                while unterminated.ends_with('/') {
                                    let mut chars = unterminated.chars();
                                    chars.next_back();
                                    unterminated = chars.as_str();
                                }
                                try_insert_route(
                                    output,
                                    unterminated,
                                    Route::TemporaryRedirect(value.parse()?),
                                );

                                value.to_string()
                            };

                            if let Some(url_rewrite) = url_rewrite {
                                if let Some(path) = &url_rewrite.path {
                                    if let Some(prefix_path) = &path.replace_prefix_match {
                                        if prefix_path.ends_with('/') {
                                            proxy = proxy.with_replace_prefix(prefix_path);
                                        } else {
                                            proxy = proxy
                                                .with_replace_prefix(format!("{prefix_path}/"));
                                        }
                                    }
                                }
                            }

                            try_insert_route(output, &prefix, Route::Proxy(proxy.clone()));
                            try_insert_route(
                                output,
                                &format!("{prefix}{{*path}}"),
                                Route::Proxy(proxy),
                            );
                        }
                        Some(HTTPRouteRulesMatchesPathType::Exact) => {
                            try_insert_route(output, value, Route::Proxy(proxy));
                        }
                        Some(HTTPRouteRulesMatchesPathType::RegularExpression) => {
                            warn!(name, "regular expression path match not supported");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn try_insert_route(output: &mut matchit::Router<Route>, path: &str, route: Route) {
    if let Err(_e) = output.insert(path, route) {
        info!(path, "not inserting route because already occupied");
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    fn build_test_routing(yamls: Vec<&'static str>) -> matchit::Router<Route> {
        let routes: Vec<HTTPRoute> = yamls
            .iter()
            .map(|yaml| serde_yaml::from_str(yaml).unwrap())
            .collect();

        let routes = routes
            .into_iter()
            .filter_map(filter_k8s_http_route)
            .collect();

        rebuild_routing_table(&routes, reqwest::Client::new()).unwrap()
    }

    #[test]
    fn simple_route() {
        let matchit_router = build_test_routing(vec![indoc! {
            "
            metadata:
              name: test
            spec:
              parentRefs:
                - name: arx
              rules:
                - matches:
                  - path:
                      value: /authly
                  filters:
                    - type: URLRewrite
                      urlRewrite:
                        path:
                          type: ReplacePrefixMatch
                          replacePrefixMatch: /
                  backendRefs:
                    - name: authly
                      port: 443
            "
        }]);

        let Ok(matchit::Match {
            value: Route::Proxy(proxy),
            ..
        }) = matchit_router.at("/authly/")
        else {
            panic!()
        };

        assert_eq!(Some("/"), proxy.replace_prefix());

        let Ok(matchit::Match {
            value: Route::Proxy(proxy),
            ..
        }) = matchit_router.at("/authly/api/")
        else {
            panic!()
        };

        assert_eq!(Some("/"), proxy.replace_prefix());
    }

    #[test]
    fn authly_auth_whitelist() {
        let matchit_router = build_test_routing(vec![indoc! {
            "
            metadata:
              name: test
            spec:
              parentRefs:
                - name: arx
              rules:
                - matches:
                    - path:
                        value: /authly/api/auth
                  filters:
                    - type: URLRewrite
                      urlRewrite:
                        path:
                          type: ReplacePrefixMatch
                          replacePrefixMatch: /api/auth
                  backendRefs:
                    - name: authly
                      port: 443
                - matches:
                    - path:
                        value: /authly
                  filters:
                    - type: ExtensionRef
                      extensionRef:
                        group: authly.id
                        kind: Service
                        name: authn
                    - type: URLRewrite
                      urlRewrite:
                        path:
                          type: ReplacePrefixMatch
                          replacePrefixMatch: /
                  backendRefs:
                    - name: authly
                      port: 443
            "
        }]);

        let Ok(matchit::Match {
            value: Route::Proxy(proxy),
            ..
        }) = matchit_router.at("/authly/api/auth/")
        else {
            panic!()
        };

        assert_eq!(Some("/api/auth/"), proxy.replace_prefix());
    }
}
