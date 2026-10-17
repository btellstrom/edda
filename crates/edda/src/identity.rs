//! This device's identity: its key and the campaigns it belongs to, kept in
//! the config directory (`$EDDA_CONFIG_DIR`, or `$XDG_CONFIG_HOME/edda`, or
//! `~/.config/edda`; one from before the rename, `campaign-manager` next to
//! it, is moved there):
//!
//! ```text
//! device.key                  the key, the same in every campaign
//! campaigns/<campaign id>.json  one profile per campaign joined
//! current                     the id of the campaign in use
//! ```

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use edda_auth::{
    DeviceKey, PublicKey,
    protocol::{Campaign, Session},
};
use edda_client::auth::AuthClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `edda` in the config directory `config`, moved there from where Edda
/// kept it when it was called the campaign manager, so a device keeps its
/// key and campaigns.
fn default_dir(config: &Path) -> Result<PathBuf> {
    let dir = config.join("edda");
    let old = config.join("campaign-manager");
    if !dir.exists() && old.is_dir() {
        fs::rename(&old, &dir)
            .with_context(|| format!("moving {} to {}", old.display(), dir.display()))?;
    }
    Ok(dir)
}

/// A campaign this device is in, and where to log in to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        let dir = match env::var_os("EDDA_CONFIG_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => {
                let config = env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
                    .ok_or_else(|| anyhow!("can't find a config directory; set EDDA_CONFIG_DIR"))?;
                default_dir(&config)?
            }
        };
        Ok(Self::at(dir))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("device.key")
    }

    fn campaigns_dir(&self) -> PathBuf {
        self.dir.join("campaigns")
    }

    fn profile_path(&self, campaign: Uuid) -> PathBuf {
        self.campaigns_dir().join(format!("{campaign}.json"))
    }

    fn current_path(&self) -> PathBuf {
        self.dir.join("current")
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

    /// Moves the single `profile.json` of devices set up before several
    /// campaigns were possible into `campaigns/`, keeping it in use.
    fn migrate(&self) -> Result<()> {
        let legacy = self.dir.join("profile.json");
        let text = match fs::read_to_string(&legacy) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).context("reading profile.json"),
        };
        let profile: Profile = serde_json::from_str(&text).context("reading profile.json")?;
        self.write(&profile)?;
        if self.current_id()?.is_none() {
            self.set_current(profile.campaign.id)?;
        }
        fs::remove_file(&legacy).context("removing the old profile.json")
    }

    /// Every campaign this device is in, by name.
    pub fn profiles(&self) -> Result<Vec<Profile>> {
        self.migrate()?;
        let entries = match fs::read_dir(self.campaigns_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).context("reading the campaigns directory"),
        };
        let mut profiles = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                profiles.push(
                    serde_json::from_str(&text)
                        .with_context(|| format!("reading {}", path.display()))?,
                );
            }
        }
        profiles.sort_by_key(|p: &Profile| p.campaign.name.to_lowercase());
        Ok(profiles)
    }

    fn current_id(&self) -> Result<Option<Uuid>> {
        match fs::read_to_string(self.current_path()) {
            Ok(text) => Ok(text.trim().parse().ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("reading the current campaign"),
        }
    }

    fn set_current(&self, campaign: Uuid) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(self.current_path(), campaign.to_string()).context("saving the current campaign")
    }

    fn write(&self, profile: &Profile) -> Result<()> {
        fs::create_dir_all(self.campaigns_dir())?;
        let path = self.profile_path(profile.campaign.id);
        fs::write(&path, serde_json::to_string_pretty(profile)?)
            .with_context(|| format!("saving {}", path.display()))
    }

    /// The campaign in use, if any.
    pub fn profile(&self) -> Result<Option<Profile>> {
        self.migrate()?;
        let Some(id) = self.current_id()? else {
            return Ok(None);
        };
        Ok(self.profiles()?.into_iter().find(|p| p.campaign.id == id))
    }

    /// Adds (or updates) a campaign and switches to it.
    pub fn save_profile(&self, profile: &Profile) -> Result<()> {
        self.migrate()?;
        self.write(profile)?;
        self.set_current(profile.campaign.id)
    }

    /// The campaign `query` names: its name (any case), a unique prefix of
    /// the name, or a prefix of its id.
    pub fn find(&self, query: &str) -> Result<Profile> {
        let profiles = self.profiles()?;
        let query_lower = query.to_lowercase();
        if let Some(exact) = profiles
            .iter()
            .find(|p| p.campaign.name.to_lowercase() == query_lower)
        {
            return Ok(exact.clone());
        }
        let matching: Vec<_> = profiles
            .iter()
            .filter(|p| {
                p.campaign.name.to_lowercase().starts_with(&query_lower)
                    || p.campaign.id.to_string().starts_with(&query_lower)
            })
            .collect();
        match matching.as_slice() {
            [one] => Ok((*one).clone()),
            [] if profiles.is_empty() => bail!("this device isn't in any campaign yet"),
            [] => bail!(
                "this device isn't in a campaign called {query:?}; it's in {}",
                names(&profiles)
            ),
            many => {
                let many: Vec<Profile> = many.iter().map(|p| (*p).clone()).collect();
                bail!("{query:?} could be {}", names(&many))
            }
        }
    }

    /// Switches to the campaign `query` names.
    pub fn select(&self, query: &str) -> Result<Profile> {
        let profile = self.find(query)?;
        self.set_current(profile.campaign.id)?;
        Ok(profile)
    }

    /// Forgets a campaign on this device; if it was in use, none is until
    /// another is chosen. Returns it and whether it was in use.
    pub fn forget(&self, query: &str) -> Result<(Profile, bool)> {
        let profile = self.find(query)?;
        let path = self.profile_path(profile.campaign.id);
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        let was_current = self.current_id()? == Some(profile.campaign.id);
        if was_current {
            fs::remove_file(self.current_path()).context("clearing the current campaign")?;
        }
        Ok((profile, was_current))
    }

    /// Logs in to the campaign in use.
    pub async fn login(&self) -> Result<(Profile, Session)> {
        let Some(profile) = self.profile()? else {
            if !self.profiles()?.is_empty() {
                bail!(
                    "no campaign chosen; pick one with `edda auth use <CAMPAIGN>` \
                     (`edda auth campaigns` lists them)"
                );
            }
            bail!(
                "this device isn't in a campaign yet.\n\
                 DM:      edda auth setup <CODE> --name <DEVICE NAME>\n\
                 Player:  edda auth pair <CODE> --name <DEVICE NAME>, with a code\n\
                 \x20        from your DM's `edda auth invite`"
            );
        };
        let session = self.login_to(&profile).await?;
        Ok((profile, session))
    }

    /// Logs in to a given campaign, whichever is in use.
    pub async fn login_to(&self, profile: &Profile) -> Result<Session> {
        AuthClient::new(&profile.auth_server)
            .login(profile.campaign.id, &self.key()?, Some(profile.server_key))
            .await
            .context("logging in")
    }
}

/// "Vox Machina, Oneshot".
fn names(profiles: &[Profile]) -> String {
    profiles
        .iter()
        .map(|p| p.campaign.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An identity in a fresh directory of its own.
    fn identity() -> Identity {
        let dir = env::temp_dir().join(format!("edda-test-{}", Uuid::new_v4()));
        Identity::at(dir)
    }

    fn profile(name: &str) -> Profile {
        Profile {
            auth_server: "http://auth".into(),
            server_key: DeviceKey::generate().public(),
            campaign: Campaign {
                id: Uuid::new_v4(),
                name: name.into(),
                edition: Default::default(),
            },
        }
    }

    #[test]
    fn the_config_from_before_the_rename_moves_to_edda() {
        let config = env::temp_dir().join(format!("edda-test-{}", Uuid::new_v4()));
        let old = Identity::at(config.join("campaign-manager"));
        let vox = profile("Vox Machina");
        old.save_profile(&vox).unwrap();
        let key = old.key().unwrap().public();

        let dir = default_dir(&config).unwrap();
        assert_eq!(dir, config.join("edda"));
        assert!(!config.join("campaign-manager").exists());
        let moved = Identity::at(dir.clone());
        assert_eq!(moved.key().unwrap().public(), key);
        assert_eq!(moved.profile().unwrap(), Some(vox));

        // Only once: a new one is left alone.
        fs::create_dir(config.join("campaign-manager")).unwrap();
        assert_eq!(default_dir(&config).unwrap(), dir);
        assert!(config.join("campaign-manager").exists());
    }

    #[test]
    fn joining_adds_a_campaign_and_switches_to_it() {
        let identity = identity();
        assert_eq!(identity.profile().unwrap(), None);
        let vox = profile("Vox Machina");
        let oneshot = profile("Oneshot");
        identity.save_profile(&vox).unwrap();
        identity.save_profile(&oneshot).unwrap();
        assert_eq!(identity.profile().unwrap(), Some(oneshot.clone()));
        assert_eq!(identity.profiles().unwrap(), [oneshot.clone(), vox.clone()]);

        assert_eq!(identity.select("vox").unwrap(), vox, "by name prefix");
        assert_eq!(identity.profile().unwrap(), Some(vox.clone()));
        let id_prefix = &oneshot.campaign.id.to_string()[..8];
        assert_eq!(identity.select(id_prefix).unwrap(), oneshot);
        assert!(identity.select("Mighty Nein").is_err());
        fs::remove_dir_all(&identity.dir).unwrap();
    }

    #[test]
    fn names_must_pick_one_campaign() {
        let identity = identity();
        identity.save_profile(&profile("Vox Machina")).unwrap();
        identity
            .save_profile(&profile("Vox Machina Origins"))
            .unwrap();
        let error = identity.select("vox").unwrap_err().to_string();
        assert!(error.contains("could be"), "{error}");
        // A full name wins over being a prefix of another.
        let exact = identity.select("vox machina").unwrap();
        assert_eq!(exact.campaign.name, "Vox Machina");
        fs::remove_dir_all(&identity.dir).unwrap();
    }

    #[test]
    fn forgetting_the_campaign_in_use_leaves_none_chosen() {
        let identity = identity();
        let vox = profile("Vox Machina");
        identity.save_profile(&vox).unwrap();
        identity.save_profile(&profile("Oneshot")).unwrap();
        let (forgotten, was_current) = identity.forget("oneshot").unwrap();
        assert_eq!(
            (forgotten.campaign.name.as_str(), was_current),
            ("Oneshot", true)
        );
        assert_eq!(identity.profile().unwrap(), None);
        assert_eq!(identity.profiles().unwrap(), std::slice::from_ref(&vox));
        let (_, was_current) = identity.forget("vox").unwrap();
        assert!(!was_current);
        assert!(identity.profiles().unwrap().is_empty());
        fs::remove_dir_all(&identity.dir).unwrap();
    }

    #[test]
    fn a_profile_from_before_several_campaigns_carries_over() {
        let identity = identity();
        let vox = profile("Vox Machina");
        fs::create_dir_all(&identity.dir).unwrap();
        fs::write(
            identity.dir.join("profile.json"),
            serde_json::to_string(&vox).unwrap(),
        )
        .unwrap();
        assert_eq!(identity.profile().unwrap(), Some(vox.clone()));
        assert!(!identity.dir.join("profile.json").exists());
        identity.save_profile(&profile("Oneshot")).unwrap();
        assert_eq!(identity.profiles().unwrap().len(), 2);
        fs::remove_dir_all(&identity.dir).unwrap();
    }
}
