/// Retry decorator for any `LLMProvider`.
///
/// Wraps an inner provider and transparently retries retryable failures
/// (connection errors, rate limits, transient server errors) with exponential
/// backoff and optional jitter.  Non-retryable errors (authentication failures,
/// validation errors, unsupported features) are surfaced immediately without
/// any retry overhead.
///
/// # Example
///
/// ```rust,no_run
/// use pi_ai::providers::{AnthropicProvider, LLMProvider};
/// use pi_ai::providers::retry::{RetryConfig, RetryProvider};
/// use std::time::Duration;
///
/// let inner: Box<dyn LLMProvider> = Box::new(AnthropicProvider::new("sk-ant-key", None));
/// let provider = RetryProvider::new(inner, RetryConfig::default());
/// ```
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::{PiAiError, Result};
use crate::messages::types::AssistantMessage;
use crate::models::registry::Model;
use crate::streaming::events::StreamEvent;

use super::traits::{Context, LLMProvider, ProviderCapabilities, StreamOptions};

// ─── RetryConfig ──────────────────────────────────────────────────────────────

/// Configuration for the exponential-backoff retry strategy.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts after the initial call fails.
    /// A value of `3` means up to 4 total attempts (1 initial + 3 retries).
    pub max_retries: u32,
    /// Base delay before the first retry.  Subsequent retries double this
    /// value (capped at `max_delay`).
    pub base_delay: Duration,
    /// Hard upper bound on the computed backoff delay.
    pub max_delay: Duration,
    /// When `true`, a uniformly-random fraction of `base_delay` is added to
    /// each computed delay to avoid "thundering herd" patterns when many
    /// clients retry simultaneously.
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// Build a `RetryConfig` from a `StreamOptions` override, if present.
    ///
    /// When the caller sets `StreamOptions::max_retry_delay_ms`, that value
    /// overrides `max_delay`; all other fields keep their defaults.
    pub fn from_stream_options(options: &StreamOptions) -> Self {
        let mut cfg = Self::default();
        if let Some(ms) = options.max_retry_delay_ms {
            cfg.max_delay = Duration::from_millis(ms);
        }
        cfg
    }

    /// Compute the sleep duration for a given attempt index (0-based).
    ///
    /// Formula: `min(base_delay * 2^attempt, max_delay)` + optional jitter.
    pub fn backoff_delay(&self, attempt: u32) -> Duration {
        // 2^attempt — cap the exponent at 62 to avoid overflowing u64.
        let shift = attempt.min(62) as u64;
        let multiplier: u64 = 1u64 << shift;
        let base_ms = self.base_delay.as_millis() as u64;
        let computed_ms = base_ms.saturating_mul(multiplier);
        let capped_ms = computed_ms.min(self.max_delay.as_millis() as u64);

        let jitter_ms = if self.jitter {
            // Derive pseudo-random jitter from sub-second system time so we
            // don't require an external RNG dependency.
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as u64;
            // Linear-congruential mix.
            let mixed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            mixed % base_ms.max(1)
        } else {
            0
        };

        let max_ms = self.max_delay.as_millis() as u64;
        Duration::from_millis(capped_ms.saturating_add(jitter_ms).min(max_ms))
    }
}

// ─── Retryability classification ──────────────────────────────────────────────

/// Returns `true` when `error` represents a transient failure that is safe to
/// retry.
///
/// | Error variant            | Retryable? | Rationale                                      |
/// |--------------------------|------------|------------------------------------------------|
/// | `RateLimited`            | yes        | Provider asked us to back off; retry after wait |
/// | `Http` (connection)      | yes        | Network blip; server not yet reached           |
/// | `Http` (5xx / 429)       | yes        | Server-side transient error                    |
/// | `StreamClosed`           | yes        | Upstream closed the stream early               |
/// | `Provider` (generic)     | yes        | Assume transient unless proven otherwise       |
/// | `Io`                     | yes        | Transient I/O issue                            |
/// | `Auth`                   | no         | Bad credentials; retry won't help              |
/// | `Config`                 | no         | Caller bug; retry won't help                   |
/// | `ModelNotFound`          | no         | Wrong model ID; retry won't help               |
/// | `Unsupported`            | no         | Feature not available; retry won't help        |
/// | `Aborted`                | no         | Caller-initiated abort                         |
/// | `Json`                   | no         | Protocol mismatch; retry won't help            |
pub fn is_retryable(error: &PiAiError) -> bool {
    match error {
        PiAiError::RateLimited { .. } => true,
        PiAiError::StreamClosed => true,
        PiAiError::Io(_) => true,
        // Provider-level generic error: assume transient.
        PiAiError::Provider { .. } => true,

        // reqwest HTTP errors: retry on connection/timeout failures and 5xx/429.
        PiAiError::Http(e) => {
            if e.is_connect() || e.is_timeout() || e.is_request() {
                return true;
            }
            if let Some(status) = e.status() {
                let code = status.as_u16();
                return code == 429 || (500..=599).contains(&code);
            }
            false
        }

        // Definitively non-retryable.
        PiAiError::Auth(_) => false,
        PiAiError::Config(_) => false,
        PiAiError::ModelNotFound(_) => false,
        PiAiError::Unsupported(_) => false,
        PiAiError::Aborted => false,
        PiAiError::Json(_) => false,
    }
}

/// Extract a server-suggested retry-after delay from a `RateLimited` error.
fn rate_limit_delay(error: &PiAiError) -> Option<Duration> {
    if let PiAiError::RateLimited { retry_after_ms } = error {
        Some(Duration::from_millis(*retry_after_ms))
    } else {
        None
    }
}

// ─── RetryProvider ────────────────────────────────────────────────────────────

/// An `LLMProvider` wrapper that adds automatic retry with exponential backoff.
///
/// All `LLMProvider` methods are delegated to the inner provider.  Only
/// `stream` and `complete` participate in the retry loop; `name` and
/// `capabilities` are always forwarded immediately.
pub struct RetryProvider {
    inner: Box<dyn LLMProvider>,
    config: RetryConfig,
}

impl RetryProvider {
    /// Wrap `inner` with the given retry `config`.
    pub fn new(inner: Box<dyn LLMProvider>, config: RetryConfig) -> Self {
        Self { inner, config }
    }

    /// Wrap `inner` with default retry settings.
    pub fn with_defaults(inner: Box<dyn LLMProvider>) -> Self {
        Self::new(inner, RetryConfig::default())
    }
}

impl std::fmt::Debug for RetryProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryProvider")
            .field("inner", &self.inner.name())
            .field("config", &self.config)
            .finish()
    }
}

// ─── LLMProvider implementation ───────────────────────────────────────────────

#[async_trait]
impl LLMProvider for RetryProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.inner.capabilities()
    }

    /// Stream with automatic retry on transient failures.
    ///
    /// On a retryable error the method:
    /// 1. Waits for the computed backoff delay (respecting `Retry-After` hints
    ///    from `RateLimited` errors).
    /// 2. Calls the inner provider again with a fresh channel.
    /// 3. Repeats until success or `max_retries` is exhausted, at which point
    ///    the last error is returned to the caller.
    ///
    /// Non-retryable errors are returned immediately on first occurrence.
    ///
    /// # Channel behaviour
    ///
    /// On each attempt the method creates an internal channel, drives the inner
    /// provider's stream, and forwards every received event to the caller's `tx`.
    /// If the caller drops `tx` between events (i.e., `tx.send` fails), the
    /// method returns `StreamClosed` immediately without further retries.
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let max_retries = self.config.max_retries;

        for attempt in 0..=max_retries {
            let (inner_tx, mut inner_rx) = mpsc::channel::<StreamEvent>(256);

            // Drive the inner provider and drain its channel concurrently so
            // the bounded channel never causes a deadlock.
            let (stream_result, forward_error) = tokio::join!(
                self.inner.stream(model, context, options, inner_tx),
                async {
                    let mut err: Option<()> = None;
                    while let Some(event) = inner_rx.recv().await {
                        if tx.send(event).await.is_err() {
                            err = Some(());
                            break;
                        }
                    }
                    err
                }
            );

            // Caller dropped their receiver — no point retrying.
            if forward_error.is_some() {
                return Err(PiAiError::StreamClosed);
            }

            match stream_result {
                Ok(()) => return Ok(()),
                Err(err) => {
                    let retryable = is_retryable(&err);
                    let exhausted = attempt == max_retries;

                    if !retryable || exhausted {
                        if retryable && exhausted {
                            warn!(
                                provider = self.inner.name(),
                                attempts = attempt + 1,
                                error = %err,
                                "RetryProvider: max retries exhausted"
                            );
                        }
                        return Err(err);
                    }

                    // Compute delay, honouring any Retry-After hint.
                    let delay = rate_limit_delay(&err)
                        .map(|hint| hint.min(self.config.max_delay))
                        .unwrap_or_else(|| self.config.backoff_delay(attempt));

                    warn!(
                        provider = self.inner.name(),
                        attempt = attempt + 1,
                        max_retries,
                        delay_ms = delay.as_millis(),
                        error = %err,
                        "RetryProvider: retryable error — backing off before retry"
                    );

                    debug!(
                        provider = self.inner.name(),
                        attempt,
                        "RetryProvider: sleeping before next attempt"
                    );

                    tokio::time::sleep(delay).await;
                }
            }
        }

        // Unreachable: every iteration either returns or continues.
        unreachable!("retry loop exited without returning")
    }

    /// Non-streaming completion with retry.
    ///
    /// Applies the same backoff strategy as `stream` but delegates directly to
    /// the inner provider's `complete` method.
    async fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> Result<AssistantMessage> {
        let max_retries = self.config.max_retries;

        for attempt in 0..=max_retries {
            match self.inner.complete(model, context, options).await {
                Ok(msg) => return Ok(msg),
                Err(err) => {
                    let retryable = is_retryable(&err);
                    let exhausted = attempt == max_retries;

                    if !retryable || exhausted {
                        if retryable && exhausted {
                            warn!(
                                provider = self.inner.name(),
                                attempts = attempt + 1,
                                error = %err,
                                "RetryProvider::complete: max retries exhausted"
                            );
                        }
                        return Err(err);
                    }

                    let delay = rate_limit_delay(&err)
                        .map(|hint| hint.min(self.config.max_delay))
                        .unwrap_or_else(|| self.config.backoff_delay(attempt));

                    warn!(
                        provider = self.inner.name(),
                        attempt = attempt + 1,
                        max_retries,
                        delay_ms = delay.as_millis(),
                        error = %err,
                        "RetryProvider::complete: retryable error — backing off"
                    );

                    tokio::time::sleep(delay).await;
                }
            }
        }

        unreachable!("retry loop exited without returning")
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use crate::error::{PiAiError, Result};
    use crate::messages::types::{Api, AssistantMessage, Content, Provider, StopReason, Usage};
    use crate::models::registry::{Model, ModelCost};
    use crate::providers::traits::{Context, LLMProvider, ProviderCapabilities, StreamOptions};
    use crate::streaming::events::StreamEvent;

    use super::{is_retryable, RetryConfig, RetryProvider};

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn dummy_model() -> Model {
        Model {
            id: "test-model".into(),
            name: "Test Model".into(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenAI,
            base_url: "https://api.example.com".into(),
            reasoning: false,
            input_types: vec![],
            cost: ModelCost::default(),
            context_window: 16_384,
            max_tokens: 4_096,
            headers: None,
        }
    }

    fn dummy_message(model: &Model) -> AssistantMessage {
        AssistantMessage {
            content: vec![Content::text("ok")],
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    // ── Stub providers ────────────────────────────────────────────────────────

    /// Fails for the first `fail_times` calls, then succeeds.
    struct FlakyProvider {
        error_fn: fn() -> PiAiError,
        fail_times: u32,
        call_count: Arc<AtomicU32>,
    }

    impl FlakyProvider {
        fn new(error_fn: fn() -> PiAiError, fail_times: u32) -> (Self, Arc<AtomicU32>) {
            let call_count = Arc::new(AtomicU32::new(0));
            let p = Self {
                error_fn,
                fail_times,
                call_count: Arc::clone(&call_count),
            };
            (p, call_count)
        }
    }

    #[async_trait]
    impl LLMProvider for FlakyProvider {
        fn name(&self) -> &str {
            "flaky-test"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        async fn stream(
            &self,
            model: &Model,
            _context: &Context,
            _options: &StreamOptions,
            tx: mpsc::Sender<StreamEvent>,
        ) -> Result<()> {
            let call = self.call_count.fetch_add(1, Ordering::SeqCst);
            if call < self.fail_times {
                return Err((self.error_fn)());
            }
            let _ = tx
                .send(StreamEvent::Done {
                    reason: StopReason::Stop,
                    message: dummy_message(model),
                })
                .await;
            Ok(())
        }
    }

    /// Always returns the given error.
    struct AlwaysFailProvider {
        error_fn: fn() -> PiAiError,
        call_count: Arc<AtomicU32>,
    }

    impl AlwaysFailProvider {
        fn new(error_fn: fn() -> PiAiError) -> (Self, Arc<AtomicU32>) {
            let call_count = Arc::new(AtomicU32::new(0));
            let p = Self {
                error_fn,
                call_count: Arc::clone(&call_count),
            };
            (p, call_count)
        }
    }

    #[async_trait]
    impl LLMProvider for AlwaysFailProvider {
        fn name(&self) -> &str {
            "always-fail-test"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        async fn stream(
            &self,
            _model: &Model,
            _context: &Context,
            _options: &StreamOptions,
            _tx: mpsc::Sender<StreamEvent>,
        ) -> Result<()> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err((self.error_fn)())
        }
    }

    // ── is_retryable unit tests ────────────────────────────────────────────────

    #[test]
    fn rate_limited_is_retryable() {
        assert!(is_retryable(&PiAiError::RateLimited {
            retry_after_ms: 1000
        }));
    }

    #[test]
    fn stream_closed_is_retryable() {
        assert!(is_retryable(&PiAiError::StreamClosed));
    }

    #[test]
    fn provider_error_is_retryable() {
        assert!(is_retryable(&PiAiError::Provider {
            provider: "openai".into(),
            message: "internal error".into(),
        }));
    }

    #[test]
    fn auth_error_is_not_retryable() {
        assert!(!is_retryable(&PiAiError::Auth("bad api key".into())));
    }

    #[test]
    fn config_error_is_not_retryable() {
        assert!(!is_retryable(&PiAiError::Config("missing field".into())));
    }

    #[test]
    fn model_not_found_is_not_retryable() {
        assert!(!is_retryable(&PiAiError::ModelNotFound("gpt-99".into())));
    }

    #[test]
    fn unsupported_is_not_retryable() {
        assert!(!is_retryable(&PiAiError::Unsupported("vision".into())));
    }

    #[test]
    fn aborted_is_not_retryable() {
        assert!(!is_retryable(&PiAiError::Aborted));
    }

    // ── RetryConfig backoff tests ──────────────────────────────────────────────

    #[test]
    fn backoff_doubles_each_attempt() {
        let cfg = RetryConfig {
            max_retries: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(60),
            jitter: false,
        };
        assert_eq!(cfg.backoff_delay(0), Duration::from_millis(100));
        assert_eq!(cfg.backoff_delay(1), Duration::from_millis(200));
        assert_eq!(cfg.backoff_delay(2), Duration::from_millis(400));
        assert_eq!(cfg.backoff_delay(3), Duration::from_millis(800));
    }

    #[test]
    fn backoff_capped_at_max_delay() {
        let cfg = RetryConfig {
            max_retries: 10,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_millis(1_000),
            jitter: false,
        };
        // 500 * 2^10 = 512_000ms, capped at 1_000ms.
        let delay = cfg.backoff_delay(10);
        assert!(
            delay <= Duration::from_millis(1_000),
            "delay {:?} exceeds max_delay",
            delay
        );
    }

    // ── Integration tests ──────────────────────────────────────────────────────

    /// Test 1 — Retries on a transient server error and eventually succeeds.
    #[tokio::test]
    async fn retries_on_server_error_then_succeeds() {
        let (inner, call_count) = FlakyProvider::new(|| PiAiError::StreamClosed, 2);

        let config = RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            jitter: false,
        };
        let provider = RetryProvider::new(Box::new(inner), config);

        let model = dummy_model();
        let ctx = Context::default();
        let opts = StreamOptions::default();
        let (tx, mut rx) = mpsc::channel(64);

        let result = provider.stream(&model, &ctx, &opts, tx).await;

        assert!(result.is_ok(), "expected success after retries: {:?}", result);
        // 2 failures + 1 success = 3 total calls.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            3,
            "expected exactly 3 calls"
        );

        let event = rx.recv().await.expect("expected Done event on caller rx");
        assert!(matches!(event, StreamEvent::Done { .. }));
    }

    /// Test 2 — Does NOT retry on an auth error; fails immediately.
    #[tokio::test]
    async fn no_retry_on_auth_error() {
        let (inner, call_count) =
            AlwaysFailProvider::new(|| PiAiError::Auth("invalid key".into()));

        let config = RetryConfig {
            max_retries: 5,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            jitter: false,
        };
        let provider = RetryProvider::new(Box::new(inner), config);

        let model = dummy_model();
        let ctx = Context::default();
        let opts = StreamOptions::default();
        let (tx, _rx) = mpsc::channel(64);

        let result = provider.stream(&model, &ctx, &opts, tx).await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), PiAiError::Auth(_)),
            "expected Auth error"
        );
        // Must have been called exactly once — zero retries.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "expected exactly 1 call (no retries)"
        );
    }

    /// Test 3 — Gives up after max_retries and surfaces the error.
    #[tokio::test]
    async fn max_retries_exceeded_returns_error() {
        let (inner, call_count) = AlwaysFailProvider::new(|| PiAiError::StreamClosed);

        let config = RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            jitter: false,
        };
        let provider = RetryProvider::new(Box::new(inner), config);

        let model = dummy_model();
        let ctx = Context::default();
        let opts = StreamOptions::default();
        let (tx, _rx) = mpsc::channel(64);

        let result = provider.stream(&model, &ctx, &opts, tx).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PiAiError::StreamClosed));
        // max_retries = 3 → 4 total attempts (initial + 3 retries).
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            4,
            "expected 4 total attempts"
        );
    }

    /// Test 4 — Retries on a RateLimited error and respects the hint delay.
    #[tokio::test]
    async fn retries_on_rate_limit() {
        let (inner, call_count) =
            FlakyProvider::new(|| PiAiError::RateLimited { retry_after_ms: 1 }, 1);

        let config = RetryConfig {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: false,
        };
        let provider = RetryProvider::new(Box::new(inner), config);

        let model = dummy_model();
        let ctx = Context::default();
        let opts = StreamOptions::default();
        let (tx, mut rx) = mpsc::channel(64);

        let result = provider.stream(&model, &ctx, &opts, tx).await;

        assert!(result.is_ok(), "expected success after rate-limit retry: {:?}", result);
        assert_eq!(call_count.load(Ordering::SeqCst), 2);

        let event = rx.recv().await.expect("expected Done event");
        assert!(matches!(event, StreamEvent::Done { .. }));
    }

    /// Test 5 — `name()` and `capabilities()` delegate to the inner provider.
    #[test]
    fn delegates_name_and_capabilities() {
        let (inner, _) = AlwaysFailProvider::new(|| PiAiError::Aborted);
        let provider = RetryProvider::with_defaults(Box::new(inner));
        assert_eq!(provider.name(), "always-fail-test");
        let _ = provider.capabilities(); // must not panic
    }

    /// Test 6 — `RetryConfig::from_stream_options` picks up max_retry_delay_ms.
    #[test]
    fn retry_config_from_stream_options() {
        let mut opts = StreamOptions::default();
        opts.max_retry_delay_ms = Some(5_000);
        let cfg = RetryConfig::from_stream_options(&opts);
        assert_eq!(cfg.max_delay, Duration::from_millis(5_000));
        assert_eq!(cfg.max_retries, 3); // default unchanged
    }
}
