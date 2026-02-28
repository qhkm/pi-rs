/// HTTP proxy configuration for `reqwest` clients.
///
/// Reads the standard proxy environment variables (`HTTPS_PROXY`, `HTTP_PROXY`,
/// `https_proxy`, `http_proxy`, `NO_PROXY`, `no_proxy`) and wires them into a
/// [`reqwest::ClientBuilder`].
///
/// # Why this module exists
///
/// `reqwest` does _not_ automatically read proxy environment variables when
/// built with `default-features = false` (which disables the `default-tls`
/// feature flag that activates the automatic `env_proxy` path).  This crate
/// uses `rustls-tls` without the `default` feature set, so we need to read and
/// configure proxy settings ourselves.
///
/// All providers call [`build_http_client`] instead of constructing their own
/// `reqwest::Client`, which keeps proxy and timeout behaviour consistent across
/// Anthropic, OpenAI, and Google backends.

use std::time::Duration;

use tracing::debug;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Build a `reqwest::Client` with a consistent timeout and proxy settings
/// derived from the process environment.
///
/// Environment variables consulted (checked in order, first non-empty wins):
///
/// | Variable | Applies to |
/// |---|---|
/// | `HTTPS_PROXY` / `https_proxy` | HTTPS requests |
/// | `HTTP_PROXY` / `http_proxy` | HTTP requests |
/// | `ALL_PROXY` / `all_proxy` | Both HTTP and HTTPS requests |
/// | `NO_PROXY` / `no_proxy` | Hosts that bypass the proxy |
///
/// # Panics
///
/// Panics if the TLS backend fails to initialise — this is a hard failure
/// because all providers depend on a working HTTPS stack.
pub fn build_http_client(timeout_secs: u64) -> reqwest::Client {
    configure_proxy(
        reqwest::Client::builder().timeout(Duration::from_secs(timeout_secs)),
    )
    .build()
    .expect("failed to build HTTP client")
}

/// Apply proxy settings from environment variables to a [`reqwest::ClientBuilder`].
///
/// Returns the (possibly modified) builder so callers can chain additional
/// configuration before calling `.build()`.
///
/// This is exposed as a separate function so tests and specialised code-paths
/// can compose it with their own builder configuration.
pub fn configure_proxy(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // ── HTTPS proxy ──────────────────────────────────────────────────────────
    if let Some(url) = env_var_nonempty(&["HTTPS_PROXY", "https_proxy"]) {
        match reqwest::Proxy::https(&url) {
            Ok(proxy) => {
                debug!(proxy_url = %url, "HTTPS proxy configured");
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                tracing::warn!(proxy_url = %url, error = %e, "invalid HTTPS proxy URL, ignoring");
            }
        }
    }

    // ── HTTP proxy ───────────────────────────────────────────────────────────
    if let Some(url) = env_var_nonempty(&["HTTP_PROXY", "http_proxy"]) {
        match reqwest::Proxy::http(&url) {
            Ok(proxy) => {
                debug!(proxy_url = %url, "HTTP proxy configured");
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                tracing::warn!(proxy_url = %url, error = %e, "invalid HTTP proxy URL, ignoring");
            }
        }
    }

    // ── ALL_PROXY (covers both HTTP and HTTPS if the above are not set) ──────
    if let Some(url) = env_var_nonempty(&["ALL_PROXY", "all_proxy"]) {
        match reqwest::Proxy::all(&url) {
            Ok(proxy) => {
                debug!(proxy_url = %url, "ALL proxy configured");
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                tracing::warn!(proxy_url = %url, error = %e, "invalid ALL_PROXY URL, ignoring");
            }
        }
    }

    // ── NO_PROXY ─────────────────────────────────────────────────────────────
    // reqwest accepts a comma-separated list of hostnames / CIDR ranges that
    // should bypass the proxy.  We pass the raw string through unchanged so
    // that the standard curl-compatible syntax is honoured (e.g.
    // `NO_PROXY=localhost,127.0.0.1,.internal.corp`).
    //
    // Note: there is no dedicated `reqwest::NoProxy` builder method — instead
    // reqwest reads `no_proxy` / `NO_PROXY` via the `Proxy::custom` path.
    // The simplest compliant approach is to call `reqwest::Proxy::custom` on
    // the matching schemes and handle the no-proxy filter there.  For
    // compatibility we instead rely on reqwest's built-in env-proxy support
    // which _does_ honour `NO_PROXY` when proxies were explicitly set above.
    //
    // If you need fine-grained no-proxy logic, set it on the individual
    // `reqwest::Proxy` via `.no_proxy(reqwest::NoProxy::from_env())`.
    if let Some(no_proxy_val) = env_var_nonempty(&["NO_PROXY", "no_proxy"]) {
        debug!(no_proxy = %no_proxy_val, "NO_PROXY configured (handled by reqwest proxy filter)");
        // Re-emit the proxies with the no_proxy filter applied.
        builder = apply_no_proxy(builder, &no_proxy_val);
    }

    builder
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Return the value of the first non-empty environment variable in `names`.
fn env_var_nonempty(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(val) = std::env::var(name) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// Re-create proxy entries with a `NoProxy` filter derived from `no_proxy_str`.
///
/// reqwest's `Proxy::no_proxy` method accepts a `reqwest::NoProxy` which can
/// be built from a comma-separated list of bypass patterns — the same format
/// used by curl and most UNIX tools.
fn apply_no_proxy(mut builder: reqwest::ClientBuilder, no_proxy_str: &str) -> reqwest::ClientBuilder {
    let no_proxy = reqwest::NoProxy::from_string(no_proxy_str);

    // HTTPS proxy with no_proxy filter.
    if let Some(url) = env_var_nonempty(&["HTTPS_PROXY", "https_proxy"]) {
        if let Ok(proxy) = reqwest::Proxy::https(&url) {
            builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
        }
    }

    // HTTP proxy with no_proxy filter.
    if let Some(url) = env_var_nonempty(&["HTTP_PROXY", "http_proxy"]) {
        if let Ok(proxy) = reqwest::Proxy::http(&url) {
            builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
        }
    }

    // ALL_PROXY with no_proxy filter.
    if let Some(url) = env_var_nonempty(&["ALL_PROXY", "all_proxy"]) {
        if let Ok(proxy) = reqwest::Proxy::all(&url) {
            builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
        }
    }

    builder
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain client (no proxy env vars set) must build successfully.
    /// This exercises the TLS initialisation path.
    #[test]
    fn client_builds_without_proxy_env() {
        // Temporarily clear any proxy vars that might be set in the test
        // environment so this test is deterministic.
        let _guard = EnvGuard::clear(&[
            "HTTPS_PROXY", "https_proxy",
            "HTTP_PROXY",  "http_proxy",
            "ALL_PROXY",   "all_proxy",
            "NO_PROXY",    "no_proxy",
        ]);

        let client = build_http_client(30);
        // The fact that we reach this line means `.build()` did not panic.
        // We perform a trivial assertion to prevent the binding being dropped.
        drop(client);
    }

    /// When `HTTPS_PROXY` is set to a valid URL the builder should not panic.
    #[test]
    fn client_builds_with_https_proxy() {
        let _guard = EnvGuard::set("HTTPS_PROXY", "http://proxy.example.com:3128");

        let client = build_http_client(30);
        drop(client);
    }

    /// When `HTTP_PROXY` is set to a valid URL the builder should not panic.
    #[test]
    fn client_builds_with_http_proxy() {
        let _guard = EnvGuard::set("HTTP_PROXY", "http://proxy.example.com:8080");

        let client = build_http_client(30);
        drop(client);
    }

    /// When `ALL_PROXY` is set to a valid URL the builder should not panic.
    #[test]
    fn client_builds_with_all_proxy() {
        let _guard = EnvGuard::set("ALL_PROXY", "http://proxy.example.com:1080");

        let client = build_http_client(30);
        drop(client);
    }

    /// An invalid proxy URL should not panic — the warning path is taken
    /// instead and the client still builds (without a proxy).
    #[test]
    fn client_builds_with_invalid_proxy_url() {
        let _guard = EnvGuard::set("HTTPS_PROXY", "not-a-valid-url!!!");

        // Should not panic.
        let client = build_http_client(30);
        drop(client);
    }

    /// An empty proxy env var is treated as "not set".
    #[test]
    fn empty_proxy_env_var_is_ignored() {
        let _guard = EnvGuard::set("HTTPS_PROXY", "");

        let client = build_http_client(30);
        drop(client);
    }

    /// configure_proxy can be used to compose additional builder options.
    #[test]
    fn configure_proxy_returns_builder_for_chaining() {
        let _guard = EnvGuard::clear(&[
            "HTTPS_PROXY", "https_proxy",
            "HTTP_PROXY",  "http_proxy",
            "ALL_PROXY",   "all_proxy",
            "NO_PROXY",    "no_proxy",
        ]);

        let client = configure_proxy(reqwest::Client::builder())
            .timeout(Duration::from_secs(60))
            .build()
            .expect("client should build");

        drop(client);
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    /// RAII guard that restores environment variables when dropped.
    ///
    /// This prevents individual tests from polluting the environment for other
    /// tests in the same process (cargo runs tests in parallel by default).
    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        /// Set a single environment variable, restoring its original value on drop.
        fn set(key: &str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // Safety: test-only, single-threaded env manipulation.
            unsafe { std::env::set_var(key, value) };
            EnvGuard {
                vars: vec![(key.to_string(), previous)],
            }
        }

        /// Clear multiple environment variables, restoring them on drop.
        fn clear(keys: &[&str]) -> Self {
            let vars: Vec<(String, Option<String>)> = keys
                .iter()
                .map(|k| {
                    let prev = std::env::var(k).ok();
                    // Safety: test-only.
                    unsafe { std::env::remove_var(k) };
                    (k.to_string(), prev)
                })
                .collect();
            EnvGuard { vars }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, maybe_val) in &self.vars {
                match maybe_val {
                    // Safety: test-only.
                    Some(v) => unsafe { std::env::set_var(key, v) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }
}
