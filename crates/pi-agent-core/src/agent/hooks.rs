use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

/// Every lifecycle point at which extensions can intercept agent behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    BeforeTurn,
    AfterTurn,
    BeforeCompact,
    AfterCompact,
    BeforeToolExecution,
    AfterToolExecution,
    SessionStart,
    SessionEnd,
    // UI-specific hooks for TUI integration
    /// Called before rendering the main UI frame
    BeforeRender,
    /// Called when a key event is received (allows interception)
    OnKeyEvent,
    /// Called when a slash command is executed
    OnSlashCommand,
    /// Called before showing a tool execution visualization
    BeforeToolVisualization,
    /// Called when the footer is rendered (allows adding custom status)
    OnFooterRender,
}

/// Data passed to every hook handler when an event fires.
pub struct HookContext {
    pub event: HookEvent,
    /// Arbitrary JSON payload associated with this event (e.g. tool name,
    /// turn index, compact stats). Extensions may inspect but must not mutate
    /// this struct -- mutations are expressed via `HookResult::Modified`.
    pub data: Value,
}

/// The return type a handler produces after inspecting a `HookContext`.
pub enum HookResult {
    /// Allow the agent to continue normally.
    Continue,
    /// Request that the operation triggering this event be cancelled.
    /// The agent loop is responsible for honouring cancellation semantics.
    Cancel,
    /// Supply a modified version of `HookContext::data` that the agent should
    /// use in place of the original.
    Modified(Value),
}

/// A type-erased, boxed hook function.
///
/// Handlers receive a shared reference to the context and return a `HookResult`.
/// They are required to be `Send + Sync` so they can be stored across await
/// points and called from any async task.
pub type HookHandler = Box<dyn Fn(&HookContext) -> HookResult + Send + Sync>;

/// Internal record stored in the registry for each registered hook.
struct HookEntry {
    event: HookEvent,
    extension_name: String,
    handler: HookHandler,
}

/// A thread-safe registry that maps `(HookEvent, extension_name)` pairs to
/// handler closures. Multiple handlers may be registered for the same event --
/// they are invoked in registration order when the event is dispatched.
pub struct HookRegistry {
    hooks: Arc<RwLock<Vec<HookEntry>>>,
}

impl HookRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a handler for `event` under the given `extension_name`.
    ///
    /// Multiple handlers can be registered for the same `(event, extension_name)`
    /// pair; they are all called in order of registration.
    pub async fn register(&self, event: HookEvent, extension_name: String, handler: HookHandler) {
        let mut guard = self.hooks.write().await;
        guard.push(HookEntry {
            event,
            extension_name,
            handler,
        });
    }

    /// Dispatch `ctx` to every handler registered for `ctx.event`.
    ///
    /// Handlers are called sequentially in registration order.  All results
    /// are collected and returned -- callers decide how to interpret a mix of
    /// `Continue`, `Cancel`, and `Modified` outcomes.
    pub async fn dispatch(&self, ctx: &HookContext) -> Vec<HookResult> {
        let guard = self.hooks.read().await;
        let mut results = Vec::new();
        for entry in guard.iter() {
            if entry.event == ctx.event {
                let result = (entry.handler)(ctx);
                results.push(result);
            }
        }
        results
    }

    /// Remove every handler that was registered under `extension_name`.
    ///
    /// This should be called when an extension is unloaded so that stale
    /// handlers are not invoked for future events.
    pub async fn unregister_all(&self, extension_name: &str) {
        let mut guard = self.hooks.write().await;
        guard.retain(|entry| entry.extension_name != extension_name);
    }

    /// Returns `true` if there are any handlers registered for the given event.
    pub async fn has_handlers(&self, event: HookEvent) -> bool {
        let guard = self.hooks.read().await;
        guard.iter().any(|entry| entry.event == event)
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper: interpret a list of HookResults into an aggregate decision.
// ---------------------------------------------------------------------------

/// Aggregate outcome after dispatching a hook event to all registered handlers.
pub enum HookOutcome {
    /// No handlers returned Cancel or Modified; proceed normally.
    Continue,
    /// At least one handler returned Cancel; the operation should be skipped.
    Cancelled,
    /// At least one handler returned Modified; the last `Modified` payload wins.
    Modified(Value),
}

/// Reduce a `Vec<HookResult>` into a single `HookOutcome`.
///
/// Semantics:
/// - If *any* handler returns `Cancel`, the outcome is `Cancelled` (Cancel
///   takes precedence over everything).
/// - Otherwise, if *any* handler returns `Modified`, the outcome is
///   `Modified` with the payload from the **last** such handler (later
///   handlers override earlier ones).
/// - Otherwise the outcome is `Continue`.
pub fn resolve_hook_results(results: Vec<HookResult>) -> HookOutcome {
    let mut last_modified: Option<Value> = None;
    for result in results {
        match result {
            HookResult::Cancel => return HookOutcome::Cancelled,
            HookResult::Modified(v) => {
                last_modified = Some(v);
            }
            HookResult::Continue => {}
        }
    }
    match last_modified {
        Some(v) => HookOutcome::Modified(v),
        None => HookOutcome::Continue,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Helper: build a context for a given event with optional data.
    fn ctx(event: HookEvent, data: Value) -> HookContext {
        HookContext { event, data }
    }

    // ---------------------------------------------------------------------------
    // Test 1: register a single handler and verify dispatch calls it.
    // ---------------------------------------------------------------------------
    #[tokio::test]
    async fn dispatch_calls_registered_handler() {
        let registry = HookRegistry::new();

        // Register a handler that always returns Continue.
        registry
            .register(
                HookEvent::BeforeTurn,
                "test-ext".to_string(),
                Box::new(|_ctx| HookResult::Continue),
            )
            .await;

        let results = registry
            .dispatch(&ctx(HookEvent::BeforeTurn, json!(null)))
            .await;

        assert_eq!(results.len(), 1, "expected exactly one result");
        assert!(
            matches!(results[0], HookResult::Continue),
            "expected Continue from the handler"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 2: multiple handlers registered for the same event are all called
    // in registration order, and handlers for other events are NOT called.
    // ---------------------------------------------------------------------------
    #[tokio::test]
    async fn dispatch_calls_multiple_handlers_in_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;

        let registry = HookRegistry::new();
        let call_order: StdArc<std::sync::Mutex<Vec<u32>>> =
            StdArc::new(std::sync::Mutex::new(Vec::new()));

        // First handler -- push 1.
        let order1 = StdArc::clone(&call_order);
        registry
            .register(
                HookEvent::AfterTurn,
                "ext-a".to_string(),
                Box::new(move |_| {
                    order1.lock().unwrap().push(1);
                    HookResult::Continue
                }),
            )
            .await;

        // Second handler -- push 2.
        let order2 = StdArc::clone(&call_order);
        registry
            .register(
                HookEvent::AfterTurn,
                "ext-b".to_string(),
                Box::new(move |_| {
                    order2.lock().unwrap().push(2);
                    HookResult::Continue
                }),
            )
            .await;

        // Handler for a DIFFERENT event -- must NOT be called.
        let counter = StdArc::new(AtomicUsize::new(0));
        let counter_clone = StdArc::clone(&counter);
        registry
            .register(
                HookEvent::SessionStart,
                "ext-c".to_string(),
                Box::new(move |_| {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    HookResult::Continue
                }),
            )
            .await;

        let results = registry
            .dispatch(&ctx(HookEvent::AfterTurn, json!({})))
            .await;

        // Only the two AfterTurn handlers should have fired.
        assert_eq!(results.len(), 2);
        assert_eq!(*call_order.lock().unwrap(), vec![1, 2]);
        // SessionStart handler must not have been invoked.
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    // ---------------------------------------------------------------------------
    // Test 3: a handler returning Cancel is surfaced correctly in results.
    // ---------------------------------------------------------------------------
    #[tokio::test]
    async fn dispatch_surfaces_cancel_result() {
        let registry = HookRegistry::new();

        // First handler: Continue.
        registry
            .register(
                HookEvent::BeforeToolExecution,
                "guard-ext".to_string(),
                Box::new(|_| HookResult::Continue),
            )
            .await;

        // Second handler: Cancel -- simulates a guard refusing the action.
        registry
            .register(
                HookEvent::BeforeToolExecution,
                "guard-ext".to_string(),
                Box::new(|ctx| {
                    // Cancel only when the tool name matches "dangerous_tool".
                    if ctx.data.get("tool").and_then(Value::as_str).unwrap_or("")
                        == "dangerous_tool"
                    {
                        HookResult::Cancel
                    } else {
                        HookResult::Continue
                    }
                }),
            )
            .await;

        let results = registry
            .dispatch(&ctx(
                HookEvent::BeforeToolExecution,
                json!({ "tool": "dangerous_tool" }),
            ))
            .await;

        assert_eq!(results.len(), 2);
        assert!(matches!(results[0], HookResult::Continue));
        assert!(matches!(results[1], HookResult::Cancel));
    }

    // ---------------------------------------------------------------------------
    // Test 4: unregister_all removes all hooks belonging to one extension
    // without affecting hooks from other extensions.
    // ---------------------------------------------------------------------------
    #[tokio::test]
    async fn unregister_all_removes_only_target_extension() {
        let registry = HookRegistry::new();

        // Two handlers from "ext-to-remove".
        registry
            .register(
                HookEvent::SessionStart,
                "ext-to-remove".to_string(),
                Box::new(|_| HookResult::Continue),
            )
            .await;
        registry
            .register(
                HookEvent::SessionEnd,
                "ext-to-remove".to_string(),
                Box::new(|_| HookResult::Continue),
            )
            .await;

        // One handler that must survive.
        registry
            .register(
                HookEvent::SessionStart,
                "ext-to-keep".to_string(),
                Box::new(|_| HookResult::Modified(json!({ "kept": true }))),
            )
            .await;

        // Remove everything from "ext-to-remove".
        registry.unregister_all("ext-to-remove").await;

        // SessionStart now has only the surviving handler.
        let start_results = registry
            .dispatch(&ctx(HookEvent::SessionStart, json!(null)))
            .await;
        assert_eq!(
            start_results.len(),
            1,
            "only ext-to-keep's handler survives"
        );
        assert!(matches!(start_results[0], HookResult::Modified(_)));

        // SessionEnd has no handlers left.
        let end_results = registry
            .dispatch(&ctx(HookEvent::SessionEnd, json!(null)))
            .await;
        assert!(
            end_results.is_empty(),
            "ext-to-remove's SessionEnd handler must be gone"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 5: Modified result carries the new data value through intact.
    // ---------------------------------------------------------------------------
    #[tokio::test]
    async fn modified_result_preserves_payload() {
        let registry = HookRegistry::new();

        let new_payload = json!({ "rewritten": true, "count": 42 });
        let expected = new_payload.clone();

        registry
            .register(
                HookEvent::BeforeCompact,
                "rewrite-ext".to_string(),
                Box::new(move |_| HookResult::Modified(new_payload.clone())),
            )
            .await;

        let results = registry
            .dispatch(&ctx(HookEvent::BeforeCompact, json!({ "original": true })))
            .await;

        assert_eq!(results.len(), 1);
        if let HookResult::Modified(val) = &results[0] {
            assert_eq!(*val, expected);
        } else {
            panic!("expected Modified result");
        }
    }

    // ---------------------------------------------------------------------------
    // Test 6: resolve_hook_results helper
    // ---------------------------------------------------------------------------
    #[test]
    fn resolve_empty_results_gives_continue() {
        let outcome = resolve_hook_results(vec![]);
        assert!(matches!(outcome, HookOutcome::Continue));
    }

    #[test]
    fn resolve_cancel_takes_precedence() {
        let outcome = resolve_hook_results(vec![
            HookResult::Modified(json!("ignored")),
            HookResult::Cancel,
        ]);
        assert!(matches!(outcome, HookOutcome::Cancelled));
    }

    #[test]
    fn resolve_last_modified_wins() {
        let outcome = resolve_hook_results(vec![
            HookResult::Modified(json!(1)),
            HookResult::Continue,
            HookResult::Modified(json!(2)),
        ]);
        match outcome {
            HookOutcome::Modified(v) => assert_eq!(v, json!(2)),
            other => panic!(
                "expected Modified, got {:?}",
                match other {
                    HookOutcome::Continue => "Continue",
                    HookOutcome::Cancelled => "Cancelled",
                    _ => "unknown",
                }
            ),
        }
    }

    // ---------------------------------------------------------------------------
    // Test 7: has_handlers returns correct value
    // ---------------------------------------------------------------------------
    #[tokio::test]
    async fn has_handlers_reports_correctly() {
        let registry = HookRegistry::new();
        assert!(!registry.has_handlers(HookEvent::BeforeTurn).await);

        registry
            .register(
                HookEvent::BeforeTurn,
                "test".to_string(),
                Box::new(|_| HookResult::Continue),
            )
            .await;

        assert!(registry.has_handlers(HookEvent::BeforeTurn).await);
        assert!(!registry.has_handlers(HookEvent::AfterTurn).await);
    }
}
