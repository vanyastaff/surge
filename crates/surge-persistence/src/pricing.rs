//! Pricing models for AI provider token consumption.
//!
//! This module defines pricing structures for different AI providers (Claude, GPT, Gemini)
//! and provides methods to calculate costs based on token usage.

use serde::{Deserialize, Serialize};

/// Pricing model for a specific AI model variant.
///
/// All rates are in USD per million tokens. Different token types may have
/// different pricing (e.g., cache reads are typically cheaper than regular input).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricingModel {
    /// Model identifier (e.g., "claude-3-5-sonnet-20241022").
    pub model_id: String,

    /// Price per million input tokens (USD).
    pub input_price_per_million: f64,

    /// Price per million output tokens (USD).
    pub output_price_per_million: f64,

    /// Price per million thought/reasoning tokens (USD).
    ///
    /// Used by models with extended thinking capabilities (e.g., Claude with thinking tokens).
    /// If `None`, thought tokens are priced the same as output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_price_per_million: Option<f64>,

    /// Price per million cache-read tokens (USD).
    ///
    /// Cache reads are typically cheaper than regular input tokens.
    /// If `None`, cache reads are priced the same as input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_price_per_million: Option<f64>,

    /// Price per million cache-write tokens (USD).
    ///
    /// Cache writes may have a different cost than regular input tokens.
    /// If `None`, cache writes are priced the same as input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_price_per_million: Option<f64>,
}

impl PricingModel {
    /// Calculate the cost for the given token usage.
    ///
    /// Returns the estimated cost in USD based on this pricing model.
    #[must_use]
    pub fn calculate_cost(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        thought_tokens: Option<u64>,
        cached_read_tokens: Option<u64>,
        cached_write_tokens: Option<u64>,
    ) -> f64 {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * self.input_price_per_million;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * self.output_price_per_million;

        let thought_cost = if let Some(tokens) = thought_tokens {
            let price = self
                .thought_price_per_million
                .unwrap_or(self.output_price_per_million);
            (tokens as f64 / 1_000_000.0) * price
        } else {
            0.0
        };

        let cache_read_cost = if let Some(tokens) = cached_read_tokens {
            let price = self
                .cache_read_price_per_million
                .unwrap_or(self.input_price_per_million);
            (tokens as f64 / 1_000_000.0) * price
        } else {
            0.0
        };

        let cache_write_cost = if let Some(tokens) = cached_write_tokens {
            let price = self
                .cache_write_price_per_million
                .unwrap_or(self.input_price_per_million);
            (tokens as f64 / 1_000_000.0) * price
        } else {
            0.0
        };

        input_cost + output_cost + thought_cost + cache_read_cost + cache_write_cost
    }

    /// Validate the pricing model.
    ///
    /// Ensures all prices are non-negative and the model ID is not empty.
    pub fn validate(&self) -> Result<(), String> {
        if self.model_id.trim().is_empty() {
            return Err("model_id must not be empty".to_string());
        }

        if self.input_price_per_million < 0.0 {
            return Err("input_price_per_million must be non-negative".to_string());
        }

        if self.output_price_per_million < 0.0 {
            return Err("output_price_per_million must be non-negative".to_string());
        }

        if let Some(price) = self.thought_price_per_million
            && price < 0.0
        {
            return Err("thought_price_per_million must be non-negative".to_string());
        }

        if let Some(price) = self.cache_read_price_per_million
            && price < 0.0
        {
            return Err("cache_read_price_per_million must be non-negative".to_string());
        }

        if let Some(price) = self.cache_write_price_per_million
            && price < 0.0
        {
            return Err("cache_write_price_per_million must be non-negative".to_string());
        }

        Ok(())
    }
}

/// AI provider identifier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Anthropic Claude models.
    Claude,
    /// OpenAI GPT models.
    Gpt,
    /// Google Gemini models.
    Gemini,
    /// Generic/unknown provider.
    #[serde(other)]
    Unknown,
}

impl Provider {
    /// Get the default pricing model for this provider.
    ///
    /// Returns the pricing for the provider's default model. For more granular
    /// control, use `get_model_pricing` with a specific model ID.
    #[must_use]
    pub fn default_pricing(self) -> PricingModel {
        match self {
            Provider::Claude => claude_sonnet_35_pricing(),
            Provider::Gpt => gpt4_turbo_pricing(),
            Provider::Gemini => gemini_pro_pricing(),
            Provider::Unknown => PricingModel {
                model_id: "unknown".to_string(),
                input_price_per_million: 0.0,
                output_price_per_million: 0.0,
                thought_price_per_million: None,
                cache_read_price_per_million: None,
                cache_write_price_per_million: None,
            },
        }
    }
}

// ── Default Pricing Models ──────────────────────────────────────────

/// Claude 3.5 Sonnet pricing.
///
/// Based on Anthropic's published rates as of 2026-03.
/// - Input: $3.00 per million tokens
/// - Output: $15.00 per million tokens
/// - Cache reads: $0.30 per million tokens (10x cheaper)
/// - Cache writes: $3.75 per million tokens (1.25x input rate)
#[must_use]
pub fn claude_sonnet_35_pricing() -> PricingModel {
    PricingModel {
        model_id: "claude-3-5-sonnet-20241022".to_string(),
        input_price_per_million: 3.0,
        output_price_per_million: 15.0,
        thought_price_per_million: Some(15.0), // Same as output
        cache_read_price_per_million: Some(0.3),
        cache_write_price_per_million: Some(3.75),
    }
}

/// Claude 3 Opus pricing.
///
/// Based on Anthropic's published rates as of 2026-03.
/// - Input: $15.00 per million tokens
/// - Output: $75.00 per million tokens
/// - Cache reads: $1.50 per million tokens (10x cheaper)
/// - Cache writes: $18.75 per million tokens (1.25x input rate)
#[must_use]
pub fn claude_opus_pricing() -> PricingModel {
    PricingModel {
        model_id: "claude-3-opus-20240229".to_string(),
        input_price_per_million: 15.0,
        output_price_per_million: 75.0,
        thought_price_per_million: Some(75.0),
        cache_read_price_per_million: Some(1.5),
        cache_write_price_per_million: Some(18.75),
    }
}

/// GPT-4 Turbo pricing.
///
/// Based on OpenAI's published rates as of 2026-03.
/// - Input: $10.00 per million tokens
/// - Output: $30.00 per million tokens
#[must_use]
pub fn gpt4_turbo_pricing() -> PricingModel {
    PricingModel {
        model_id: "gpt-4-turbo".to_string(),
        input_price_per_million: 10.0,
        output_price_per_million: 30.0,
        thought_price_per_million: None,
        cache_read_price_per_million: None,
        cache_write_price_per_million: None,
    }
}

/// GPT-4o pricing.
///
/// Based on OpenAI's published rates as of 2026-03.
/// - Input: $5.00 per million tokens
/// - Output: $15.00 per million tokens
#[must_use]
pub fn gpt4o_pricing() -> PricingModel {
    PricingModel {
        model_id: "gpt-4o".to_string(),
        input_price_per_million: 5.0,
        output_price_per_million: 15.0,
        thought_price_per_million: None,
        cache_read_price_per_million: None,
        cache_write_price_per_million: None,
    }
}

/// Gemini Pro pricing.
///
/// Based on Google's published rates as of 2026-03.
/// - Input: $0.50 per million tokens
/// - Output: $1.50 per million tokens
#[must_use]
pub fn gemini_pro_pricing() -> PricingModel {
    PricingModel {
        model_id: "gemini-pro".to_string(),
        input_price_per_million: 0.5,
        output_price_per_million: 1.5,
        thought_price_per_million: None,
        cache_read_price_per_million: None,
        cache_write_price_per_million: None,
    }
}

/// Gemini Pro 1.5 pricing.
///
/// Based on Google's published rates as of 2026-03.
/// - Input: $1.25 per million tokens
/// - Output: $5.00 per million tokens
#[must_use]
pub fn gemini_pro_15_pricing() -> PricingModel {
    PricingModel {
        model_id: "gemini-1.5-pro".to_string(),
        input_price_per_million: 1.25,
        output_price_per_million: 5.0,
        thought_price_per_million: None,
        cache_read_price_per_million: None,
        cache_write_price_per_million: None,
    }
}

/// Get pricing model by model ID.
///
/// Returns the appropriate pricing model for known model IDs, or a generic
/// pricing model for unknown IDs.
#[must_use]
pub fn get_model_pricing(model_id: &str) -> PricingModel {
    match model_id {
        // Claude models
        id if id.starts_with("claude-3-5-sonnet") => claude_sonnet_35_pricing(),
        id if id.starts_with("claude-3-opus") => claude_opus_pricing(),

        // GPT models
        id if id.starts_with("gpt-4-turbo") => gpt4_turbo_pricing(),
        id if id.starts_with("gpt-4o") => gpt4o_pricing(),

        // Gemini models
        id if id.starts_with("gemini-1.5-pro") => gemini_pro_15_pricing(),
        id if id.starts_with("gemini-pro") => gemini_pro_pricing(),

        // Unknown model
        _ => PricingModel {
            model_id: model_id.to_string(),
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        },
    }
}

// ── Cost Calculation Tests ──────────────────────────────────────────
// Comprehensive tests for the cost calculator functionality.
// This is a separate module to allow filtering with: cargo test pricing::cost_calculation_tests
#[cfg(test)]
pub mod cost_calculation_tests {
    use super::*;

    #[test]
    fn test_zero_tokens_zero_cost() {
        let pricing = claude_sonnet_35_pricing();
        let cost = pricing.calculate_cost(0, 0, None, None, None);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_input_tokens_only() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 1 million input tokens = 1M * $10 = $10.00
        let cost = pricing.calculate_cost(1_000_000, 0, None, None, None);
        assert!((cost - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_output_tokens_only() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 1 million output tokens = 1M * $30 = $30.00
        let cost = pricing.calculate_cost(0, 1_000_000, None, None, None);
        assert!((cost - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_small_token_amounts() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 100 input tokens = 0.0001M * $10 = $0.001
        // 50 output tokens = 0.00005M * $30 = $0.0015
        // Total = $0.0025
        let cost = pricing.calculate_cost(100, 50, None, None, None);
        assert!((cost - 0.0025).abs() < 0.000001);
    }

    #[test]
    fn test_large_token_amounts() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 10 million input tokens = 10M * $10 = $100.00
        // 5 million output tokens = 5M * $30 = $150.00
        // Total = $250.00
        let cost = pricing.calculate_cost(10_000_000, 5_000_000, None, None, None);
        assert!((cost - 250.0).abs() < 0.001);
    }

    #[test]
    fn test_thought_tokens_with_specific_pricing() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: Some(25.0),
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 20k thought = 0.02M * $25 = $0.50
        // Total = $3.00
        let cost = pricing.calculate_cost(100_000, 50_000, Some(20_000), None, None);
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_thought_tokens_fallback_to_output_pricing() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 20k thought = 0.02M * $30 (fallback) = $0.60
        // Total = $3.10
        let cost = pricing.calculate_cost(100_000, 50_000, Some(20_000), None, None);
        assert!((cost - 3.1).abs() < 0.001);
    }

    #[test]
    fn test_cache_read_tokens_with_specific_pricing() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: Some(1.0),
            cache_write_price_per_million: None,
        };

        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 500k cache reads = 0.5M * $1 = $0.50
        // Total = $3.00
        let cost = pricing.calculate_cost(100_000, 50_000, None, Some(500_000), None);
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_read_tokens_fallback_to_input_pricing() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 50k cache reads = 0.05M * $10 (fallback) = $0.50
        // Total = $3.00
        let cost = pricing.calculate_cost(100_000, 50_000, None, Some(50_000), None);
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_write_tokens_with_specific_pricing() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: Some(12.5),
        };

        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 40k cache writes = 0.04M * $12.5 = $0.50
        // Total = $3.00
        let cost = pricing.calculate_cost(100_000, 50_000, None, None, Some(40_000));
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_write_tokens_fallback_to_input_pricing() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 50k cache writes = 0.05M * $10 (fallback) = $0.50
        // Total = $3.00
        let cost = pricing.calculate_cost(100_000, 50_000, None, None, Some(50_000));
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_all_token_types_combined() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: Some(20.0),
            cache_read_price_per_million: Some(1.0),
            cache_write_price_per_million: Some(12.5),
        };

        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 25k thought = 0.025M * $20 = $0.50
        // 200k cache reads = 0.2M * $1 = $0.20
        // 40k cache writes = 0.04M * $12.5 = $0.50
        // Total = $3.70
        let cost = pricing.calculate_cost(
            100_000,
            50_000,
            Some(25_000),
            Some(200_000),
            Some(40_000),
        );
        assert!((cost - 3.7).abs() < 0.001);
    }

    #[test]
    fn test_claude_sonnet_35_realistic_session() {
        let pricing = claude_sonnet_35_pricing();

        // Typical coding session:
        // - 50k input tokens (new code context)
        // - 10k output tokens (generated code)
        // - 5k thought tokens (reasoning)
        // - 100k cache reads (reused context from previous turns)
        // - 10k cache writes (new context being cached)
        let cost =
            pricing.calculate_cost(50_000, 10_000, Some(5_000), Some(100_000), Some(10_000));

        // Expected calculation:
        // Input: 0.05M * $3.00 = $0.150
        // Output: 0.01M * $15.00 = $0.150
        // Thought: 0.005M * $15.00 = $0.075
        // Cache read: 0.1M * $0.30 = $0.030
        // Cache write: 0.01M * $3.75 = $0.0375
        // Total = $0.4425
        assert!((cost - 0.4425).abs() < 0.0001);
    }

    #[test]
    fn test_claude_opus_high_volume() {
        let pricing = claude_opus_pricing();

        // High-volume processing:
        // - 500k input tokens
        // - 100k output tokens
        let cost = pricing.calculate_cost(500_000, 100_000, None, None, None);

        // Expected calculation:
        // Input: 0.5M * $15.00 = $7.50
        // Output: 0.1M * $75.00 = $7.50
        // Total = $15.00
        assert!((cost - 15.0).abs() < 0.001);
    }

    #[test]
    fn test_gpt4_turbo_typical_usage() {
        let pricing = gpt4_turbo_pricing();

        // Typical GPT-4 Turbo usage:
        // - 100k input tokens
        // - 30k output tokens
        let cost = pricing.calculate_cost(100_000, 30_000, None, None, None);

        // Expected calculation:
        // Input: 0.1M * $10.00 = $1.00
        // Output: 0.03M * $30.00 = $0.90
        // Total = $1.90
        assert!((cost - 1.9).abs() < 0.001);
    }

    #[test]
    fn test_gpt4o_efficient_usage() {
        let pricing = gpt4o_pricing();

        // GPT-4o efficient usage:
        // - 200k input tokens
        // - 50k output tokens
        let cost = pricing.calculate_cost(200_000, 50_000, None, None, None);

        // Expected calculation:
        // Input: 0.2M * $5.00 = $1.00
        // Output: 0.05M * $15.00 = $0.75
        // Total = $1.75
        assert!((cost - 1.75).abs() < 0.001);
    }

    #[test]
    fn test_gemini_pro_15_batch_processing() {
        let pricing = gemini_pro_15_pricing();

        // Gemini Pro 1.5 batch processing:
        // - 1M input tokens
        // - 200k output tokens
        let cost = pricing.calculate_cost(1_000_000, 200_000, None, None, None);

        // Expected calculation:
        // Input: 1M * $1.25 = $1.25
        // Output: 0.2M * $5.00 = $1.00
        // Total = $2.25
        assert!((cost - 2.25).abs() < 0.001);
    }

    #[test]
    fn test_cost_precision_with_fractional_tokens() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 3.0,
            output_price_per_million: 15.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // Very small token counts to test precision:
        // 1 input token = 0.000001M * $3 = $0.000003
        // 1 output token = 0.000001M * $15 = $0.000015
        // Total = $0.000018
        let cost = pricing.calculate_cost(1, 1, None, None, None);
        assert!((cost - 0.000018).abs() < 0.0000001);
    }

    #[test]
    fn test_cost_calculation_with_zero_pricing() {
        let pricing = PricingModel {
            model_id: "free-model".to_string(),
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // Should return zero cost for any token amount
        let cost = pricing.calculate_cost(1_000_000, 1_000_000, None, None, None);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_none_optional_tokens_dont_contribute() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: Some(20.0),
            cache_read_price_per_million: Some(1.0),
            cache_write_price_per_million: Some(12.5),
        };

        // Only input and output should be counted
        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // Total = $2.50
        let cost = pricing.calculate_cost(100_000, 50_000, None, None, None);
        assert!((cost - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_some_zero_tokens_dont_contribute() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: Some(20.0),
            cache_read_price_per_million: Some(1.0),
            cache_write_price_per_million: Some(12.5),
        };

        // Only input and output should be counted (zeros don't add cost)
        // 100k input = 0.1M * $10 = $1.00
        // 50k output = 0.05M * $30 = $1.50
        // 0 thought = 0
        // 0 cache reads = 0
        // 0 cache writes = 0
        // Total = $2.50
        let cost = pricing.calculate_cost(100_000, 50_000, Some(0), Some(0), Some(0));
        assert!((cost - 2.5).abs() < 0.001);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pricing_model_calculate_cost_basic() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 100k input tokens = 0.1M * $10 = $1.00
        // 50k output tokens = 0.05M * $30 = $1.50
        // Total = $2.50
        let cost = pricing.calculate_cost(100_000, 50_000, None, None, None);
        assert!((cost - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_pricing_model_calculate_cost_with_thought_tokens() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: Some(20.0),
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };

        // 100k input tokens = 0.1M * $10 = $1.00
        // 50k output tokens = 0.05M * $30 = $1.50
        // 25k thought tokens = 0.025M * $20 = $0.50
        // Total = $3.00
        let cost = pricing.calculate_cost(100_000, 50_000, Some(25_000), None, None);
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_pricing_model_calculate_cost_with_cache() {
        let pricing = PricingModel {
            model_id: "test-model".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: Some(1.0),
            cache_write_price_per_million: Some(12.5),
        };

        // 100k input tokens = 0.1M * $10 = $1.00
        // 50k output tokens = 0.05M * $30 = $1.50
        // 200k cache reads = 0.2M * $1 = $0.20
        // 40k cache writes = 0.04M * $12.5 = $0.50
        // Total = $3.20
        let cost = pricing.calculate_cost(100_000, 50_000, None, Some(200_000), Some(40_000));
        assert!((cost - 3.2).abs() < 0.001);
    }

    #[test]
    fn test_pricing_model_validate_success() {
        let pricing = claude_sonnet_35_pricing();
        assert!(pricing.validate().is_ok());
    }

    #[test]
    fn test_pricing_model_validate_empty_id() {
        let pricing = PricingModel {
            model_id: "".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };
        assert!(pricing.validate().is_err());
    }

    #[test]
    fn test_pricing_model_validate_negative_input_price() {
        let pricing = PricingModel {
            model_id: "test".to_string(),
            input_price_per_million: -1.0,
            output_price_per_million: 30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };
        assert!(pricing.validate().is_err());
    }

    #[test]
    fn test_pricing_model_validate_negative_output_price() {
        let pricing = PricingModel {
            model_id: "test".to_string(),
            input_price_per_million: 10.0,
            output_price_per_million: -30.0,
            thought_price_per_million: None,
            cache_read_price_per_million: None,
            cache_write_price_per_million: None,
        };
        assert!(pricing.validate().is_err());
    }

    #[test]
    fn test_provider_default_pricing() {
        let claude_pricing = Provider::Claude.default_pricing();
        assert_eq!(claude_pricing.model_id, "claude-3-5-sonnet-20241022");
        assert_eq!(claude_pricing.input_price_per_million, 3.0);

        let gpt_pricing = Provider::Gpt.default_pricing();
        assert_eq!(gpt_pricing.model_id, "gpt-4-turbo");
        assert_eq!(gpt_pricing.input_price_per_million, 10.0);

        let gemini_pricing = Provider::Gemini.default_pricing();
        assert_eq!(gemini_pricing.model_id, "gemini-pro");
        assert_eq!(gemini_pricing.input_price_per_million, 0.5);
    }

    #[test]
    fn test_claude_sonnet_35_pricing() {
        let pricing = claude_sonnet_35_pricing();
        assert_eq!(pricing.model_id, "claude-3-5-sonnet-20241022");
        assert_eq!(pricing.input_price_per_million, 3.0);
        assert_eq!(pricing.output_price_per_million, 15.0);
        assert_eq!(pricing.thought_price_per_million, Some(15.0));
        assert_eq!(pricing.cache_read_price_per_million, Some(0.3));
        assert_eq!(pricing.cache_write_price_per_million, Some(3.75));
    }

    #[test]
    fn test_claude_opus_pricing() {
        let pricing = claude_opus_pricing();
        assert_eq!(pricing.model_id, "claude-3-opus-20240229");
        assert_eq!(pricing.input_price_per_million, 15.0);
        assert_eq!(pricing.output_price_per_million, 75.0);
    }

    #[test]
    fn test_gpt4_turbo_pricing() {
        let pricing = gpt4_turbo_pricing();
        assert_eq!(pricing.model_id, "gpt-4-turbo");
        assert_eq!(pricing.input_price_per_million, 10.0);
        assert_eq!(pricing.output_price_per_million, 30.0);
        assert_eq!(pricing.thought_price_per_million, None);
    }

    #[test]
    fn test_gpt4o_pricing() {
        let pricing = gpt4o_pricing();
        assert_eq!(pricing.model_id, "gpt-4o");
        assert_eq!(pricing.input_price_per_million, 5.0);
        assert_eq!(pricing.output_price_per_million, 15.0);
    }

    #[test]
    fn test_gemini_pro_pricing() {
        let pricing = gemini_pro_pricing();
        assert_eq!(pricing.model_id, "gemini-pro");
        assert_eq!(pricing.input_price_per_million, 0.5);
        assert_eq!(pricing.output_price_per_million, 1.5);
    }

    #[test]
    fn test_gemini_pro_15_pricing() {
        let pricing = gemini_pro_15_pricing();
        assert_eq!(pricing.model_id, "gemini-1.5-pro");
        assert_eq!(pricing.input_price_per_million, 1.25);
        assert_eq!(pricing.output_price_per_million, 5.0);
    }

    #[test]
    fn test_get_model_pricing_claude() {
        let pricing = get_model_pricing("claude-3-5-sonnet-20241022");
        assert_eq!(pricing.input_price_per_million, 3.0);

        let pricing = get_model_pricing("claude-3-opus-20240229");
        assert_eq!(pricing.input_price_per_million, 15.0);
    }

    #[test]
    fn test_get_model_pricing_gpt() {
        let pricing = get_model_pricing("gpt-4-turbo");
        assert_eq!(pricing.input_price_per_million, 10.0);

        let pricing = get_model_pricing("gpt-4o");
        assert_eq!(pricing.input_price_per_million, 5.0);
    }

    #[test]
    fn test_get_model_pricing_gemini() {
        let pricing = get_model_pricing("gemini-pro");
        assert_eq!(pricing.input_price_per_million, 0.5);

        let pricing = get_model_pricing("gemini-1.5-pro");
        assert_eq!(pricing.input_price_per_million, 1.25);
    }

    #[test]
    fn test_get_model_pricing_unknown() {
        let pricing = get_model_pricing("unknown-model-xyz");
        assert_eq!(pricing.model_id, "unknown-model-xyz");
        assert_eq!(pricing.input_price_per_million, 0.0);
        assert_eq!(pricing.output_price_per_million, 0.0);
    }

    #[test]
    fn test_realistic_claude_cost_calculation() {
        let pricing = claude_sonnet_35_pricing();

        // Simulate a typical session:
        // - 10k input tokens
        // - 2k output tokens
        // - 500 thought tokens
        // - 5k cache read tokens (from previous context)
        // - 1k cache write tokens
        let cost = pricing.calculate_cost(10_000, 2_000, Some(500), Some(5_000), Some(1_000));

        // Expected calculation:
        // Input: 0.01M * $3.00 = $0.030
        // Output: 0.002M * $15.00 = $0.030
        // Thought: 0.0005M * $15.00 = $0.0075
        // Cache read: 0.005M * $0.30 = $0.0015
        // Cache write: 0.001M * $3.75 = $0.00375
        // Total = $0.07275
        assert!((cost - 0.07275).abs() < 0.00001);
    }

    #[test]
    fn test_provider_serialization() {
        let provider = Provider::Claude;
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(json, "\"claude\"");

        let deserialized: Provider = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, Provider::Claude);
    }

    #[test]
    fn test_pricing_model_serialization() {
        let pricing = claude_sonnet_35_pricing();
        let json = serde_json::to_string(&pricing).unwrap();
        let deserialized: PricingModel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, pricing);
    }
}
