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
    configure_proxy(reqwest::Client::builder().timeout(Duration::from_secs(timeout_secs)))
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
    let no_proxy = env_var_nonempty(&["NO_PROXY", "no_proxy"]);
    let no_proxy_filter = no_proxy.as_deref().map(reqwest::NoProxy::from_string);

    if let Some(ref val) = no_proxy {
        debug!(no_proxy = %val, "NO_PROXY configured");
    }

    // ── HTTPS proxy ──────────────────────────────────────────────────────────
    if let Some(url) = env_var_nonempty(&["HTTPS_PROXY", "https_proxy"]) {
        match reqwest::Proxy::https(&url) {
            Ok(proxy) => {
                debug!(proxy_url = %url, "HTTPS proxy configured");
                let proxy = if let Some(ref np) = no_proxy_filter {
                    proxy.no_proxy(np.clone())
                } else {
                    proxy
                };
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
                let proxy = if let Some(ref np) = no_proxy_filter {
                    proxy.no_proxy(np.clone())
                } else {
                    proxy
                };
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
                let proxy = if let Some(ref np) = no_proxy_filter {
                    proxy.no_proxy(np.clone())
                } else {
                    proxy
                };
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                tracing::warn!(proxy_url = %url, error = %e, "invalid ALL_PROXY URL, ignoring");
            }
        }
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Global mutex to serialise proxy tests.  Environment variables are
    /// process-global, so concurrent test threads that modify them would race.
    /// All tests in this module lock `ENV_MUTEX` before touching env vars.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// A plain client (no proxy env vars set) must build successfully.
    /// This exercises the TLS initialisation path.
    #[test]
    fn client_builds_without_proxy_env() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::clear(&[
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ]);

        let client = build_http_client(30);
        drop(client);
    }

    /// When `HTTPS_PROXY` is set to a valid URL the builder should not panic.
    #[test]
    fn client_builds_with_https_proxy() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::set("HTTPS_PROXY", "http://proxy.example.com:3128");

        let client = build_http_client(30);
        drop(client);
    }

    /// When `HTTP_PROXY` is set to a valid URL the builder should not panic.
    #[test]
    fn client_builds_with_http_proxy() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::set("HTTP_PROXY", "http://proxy.example.com:8080");

        let client = build_http_client(30);
        drop(client);
    }

    /// When `ALL_PROXY` is set to a valid URL the builder should not panic.
    #[test]
    fn client_builds_with_all_proxy() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::set("ALL_PROXY", "http://proxy.example.com:1080");

        let client = build_http_client(30);
        drop(client);
    }

    /// An invalid proxy URL should not panic — the warning path is taken
    /// instead and the client still builds (without a proxy).
    #[test]
    fn client_builds_with_invalid_proxy_url() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::set("HTTPS_PROXY", "not-a-valid-url!!!");

        let client = build_http_client(30);
        drop(client);
    }

    /// An empty proxy env var is treated as "not set".
    #[test]
    fn empty_proxy_env_var_is_ignored() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::set("HTTPS_PROXY", "");

        let client = build_http_client(30);
        drop(client);
    }

    /// configure_proxy can be used to compose additional builder options.
    #[test]
    fn configure_proxy_returns_builder_for_chaining() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::clear(&[
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
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
    /// **Must** be used together with `ENV_MUTEX` to prevent races between
    /// tests that run on different threads in the same process.
    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        /// Set a single environment variable, restoring its original value on drop.
        fn set(key: &str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // Safety: caller holds ENV_MUTEX, so no other test thread is
            // reading/writing env vars concurrently.
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
                    // Safety: caller holds ENV_MUTEX.
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
                    // Safety: caller holds ENV_MUTEX (guard dropped before mutex).
                    Some(v) => unsafe { std::env::set_var(key, v) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }
}
