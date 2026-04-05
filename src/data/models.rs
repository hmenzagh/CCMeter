pub(crate) const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// (pattern, (input_price, output_price, cache_read_price)) per million tokens.
const PRICING_TABLE: &[(&str, (f64, f64, f64))] = &[
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

const FALLBACK_PRICING: (f64, f64, f64) = (3.0, 15.0, 0.30);

/// Prix par million de tokens (input, output, cache_read) pour chaque modèle.
pub(crate) fn model_pricing(model: &str) -> (f64, f64, f64) {
    PRICING_TABLE
        .iter()
        .find(|(pattern, _)| model.contains(pattern))
        .map(|(_, pricing)| *pricing)
        .unwrap_or(FALLBACK_PRICING)
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
