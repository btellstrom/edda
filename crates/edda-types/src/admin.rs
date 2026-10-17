//! What every service's admin API answers in common (see
//! `edda_server::admin`).

use serde::{Deserialize, Serialize};

/// `DELETE /admin/campaigns/{id}`: how many of the campaign's things a
/// service deleted (characters, combats, monsters and texts, or devices).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Deleted {
    pub deleted: u64,
}
