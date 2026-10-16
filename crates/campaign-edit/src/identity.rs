//! This device's identity: its key and the campaign it belongs to, kept in
//! the config directory (`$CAMPAIGN_CONFIG_DIR`, or
//! `$XDG_CONFIG_HOME/campaign-manager`, or `~/.config/campaign-manager`).

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use campaign_auth::{
    DeviceKey, PublicKey,
    protocol::{Campaign, Session},
};
use campaign_client::auth::AuthClient;
use serde::{Deserialize, Serialize};

/// Which campaign this device is in, and where to log in.
#[derive(Debug, Serialize, Deserialize)]
pub struct Profile {
    pub auth_server: String,
    /// The auth server's key as first seen; logins to any other are refused.
    pub server_key: PublicKey,
    pub campaign: Campaign,
}

pub struct Identity {
    dir: PathBuf,
}

impl Identity {
    pub fn locate() -> Result<Self> {
        let dir = match env::var_os("CAMPAIGN_CONFIG_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => {
                let config = env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
                    .ok_or_else(|| {
                        anyhow!("can't find a config directory; set CAMPAIGN_CONFIG_DIR")
                    })?;
                config.join("campaign-manager")
            }
        };
        Ok(Self { dir })
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("device.key")
    }

    fn profile_path(&self) -> PathBuf {
        self.dir.join("profile.json")
    }

    /// This device's key, created the first time it's needed.
    pub fn key(&self) -> Result<DeviceKey> {
        let path = self.key_path();
        if let Some(key) =
            DeviceKey::load(&path).with_context(|| format!("reading {}", path.display()))?
        {
            return Ok(key);
        }
        let key = DeviceKey::generate();
        key.save(&path)
            .with_context(|| format!("saving a new device key to {}", path.display()))?;
        Ok(key)
    }

    pub fn profile(&self) -> Result<Option<Profile>> {
        match fs::read_to_string(self.profile_path()) {
            Ok(text) => Ok(Some(
                serde_json::from_str(&text).context("reading profile.json")?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("reading profile.json"),
        }
    }

    pub fn save_profile(&self, profile: &Profile) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(self.profile_path(), serde_json::to_string_pretty(profile)?)
            .context("saving profile.json")
    }

    /// Logs in to this device's campaign.
    pub async fn login(&self) -> Result<(Profile, Session)> {
        let profile = self.profile()?.ok_or_else(|| {
            anyhow!(
                "this device isn't in a campaign yet.\n\
                 DM:      campaign-edit auth setup <CODE> --name <DEVICE NAME>\n\
                 Player:  give your DM the key from `campaign-edit auth key`, then\n\
                 \x20        campaign-edit auth join <CAMPAIGN ID>"
            )
        })?;
        let session = AuthClient::new(&profile.auth_server)
            .login(profile.campaign.id, &self.key()?, Some(profile.server_key))
            .await
            .context("logging in")?;
        Ok((profile, session))
    }
}
