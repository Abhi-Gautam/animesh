//! The desired registration plan: what Animesh wants macOS to be holding.
//!
//! Selection is a read. It never touches the OS and never writes, so the
//! reconciler can recompute it as often as it likes to check whether the plan
//! moved under it mid-pass.

use crate::domain::ids::{EventUuid, UnixTimestamp};
use crate::domain::notification::{NotificationPlanItem, NATIVE_CAPACITY};
use crate::error::AppError;
use crate::store::connection::StoreError;
use crate::store::releases::{self, PlanRow};

use super::service::Library;

/// The plan as of one moment, tagged with the generation it was computed at.
///
/// The generation is what makes a pass safe to act on: if it has moved by the
/// time the pass finishes, the plan the pass worked from is stale and the
/// reconciler runs again rather than leaving the OS holding a superseded set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedPlan {
    pub generation: u64,
    pub computed_at: UnixTimestamp,
    pub items: Vec<NotificationPlanItem>,
    /// Jobs wanted beyond [`NATIVE_CAPACITY`]. Durable, just not registered yet.
    pub deferred: u32,
}

fn to_item(row: PlanRow) -> Result<NotificationPlanItem, StoreError> {
    let uuid = row
        .key
        .as_str()
        .parse()
        .map_err(|e| StoreError::Integrity(format!("notification key is not a uuid: {e}")))?;
    let request = row.request()?;
    Ok(NotificationPlanItem {
        key: row.key,
        release_event_id: row.release_event_id,
        event_uuid: EventUuid::from_uuid(uuid),
        desired_revision: row.desired_revision,
        request,
        retry_after: row.retry_after,
        state: row.state,
    })
}

impl Library {
    /// The nearest jobs that still want to exist in the OS, soonest first.
    pub async fn selected_plan(&self) -> Result<SelectedPlan, AppError> {
        // Read the generation before the query, never after. A bump that lands
        // mid-query then makes the plan look stale, which costs one extra pass;
        // reading it after would make a stale plan look current, which leaves
        // the OS wrong until something else happens to dirty the reconciler.
        let generation = self.plan_generation();
        let computed_at = self.now();

        let (items, deferred) = self
            .store()
            .read(move |conn| {
                let rows = releases::plan(conn, NATIVE_CAPACITY)?;
                let items = rows
                    .into_iter()
                    .map(to_item)
                    .collect::<Result<Vec<_>, StoreError>>()?;
                Ok((items, releases::deferred_count(conn, NATIVE_CAPACITY)?))
            })
            .await?;

        Ok(SelectedPlan {
            generation,
            computed_at,
            items,
            deferred,
        })
    }
}
