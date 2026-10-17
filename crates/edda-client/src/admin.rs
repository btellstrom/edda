//! Client for the services' admin API (see `edda_server::admin`), for
//! whoever runs them, such as a hosting's control plane: creating
//! campaigns and their DMs' setup codes, and deleting campaigns.

use edda_auth::protocol::{CreatedCampaign, NewCampaign, SetupCode};
use edda_types::{Edition, admin::Deleted};
use uuid::Uuid;

use crate::{ClientError, send};

/// One service's admin API: at its `ADMIN_ADDR`, with its `ADMIN_TOKEN`.
#[derive(Debug, Clone)]
pub struct AdminClient {
    base_url: String,
    http: reqwest::Client,
}

impl AdminClient {
    /// The admin API at `base_url`, authorised by `token`.
    pub fn new(base_url: impl Into<String>, token: &str) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: crate::http_client(Some(token)),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// The auth service's: creates a campaign, with the one-time code its
    /// DM sets up their first device with.
    pub async fn create_campaign(
        &self,
        name: &str,
        edition: Edition,
    ) -> Result<CreatedCampaign, ClientError> {
        let new = NewCampaign {
            name: name.to_owned(),
            edition,
        };
        send(self.http.post(self.url("/admin/campaigns")).json(&new)).await
    }

    /// The auth service's: a new setup code for a DM device, as when every
    /// DM device is lost.
    pub async fn setup_code(&self, campaign: Uuid) -> Result<String, ClientError> {
        let url = self.url(&format!("/admin/campaigns/{campaign}/setup-code"));
        let code: SetupCode = send(self.http.post(url)).await?;
        Ok(code.code)
    }

    /// Any service's: deletes its part of a campaign, and says how many of
    /// its things went. Safe to repeat.
    pub async fn delete_campaign(&self, campaign: Uuid) -> Result<u64, ClientError> {
        let url = self.url(&format!("/admin/campaigns/{campaign}"));
        let deleted: Deleted = send(self.http.delete(url)).await?;
        Ok(deleted.deleted)
    }
}
