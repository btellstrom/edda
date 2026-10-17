//! What the fight screen and the encounter planner share: a login that
//! stays valid while they're open, and finding the campaign's plans and
//! monsters by name.

use anyhow::Result;
use chrono::Utc;
use edda_auth::protocol::Session;
use edda_client::{Client, combat::CombatClient, monsters::MonsterClient};
use edda_types::{encounter::Encounter, monster::MonsterSummary};

use crate::identity::{Identity, Profile};

/// Logged in, with the token renewed shortly before it expires (tokens last
/// a quarter of an hour; a fight lasts longer).
pub(crate) struct Login {
    identity: Identity,
    /// The campaign the screen was opened in, even if another is chosen
    /// meanwhile (say, in another terminal).
    pub(crate) profile: Profile,
    pub(crate) session: Session,
    characters_url: String,
    combat_url: String,
    monsters_url: String,
}

impl Login {
    pub(crate) async fn new(
        characters_url: &str,
        combat_url: &str,
        monsters_url: &str,
    ) -> Result<Self> {
        let identity = Identity::locate()?;
        let (profile, session) = identity.login().await?;
        Ok(Self {
            identity,
            profile,
            session,
            characters_url: characters_url.to_string(),
            combat_url: combat_url.to_string(),
            monsters_url: monsters_url.to_string(),
        })
    }

    async fn token(&mut self) -> Result<String> {
        if self.session.claims.expires_at < Utc::now() + chrono::Duration::minutes(2) {
            self.session = self.identity.login_to(&self.profile).await?;
        }
        Ok(self.session.token.clone())
    }

    pub(crate) async fn characters(&mut self) -> Result<Client> {
        Ok(Client::new(&self.characters_url).with_token(&self.token().await?))
    }

    pub(crate) async fn combats(&mut self) -> Result<CombatClient> {
        Ok(CombatClient::new(&self.combat_url).with_token(&self.token().await?))
    }

    pub(crate) async fn monsters(&mut self) -> Result<MonsterClient> {
        Ok(MonsterClient::new(&self.monsters_url).with_token(&self.token().await?))
    }
}

/// A plan by name: exactly, or (unless `exact`) by a unique prefix of
/// its name or id.
pub fn find_plan<'e>(
    encounters: &'e [Encounter],
    name: &str,
    exact: bool,
) -> Option<&'e Encounter> {
    let name = name.to_lowercase();
    if name.is_empty() {
        return None;
    }
    if let Some(e) = encounters
        .iter()
        .find(|e| e.plan.name.to_lowercase() == name)
    {
        return Some(e);
    }
    if exact {
        return None;
    }
    let mut matching = encounters.iter().filter(|e| {
        e.plan.name.to_lowercase().starts_with(&name) || e.id.to_string().starts_with(&name)
    });
    match (matching.next(), matching.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// The monster called `name` (any case), the campaign's own before a
/// shared one.
pub fn monster_named<'m>(bestiary: &'m [MonsterSummary], name: &str) -> Option<&'m MonsterSummary> {
    let mut matching: Vec<_> = bestiary
        .iter()
        .filter(|m| m.name.eq_ignore_ascii_case(name))
        .collect();
    matching.sort_by_key(|m| m.shared);
    matching.first().copied()
}
