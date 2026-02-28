/// API key resolution from environment variables.
///
/// Keys are resolved in priority order: first a provider-specific variable,
/// then any known aliases.  Returns `None` if no variable is set.

/// Look up the API key for a named provider from environment variables.
///
/// # Supported providers
///
/// | `provider` value | Variables checked (in order) |
/// |---|---|
/// | `"anthropic"` | `ANTHROPIC_API_KEY` |
/// | `"openai"` | `OPENAI_API_KEY` |
/// | `"google"` | `GOOGLE_API_KEY`, `GEMINI_API_KEY` |
/// | `"groq"` | `GROQ_API_KEY` |
/// | `"mistral"` | `MISTRAL_API_KEY` |
/// | `"x-ai"` / `"xai"` | `XAI_API_KEY` |
/// | `"cerebras"` | `CEREBRAS_API_KEY` |
/// | `"openrouter"` | `OPENROUTER_API_KEY` |
/// | `"huggingface"` | `HUGGINGFACE_API_KEY`, `HF_TOKEN` |
/// | `"azure-openai"` | `AZURE_OPENAI_API_KEY` |
/// | `"amazon-bedrock"` | `AWS_ACCESS_KEY_ID` (note: Bedrock uses SigV4) |
pub fn get_api_key(provider: &str) -> Option<String> {
    let key_names: &[&str] = match provider.to_lowercase().as_str() {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "google" | "google-generative-ai" | "google-vertex" => {
            &["GOOGLE_API_KEY", "GEMINI_API_KEY"]
        }
        "groq" => &["GROQ_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "x-ai" | "xai" => &["XAI_API_KEY"],
        "cerebras" => &["CEREBRAS_API_KEY"],
        "openrouter" | "open-router" => &["OPENROUTER_API_KEY"],
        "huggingface" | "hugging-face" => &["HUGGINGFACE_API_KEY", "HF_TOKEN"],
        "azure-openai" | "azure" => &["AZURE_OPENAI_API_KEY"],
        "amazon-bedrock" | "bedrock" => &["AWS_ACCESS_KEY_ID"],
        _ => &[],
    };

    key_names
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
}

/// Return `true` if the given API key looks like a plausible secret (non-empty,
/// not an obvious placeholder).
pub fn is_valid_api_key(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let lower = key.to_lowercase();
    let placeholders = [
        "your-api-key",
        "xxx",
        "changeme",
        "placeholder",
        "your_api_key",
        "insert_key_here",
        "sk-...",
        "<api_key>",
    ];
    !placeholders.iter().any(|p| lower.contains(p))
}

/// Redact an API key for safe inclusion in error messages and logs.
///
/// Shows the first 4 and last 4 characters separated by `***`.
/// For keys shorter than 12 characters the entire value is replaced with `***`.
///
/// # Examples
///
/// ```
/// use pi_ai::auth::api_key::redact_key;
///
/// assert_eq!(redact_key("sk-proj-abc123456789xyz"), "sk-p***9xyz");
/// assert_eq!(redact_key("short"), "***");
/// ```
pub fn redact_key(key: &str) -> String {
    if key.len() < 12 {
        "***".to_string()
    } else {
        let prefix = &key[..4];
        let suffix = &key[key.len() - 4..];
        format!("{prefix}***{suffix}")
    }
}

/// Validate and return the API key, or return an auth error.
pub fn require_api_key(provider: &str, options_key: Option<&str>) -> crate::error::Result<String> {
    let key = options_key
        .map(|k| k.to_string())
        .or_else(|| get_api_key(provider));

    match key {
        Some(k) if is_valid_api_key(&k) => Ok(k),
        Some(k) => Err(crate::error::PiAiError::Auth(format!(
            "API key for provider '{provider}' appears to be a placeholder: '{}'",
            redact_key(&k)
        ))),
        None => Err(crate::error::PiAiError::Auth(format!(
            "No API key found for provider '{provider}'. \
             Set the appropriate environment variable."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_api_key() {
        assert!(is_valid_api_key("sk-proj-abc123realkey"));
        assert!(!is_valid_api_key(""));
        assert!(!is_valid_api_key("your-api-key"));
        assert!(!is_valid_api_key("xxx"));
    }

    #[test]
    fn test_redact_key_long_key() {
        // Typical OpenAI-style key: first 4 + *** + last 4
        assert_eq!(redact_key("sk-proj-abc123456789xyz"), "sk-p***9xyz");
    }

    #[test]
    fn test_redact_key_exactly_12_chars() {
        // 12 characters is the minimum to show prefix/suffix
        assert_eq!(redact_key("abcd12345678"), "abcd***5678");
    }

    #[test]
    fn test_redact_key_11_chars_shows_stars_only() {
        // 11 characters — below threshold, full redaction
        assert_eq!(redact_key("abcd1234567"), "***");
    }

    #[test]
    fn test_redact_key_short_key() {
        assert_eq!(redact_key("short"), "***");
    }

    #[test]
    fn test_redact_key_empty() {
        assert_eq!(redact_key(""), "***");
    }

    #[test]
    fn test_redact_key_does_not_expose_middle() {
        let key = "sk-proj-SUPERSECRETMIDDLEPART-end";
        let redacted = redact_key(key);
        assert!(redacted.starts_with("sk-p"));
        assert!(redacted.ends_with("-end"));
        assert!(redacted.contains("***"));
        // Middle must not appear in the redacted string
        assert!(!redacted.contains("SUPERSECRETMIDDLEPART"));
    }
}
