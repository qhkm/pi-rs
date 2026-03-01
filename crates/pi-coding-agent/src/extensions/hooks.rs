//! Hook system for agent lifecycle events.
//!
//! The canonical definitions now live in `pi_agent_core::agent::hooks`.
//! This module re-exports everything so that existing code within
//! `pi-coding-agent` continues to compile without changes.

pub use pi_agent_core::{
    HookContext, HookEvent, HookHandler, HookOutcome, HookRegistry, HookResult,
    resolve_hook_results,
};

// ---------------------------------------------------------------------------
// Tests -- validate that the re-exports work and the registry behaves as
// expected when accessed through this crate.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn ctx(event: HookEvent, data: Value) -> HookContext {
        HookContext { event, data }
    }

    #[tokio::test]
    async fn reexported_registry_works() {
        let registry = HookRegistry::new();

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

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], HookResult::Continue));
    }

    #[tokio::test]
    async fn cancel_result_surfaces() {
        let registry = HookRegistry::new();

        registry
            .register(
                HookEvent::BeforeToolExecution,
                "guard".to_string(),
                Box::new(|_| HookResult::Cancel),
            )
            .await;

        let results = registry
            .dispatch(&ctx(
                HookEvent::BeforeToolExecution,
                json!({ "tool": "dangerous" }),
            ))
            .await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], HookResult::Cancel));
    }

    #[tokio::test]
    async fn resolve_hook_results_cancel_takes_precedence() {
        let results = vec![
            HookResult::Modified(json!("ignored")),
            HookResult::Cancel,
        ];
        let outcome = resolve_hook_results(results);
        assert!(matches!(outcome, HookOutcome::Cancelled));
    }

    #[tokio::test]
    async fn unregister_all_removes_target() {
        let registry = HookRegistry::new();

        registry
            .register(
                HookEvent::SessionStart,
                "remove-me".to_string(),
                Box::new(|_| HookResult::Continue),
            )
            .await;

        registry
            .register(
                HookEvent::SessionStart,
                "keep-me".to_string(),
                Box::new(|_| HookResult::Modified(json!({ "kept": true }))),
            )
            .await;

        registry.unregister_all("remove-me").await;

        let results = registry
            .dispatch(&ctx(HookEvent::SessionStart, json!(null)))
            .await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], HookResult::Modified(_)));
    }
}
