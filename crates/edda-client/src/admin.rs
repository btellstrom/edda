//! Client for the services' admin API (see `edda_server::admin`), for
//! whoever runs them, such as a hosting's control plane: creating
//! campaigns and their DMs' setup codes, exporting a whole campaign to keep
//! or move elsewhere and importing it there, and deleting campaigns.

use edda_auth::protocol::{AuthExport, CreatedCampaign, NewCampaign, SetupCode};
use edda_types::{
    Edition,
    admin::{CharactersExport, CombatExport, Deleted, Imported, MonstersExport},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
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

    /// Any service's: its part of a campaign, `T` being that service's
    /// export (`AuthExport`, `CharactersExport` and so on).
    pub async fn export<T: DeserializeOwned>(&self, campaign: Uuid) -> Result<T, ClientError> {
        let url = self.url(&format!("/admin/campaigns/{campaign}/export"));
        send(self.http.get(url)).await
    }

    /// Any service's: takes in its part of a campaign exported elsewhere,
    /// and says how many of its things came. [`ClientError::Conflict`] if
    /// the campaign has some there already.
    pub async fn import<T: Serialize>(&self, campaign: Uuid, part: &T) -> Result<u64, ClientError> {
        let url = self.url(&format!("/admin/campaigns/{campaign}/import"));
        let imported: Imported =
            send(self.http.put(url).json(part))
                .await
                .map_err(|e| match e {
                    // A 409 from the admin API is a conflict, not slots run out.
                    ClientError::Exhausted(message) => ClientError::Conflict(message),
                    other => other,
                })?;
        Ok(imported.imported)
    }
}

/// A whole campaign: every service's part of it, as exported to keep or to
/// move to another server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampaignBundle {
    /// Which version of this format: [`CampaignBundle::FORMAT`].
    pub format: u32,
    pub auth: AuthExport,
    pub characters: CharactersExport,
    pub combat: CombatExport,
    pub monsters: MonstersExport,
}

impl CampaignBundle {
    /// The format this version writes and reads.
    pub const FORMAT: u32 = 1;

    /// Whose it is.
    pub fn campaign(&self) -> Uuid {
        self.auth.campaign.id
    }
}

/// The four services' admin APIs, to act on a whole campaign at once.
#[derive(Debug, Clone)]
pub struct Services {
    pub auth: AdminClient,
    pub characters: AdminClient,
    pub combat: AdminClient,
    pub monsters: AdminClient,
}

impl Services {
    /// Every service's part of `campaign`. Make it read-only first, or
    /// changes made meanwhile may be in one part and not another.
    pub async fn export_campaign(&self, campaign: Uuid) -> Result<CampaignBundle, ClientError> {
        Ok(CampaignBundle {
            format: CampaignBundle::FORMAT,
            auth: self.auth.export(campaign).await?,
            characters: self.characters.export(campaign).await?,
            combat: self.combat.export(campaign).await?,
            monsters: self.monsters.export(campaign).await?,
        })
    }

    /// Takes in a whole campaign, the auth service's part first. Should a
    /// part fail, those already in are deleted again, so it's all or
    /// nothing; [`ClientError::Conflict`] if the campaign is there already.
    pub async fn import_campaign(&self, bundle: &CampaignBundle) -> Result<(), ClientError> {
        if bundle.format != CampaignBundle::FORMAT {
            return Err(ClientError::Rejected(format!(
                "a campaign of format {}; this version reads {}",
                bundle.format,
                CampaignBundle::FORMAT
            )));
        }
        let campaign = bundle.campaign();
        self.auth.import(campaign, &bundle.auth).await?;
        let rest = async {
            self.characters.import(campaign, &bundle.characters).await?;
            self.combat.import(campaign, &bundle.combat).await?;
            self.monsters.import(campaign, &bundle.monsters).await
        };
        if let Err(e) = rest.await {
            // Best effort: what's left over is deleted with the campaign.
            let _ = self.delete_campaign(campaign).await;
            return Err(e);
        }
        Ok(())
    }

    /// Deletes a campaign from every service, the auth service last, so
    /// that one cut short leaves it known, to delete again. Safe to repeat.
    pub async fn delete_campaign(&self, campaign: Uuid) -> Result<(), ClientError> {
        self.characters.delete_campaign(campaign).await?;
        self.combat.delete_campaign(campaign).await?;
        self.monsters.delete_campaign(campaign).await?;
        self.auth.delete_campaign(campaign).await?;
        Ok(())
    }
}
