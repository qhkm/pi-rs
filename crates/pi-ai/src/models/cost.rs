/// Cost calculation utilities for model usage.
use crate::messages::types::{Usage, UsageCost};
use crate::models::registry::ModelCost;

/// Calculate the dollar cost of a completion given model pricing (per-million
/// token rates) and token counts.
pub fn calculate_cost(pricing: &ModelCost, usage: &Usage) -> UsageCost {
    let input_cost = (usage.input as f64) * pricing.input / 1_000_000.0;
    let output_cost = (usage.output as f64) * pricing.output / 1_000_000.0;
    let cache_read_cost = (usage.cache_read as f64) * pricing.cache_read / 1_000_000.0;
    let cache_write_cost = (usage.cache_write as f64) * pricing.cache_write / 1_000_000.0;

    let total = input_cost + output_cost + cache_read_cost + cache_write_cost;

    UsageCost {
        input: input_cost,
        output: output_cost,
        cache_read: cache_read_cost,
        cache_write: cache_write_cost,
        total,
    }
}

/// Annotate a `Usage` struct with cost information derived from the given pricing.
pub fn annotate_usage(pricing: &ModelCost, mut usage: Usage) -> Usage {
    usage.cost = calculate_cost(pricing, &usage);
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::registry::ModelCost;

    #[test]
    fn test_calculate_cost() {
        let pricing = ModelCost {
            input: 3.0,   // $3 / M tokens
            output: 15.0, // $15 / M tokens
            cache_read: 0.3,
            cache_write: 3.75,
        };

        let usage = Usage {
            input: 1_000_000,
            output: 500_000,
            cache_read: 0,
            cache_write: 0,
            total_tokens: 1_500_000,
            cost: UsageCost::default(),
        };

        let cost = calculate_cost(&pricing, &usage);

        assert!((cost.input - 3.0).abs() < 1e-9);
        assert!((cost.output - 7.5).abs() < 1e-9);
        assert!((cost.total - 10.5).abs() < 1e-9);
    }
}
