use anyhow::Context;
use http::header::{self, AUTHORIZATION};
// use memoriam_common::{
//     authly::headers::{X_AUTHLY_EID, X_AUTHLY_ENTITY_ID, X_AUTHLY_ENTITY_TYPE},
//     config::MemoriamConfig,
//     errors::MemoriamError,
//     http_client::HttpClient,
// };
use serde::Deserialize;
use tracing::warn;

use crate::{config::ArxConfig, http_client::HttpClient, ArxError};

/// Authly profile data deserialization struct
#[derive(Deserialize)]
#[serde(rename_all_fields = "camelCase")]
#[serde(tag = "entityType")]
enum AuthlyProfile {
    #[serde(rename_all = "camelCase")]
    User {
        #[serde(rename(deserialize = "entityID"))]
        entity_id: i64,
        username: String,
    },
    #[serde(rename_all = "camelCase")]
    Service {
        #[serde(rename(deserialize = "entityID"))]
        entity_id: i64,
        service_name: String,
    },
}

/// Authentication middleware; verifies session with Authly
pub async fn authenticate(
    target_headers: &mut http::HeaderMap,
    arx_config: &ArxConfig,
    http_client: &HttpClient,
    authly_client: Option<&authly_client::Client>,
) -> Result<(), ArxError> {
    let cookie_jar = cookie_jar(target_headers);

    if let Some(authly_client) = authly_client {
        let Some(session_cookie) = cookie_jar.get("session-cookie") else {
            return Err(ArxError::NotAuthenticated);
        };

        let access_token = authly_client
            .get_access_token(session_cookie.value_trimmed())
            .await
            .map_err(|err| {
                warn!(?err, "authly access token error");
                ArxError::NotAuthenticated
            })?;

        target_headers.insert(
            AUTHORIZATION,
            format!("Bearer: {}", access_token.token)
                .try_into()
                .unwrap(),
        );

        Ok(())
    } else {
        Err(ArxError::NotAuthenticated)
    }
}

fn cookie_jar(headers: &http::HeaderMap) -> cookie::CookieJar {
    let cookies = headers
        .get_all(header::COOKIE)
        .into_iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|cookie| cookie::Cookie::parse(cookie.to_owned()).ok());

    let mut jar = cookie::CookieJar::new();
    for cookie in cookies {
        jar.add_original(cookie);
    }

    jar
}
