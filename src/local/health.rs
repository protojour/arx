//! The health module is work in progress.

use http::StatusCode;
use serde::Serialize;
use url::Url;

/// Health info for each service
#[derive(Serialize)]
pub struct HealthInfo {
    name: String,
    #[serde(skip_serializing)]
    #[allow(unused)]
    url: Option<Url>,
    status_code: u16,
    status: String,
}

impl HealthInfo {
    /*
    fn from_service(service: Service) -> Self {
        Self {
            name: service.name.clone(),
            url: service.health_url.clone(),
            status_code: 200,
            status: "Ok".into(),
        }
    }
    */

    #[expect(unused)]
    async fn health_query(&mut self, http_client: &reqwest::Client) {
        let Some(ref url) = self.url else { return };
        let result = http_client.get(url.clone()).send().await;
        match result {
            Ok(resp) => {
                self.status_code = resp.status().into();
                if resp.status().is_client_error() || resp.status().is_server_error() {
                    self.status = resp.text().await.unwrap_or_default();
                };
            }
            Err(err) => {
                self.status_code = err.status().unwrap_or(StatusCode::BAD_GATEWAY).into();
                self.status = err.to_string();
            }
        }
    }
}

/// Gateway health info handler; checks health of all subsystems
pub async fn health(_client: &reqwest::Client) -> Vec<HealthInfo> {
    vec![]
}
