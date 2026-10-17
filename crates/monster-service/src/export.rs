//! A campaign's own monsters, spells and conditions, exported to keep or
//! to move to another server, and imported there.

use edda_types::{
    admin::MonstersExport,
    rules_text::{ConditionText, SpellText},
};
use uuid::Uuid;

use crate::{MonsterStore, error::ApiError, store, texts};

impl MonsterStore {
    /// The campaign's own, as they are.
    pub async fn export_campaign(&self, campaign: Uuid) -> Result<MonstersExport, ApiError> {
        Ok(MonstersExport {
            monsters: self.export_own(campaign).await?,
            spells: self.export_texts::<SpellText>(campaign).await?,
            conditions: self.export_texts::<ConditionText>(campaign).await?,
        })
    }

    /// Takes in a campaign's own exported elsewhere, as they were: ids,
    /// versions and dates. All or nothing, and only while it has none here;
    /// returns how many came.
    pub async fn import_campaign(
        &self,
        campaign: Uuid,
        export: &MonstersExport,
    ) -> Result<u64, ApiError> {
        let ours = Some(campaign);
        if export.monsters.iter().any(|m| m.campaign != ours)
            || export.spells.iter().any(|s| s.campaign != ours)
            || export.conditions.iter().any(|c| c.campaign != ours)
        {
            return Err(ApiError::BadRequest(
                "the export holds what isn't this campaign's own".into(),
            ));
        }
        let mut tx = self.pool().begin().await?;
        let (kept,): (i64,) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM monsters WHERE campaign = $1) \
             + (SELECT COUNT(*) FROM rules_text WHERE campaign = $1)",
        )
        .bind(campaign)
        .fetch_one(&mut *tx)
        .await?;
        if kept > 0 {
            return Err(ApiError::Conflict(format!(
                "campaign {campaign} has {kept} monster(s), spell(s) or condition(s) here already"
            )));
        }
        for monster in &export.monsters {
            store::insert_as_is(&mut *tx, monster).await?;
        }
        for spell in &export.spells {
            texts::insert_as_is(&mut *tx, spell).await?;
        }
        for condition in &export.conditions {
            texts::insert_as_is(&mut *tx, condition).await?;
        }
        tx.commit().await?;
        let count = export.monsters.len() + export.spells.len() + export.conditions.len();
        Ok(u64::try_from(count).unwrap_or(u64::MAX))
    }
}
