use serde::{Deserialize, Serialize};

pub(crate) const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// Output tokens cost ~5x more than input tokens across every model in
/// `PRICING_TABLE` below. Used to weight stacked bars by cost contribution.
pub(crate) const OUTPUT_COST_WEIGHT: f64 = 5.0;

/// Per-model token counts and USD cost contributions within a time window.
/// Stored in rate-history / hit-history so that the per-side cost split
/// (input / cache / output) survives even after the underlying JSONL
/// events are rotated out of the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerModelUsage {
    /// Full model string as seen in the JSONL (e.g. "claude-opus-4-6-...").
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub input_cost: f64,
    pub cache_cost: f64,
    pub output_cost: f64,
}

impl PerModelUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn total_cost(&self) -> f64 {
        self.input_cost + self.cache_cost + self.output_cost
    }
}

/// Sum `(input_cost, cache_cost, output_cost)` over a per-model breakdown.
pub fn per_model_cost_split(per_model: &[PerModelUsage]) -> (f64, f64, f64) {
    per_model.iter().fold((0.0, 0.0, 0.0), |acc, m| {
        (
            acc.0 + m.input_cost,
            acc.1 + m.cache_cost,
            acc.2 + m.output_cost,
        )
    })
}

/// Sum `(input_tokens + output_tokens)` over a per-model breakdown.
pub fn per_model_total_tokens(per_model: &[PerModelUsage]) -> u64 {
    per_model.iter().map(|m| m.total_tokens()).sum()
}

/// (pattern, (input_price, output_price, cache_read_price)) per million tokens.
///
/// Patterns use dash form (`opus-4-7`). Bedrock and other proxies often emit
/// dot form (`aws.claude-opus-4.7`); `model_pricing` normalizes dots to dashes
/// before matching so a single table covers both.
const PRICING_TABLE: &[(&str, (f64, f64, f64))] = &[
    ("opus-4-7", (5.0, 25.0, 0.50)),
    ("opus-4-6", (5.0, 25.0, 0.50)),
    ("opus-4-5", (5.0, 25.0, 0.50)),
    ("opus-4-1", (15.0, 75.0, 1.50)),
    ("opus-4-0", (15.0, 75.0, 1.50)),
    ("opus-4-2", (15.0, 75.0, 1.50)),
    ("3-opus", (15.0, 75.0, 1.50)),
    ("sonnet", (3.0, 15.0, 0.30)),
    ("haiku-4-5", (1.0, 5.0, 0.10)),
    ("3-5-haiku", (0.80, 4.0, 0.08)),
    ("3-haiku", (0.25, 1.25, 0.03)),
];

/// Family-default pricing for unknown versions. Tuned to the *latest* known
/// pricing for each family, on the rationale that an unknown model is most
/// likely a newer release of an existing family (the bigger risk is silently
/// under-billing Opus-tier traffic, not over-billing a future Sonnet).
const OPUS_LATEST_PRICING: (f64, f64, f64) = (5.0, 25.0, 0.50);
const SONNET_LATEST_PRICING: (f64, f64, f64) = (3.0, 15.0, 0.30);
const HAIKU_LATEST_PRICING: (f64, f64, f64) = (1.0, 5.0, 0.10);

/// Per-million-token (input, output, cache_read) prices for a given model
/// string. Two-stage lookup:
///
/// 1. Normalize Bedrock-style dots to dashes (`aws.claude-opus-4.7` →
///    `aws-claude-opus-4-7`), strip case, then substring-match against
///    `PRICING_TABLE`.
/// 2. If no specific version matches, classify by family substring
///    (`opus`/`sonnet`/`haiku`) and fall back to the latest known pricing
///    for that family.
///
/// The previous implementation only recognized dash form and defaulted
/// every miss to Sonnet, which silently mis-billed every Bedrock-proxy
/// user on Opus and every Haiku call too.
pub(crate) fn model_pricing(model: &str) -> (f64, f64, f64) {
    let normalized = model.to_ascii_lowercase().replace('.', "-");

    if let Some((_, pricing)) = PRICING_TABLE
        .iter()
        .find(|(pattern, _)| normalized.contains(pattern))
    {
        return *pricing;
    }

    if normalized.contains("opus") {
        OPUS_LATEST_PRICING
    } else if normalized.contains("haiku") {
        HAIKU_LATEST_PRICING
    } else {
        // Sonnet is also the safe default for empty / unknown strings:
        // matches the previous fallback behavior for true unknowns while
        // letting Opus and Haiku route to their proper family rates above.
        SONNET_LATEST_PRICING
    }
}

/// Normalize a full model ID to a short family name.
pub(crate) fn normalize_model(model: &str) -> &'static str {
    if model.contains("opus") {
        "opus"
    } else if model.contains("sonnet") {
        "sonnet"
    } else if model.contains("haiku") {
        "haiku"
    } else {
        "other"
    }
}

/// Format a token count as a human-readable string (e.g. "1.2M", "450K", "99").
pub(crate) fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / TOKENS_PER_MILLION)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

/// Format a USD cost as a compact human-readable string
/// (e.g. "¢42", "$1.23", "$45", "$1.2K").
pub(crate) fn format_cost(c: f64) -> String {
    if !c.is_finite() || c <= 0.0 {
        return "$0".to_string();
    }
    if c >= 1000.0 {
        format!("${:.1}K", c / 1000.0)
    } else if c >= 100.0 {
        format!("${:.0}", c)
    } else if c >= 1.0 {
        format!("${:.2}", c)
    } else {
        format!("¢{:.0}", c * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPUS: (f64, f64, f64) = (5.0, 25.0, 0.50);
    const OPUS_LEGACY: (f64, f64, f64) = (15.0, 75.0, 1.50);
    const SONNET: (f64, f64, f64) = (3.0, 15.0, 0.30);
    const HAIKU: (f64, f64, f64) = (1.0, 5.0, 0.10);
    const HAIKU_3_5: (f64, f64, f64) = (0.80, 4.0, 0.08);

    // ----- Direct API (dash form) — pre-existing behavior must hold -------

    #[test]
    fn prices_direct_opus_4_6_dash() {
        assert_eq!(model_pricing("claude-opus-4-6-20250514"), OPUS);
    }

    #[test]
    fn prices_direct_opus_4_7_dash() {
        // Previously fell through to FALLBACK (sonnet). Now correctly Opus.
        assert_eq!(model_pricing("claude-opus-4-7"), OPUS);
    }

    #[test]
    fn prices_direct_sonnet_dash() {
        assert_eq!(model_pricing("claude-sonnet-4-5-20251022"), SONNET);
    }

    #[test]
    fn prices_direct_haiku_4_5_dash() {
        assert_eq!(model_pricing("claude-haiku-4-5"), HAIKU);
    }

    #[test]
    fn prices_legacy_opus_4_1_dash() {
        assert_eq!(model_pricing("claude-opus-4-1-20240229"), OPUS_LEGACY);
    }

    #[test]
    fn prices_legacy_3_5_haiku() {
        assert_eq!(model_pricing("claude-3-5-haiku-20241022"), HAIKU_3_5);
    }

    // ----- Bedrock / proxy (dot form) — bug being fixed -------------------

    #[test]
    fn prices_bedrock_opus_4_7_dot() {
        // The bug: dot form never matched any dash pattern → fell back to
        // sonnet. Now normalized to dash form before lookup.
        assert_eq!(model_pricing("aws.claude-opus-4.7"), OPUS);
    }

    #[test]
    fn prices_bedrock_opus_4_6_dot() {
        assert_eq!(model_pricing("aws.claude-opus-4.6"), OPUS);
    }

    #[test]
    fn prices_bedrock_haiku_4_5_dot() {
        // Was billed as sonnet ($3/$15) instead of haiku ($1/$5) — over by 3x.
        assert_eq!(model_pricing("aws.claude-haiku-4.5"), HAIKU);
    }

    #[test]
    fn prices_bedrock_sonnet_4_5_dot() {
        // Coincidentally correct under the old fallback, but now via the
        // explicit pattern path.
        assert_eq!(model_pricing("aws.claude-sonnet-4.5"), SONNET);
    }

    // ----- Family fallback for unknown versions ---------------------------

    #[test]
    fn unknown_opus_version_falls_back_to_opus_family() {
        // Future Opus release we don't know about yet. Must NOT silently
        // route to sonnet (under-billing risk).
        assert_eq!(model_pricing("claude-opus-9-9"), OPUS);
        assert_eq!(model_pricing("aws.claude-opus-9.9"), OPUS);
    }

    #[test]
    fn unknown_haiku_version_falls_back_to_haiku_family() {
        assert_eq!(model_pricing("claude-haiku-9-9"), HAIKU);
        assert_eq!(model_pricing("aws.claude-haiku-9.9"), HAIKU);
    }

    #[test]
    fn unknown_sonnet_version_falls_back_to_sonnet_family() {
        // Sonnet pattern is just "sonnet", so a future "claude-sonnet-9"
        // already matches the explicit table. The family-fallback branch
        // catches truly opaque strings ("claude-experimental-x").
        assert_eq!(model_pricing("claude-sonnet-9"), SONNET);
        assert_eq!(model_pricing("aws.claude-sonnet-9.9"), SONNET);
    }

    #[test]
    fn truly_unknown_string_defaults_to_sonnet() {
        // No family substring — preserve previous default-to-sonnet behavior.
        assert_eq!(model_pricing("claude-unknown-experiment"), SONNET);
        assert_eq!(model_pricing(""), SONNET);
    }

    #[test]
    fn case_insensitive_lookup() {
        // Lowercasing is part of normalization; uppercase model IDs occur
        // in the wild (some proxies uppercase ARN-style identifiers).
        assert_eq!(model_pricing("AWS.CLAUDE-OPUS-4.7"), OPUS);
        assert_eq!(model_pricing("Claude-Opus-4-7"), OPUS);
    }
}
