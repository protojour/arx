use cookie::Cookie;
use http::{
    header::{self, AUTHORIZATION},
    HeaderMap,
};
use tracing::warn;

use crate::{route::AuthDirective, ArxError};

/// Process the auth directive, by interacting with Authly in various ways.
///
/// The auth directive represents a rule on when to exchange a session for an access token.
pub async fn process_auth_directive(
    auth_directive: AuthDirective,
    target_headers: &mut http::HeaderMap,
    authly_client: Option<&authly_client::Client>,
) -> Result<(), ArxError> {
    match (auth_directive, authly_client) {
        (AuthDirective::Mandatory, Some(client)) => {
            let cookie_jar = cookie_jar(target_headers);
            let Some(session_cookie) = cookie_jar.get("session-cookie") else {
                return Err(ArxError::NotAuthenticated);
            };

            inject_access_token(target_headers, session_cookie, client).await
        }
        (AuthDirective::Mandatory, None) => Err(ArxError::NotAuthenticated),
        (AuthDirective::Opportunistic, Some(client)) => {
            let cookie_jar = cookie_jar(target_headers);
            let Some(session_cookie) = cookie_jar.get("session-cookie") else {
                return Ok(());
            };

            inject_access_token(target_headers, session_cookie, client).await
        }
        (AuthDirective::Opportunistic, None) => Ok(()),
        (AuthDirective::Disabled, _) => Ok(()),
    }

    /*
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
    */
}

async fn inject_access_token(
    target_headers: &mut HeaderMap,
    session_cookie: &Cookie<'static>,
    authly_client: &authly_client::Client,
) -> Result<(), ArxError> {
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
