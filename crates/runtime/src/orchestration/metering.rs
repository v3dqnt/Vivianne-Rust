//! What a subagent's model calls cost, recorded against the conversation that started it, so its day, model and
//! thread totals and its run limit count them.

use vivianne_ai::ai::types::{CostBreakdown, Usage};
use vivianne_ai::inference::usage_store;
use vivianne_providers::Spent;

/// Records one report of usage for `thread`. A model with no known price adds no cost: unknown, never zero.
pub(super) async fn record(thread: &str, spent: &Spent) {
    let clamp = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    let usage = Usage {
        input: clamp(spent.input),
        output: clamp(spent.output),
        cache_read: clamp(spent.cache_read),
        cache_write: clamp(spent.cache_write),
        total_tokens: clamp(spent.input.saturating_add(spent.output)),
        cost: spent.cost.map(|total| CostBreakdown {
            total,
            ..CostBreakdown::default()
        }),
    };
    usage_store::record_usage(&usage, &spent.provider, &spent.model).await;
    usage_store::record_usage_tagged(
        &usage,
        &spent.provider,
        &spent.model,
        &format!("task:{thread}"),
    )
    .await;
    crate::ledger::record_usage(
        thread,
        &spent.model,
        spent.input + spent.cache_read,
        spent.output,
        spent.cost,
    );
}
