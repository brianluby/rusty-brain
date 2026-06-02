//! Link-decay job: exponentially decay link `strength` by age, floored. Reads
//! candidate links via the read pool; writes via the single writer.

use crate::jobs::config::LinkDecayConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;

/// Run one bounded link-decay pass. (R4 stub: no candidate source yet, so an
/// empty store scans nothing. Real algorithm + store seam land in R5/R6.)
pub async fn run(_store: &StoreHandle, _config: &LinkDecayConfig) -> rb_types::Result<JobSummary> {
    Ok(JobSummary::default())
}
