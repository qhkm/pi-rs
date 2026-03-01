//! Integration tests for multi-step session workflows.
//!
//! These tests verify end-to-end behaviour of session management: branching,
//! merging, exporting, and stress scenarios that span multiple subsystems.

use pi_agent_core::messages::AgentMessage;
use pi_ai::Message;
use pi_coding_agent::export::html::export_to_html;
use pi_coding_agent::session::SessionManager;
use std::path::PathBuf;
use uuid::Uuid;

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pi-rs-int-{name}-{}", Uuid::new_v4()))
}

// ---------------------------------------------------------------------------
// Test 1: branch → merge → export workflow
// ---------------------------------------------------------------------------
#[tokio::test]
async fn branch_merge_export_workflow() {
    let dir = temp_dir("branch-merge-export");

    // --- Create primary session with some messages ---
    let mut primary = SessionManager::new(dir.clone());
    primary
        .create_session("/tmp/work")
        .await
        .expect("create primary");

    let _id_root = primary
        .append_message(AgentMessage::from_llm(Message::user("Hello from main")))
        .await
        .expect("root message");

    let _id_main2 = primary
        .append_message(AgentMessage::from_llm(Message::user("Second main message")))
        .await
        .expect("main message 2");

    // --- Create a separate "branch" session that simulates forked work ---
    let mut branch = SessionManager::new(dir.clone());
    branch
        .create_session("/tmp/work")
        .await
        .expect("create branch");

    branch
        .append_message(AgentMessage::from_llm(Message::user("Branch message 1")))
        .await
        .expect("branch msg 1");

    branch
        .append_message(AgentMessage::from_llm(Message::user("Branch message 2")))
        .await
        .expect("branch msg 2");

    branch
        .append_message(AgentMessage::from_llm(Message::user("Branch message 3")))
        .await
        .expect("branch msg 3");

    let branch_path = branch.session_path().expect("branch path").to_path_buf();

    // --- Merge branch into primary ---
    let merged_count = primary.merge(&branch_path).await.expect("merge");
    assert_eq!(merged_count, 3, "should merge 3 branch entries");

    // --- Verify tree integrity after merge ---
    let tree = primary.get_tree().await.expect("get_tree");

    // primary had 2 + branch had 3 → 5 total entries
    assert_eq!(tree.len(), 5, "merged tree should have 5 nodes");

    // Verify all IDs are unique
    let ids: Vec<&str> = tree.iter().map(|n| n.entry_id.as_str()).collect();
    let unique_ids: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique_ids.len(), "all IDs must be unique");

    // --- Export to HTML ---
    // Drop primary to release session lock before re-loading for export
    let primary_path = primary.session_path().expect("primary path").to_path_buf();
    drop(primary);

    let mut loader = SessionManager::new(dir.clone());
    let messages = loader
        .load_session(&primary_path)
        .await
        .expect("load messages");

    let export_path = dir.join("export.html");
    let result_path = export_to_html(&messages, Some(export_path.as_path()), Some("Test Export"))
        .await
        .expect("export_to_html");

    assert!(result_path.exists(), "exported HTML file should exist");

    let html = tokio::fs::read_to_string(&result_path)
        .await
        .expect("read HTML");

    assert!(
        html.contains("<!DOCTYPE html>") || html.contains("<html"),
        "should be valid HTML"
    );
    assert!(
        html.contains("Hello from main"),
        "should contain primary messages"
    );
    assert!(
        html.contains("Branch message 1"),
        "should contain merged branch messages"
    );

    let _ = tokio::fs::remove_dir_all(&dir).await;
}

// ---------------------------------------------------------------------------
// Test 2: branch at a specific point, diverge, verify independent trees
// ---------------------------------------------------------------------------
#[tokio::test]
async fn branch_diverge_independent_trees() {
    let dir = temp_dir("branch-diverge");
    let mut manager = SessionManager::new(dir.clone());
    manager
        .create_session("/tmp/work")
        .await
        .expect("create session");

    // Build: root → msg1 → msg2 (linear chain)
    let _id_root = manager
        .append_message(AgentMessage::from_llm(Message::user("root")))
        .await
        .expect("root");

    let id_branch_point = manager
        .append_message(AgentMessage::from_llm(Message::user("branch point")))
        .await
        .expect("branch point");

    // Arm A: continue from branch_point → A1 → A2
    let id_a1 = manager
        .append_message(AgentMessage::from_llm(Message::user("arm-A step 1")))
        .await
        .expect("A1");

    let _id_a2 = manager
        .append_message(AgentMessage::from_llm(Message::user("arm-A step 2")))
        .await
        .expect("A2");

    // Arm B: go back to branch_point and diverge → B1
    manager.branch(&id_branch_point);
    let id_b1 = manager
        .append_message(AgentMessage::from_llm(Message::user("arm-B step 1")))
        .await
        .expect("B1");

    // Verify tree has the expected structure
    let tree = manager.get_tree().await.expect("get_tree");
    assert_eq!(
        tree.len(),
        5,
        "5 total nodes: root + branch_point + A1 + A2 + B1"
    );

    // The branch point should have 2 children
    let branch_node = tree
        .iter()
        .find(|n| n.entry_id == id_branch_point)
        .expect("branch_point node");
    assert_eq!(
        branch_node.children.len(),
        2,
        "branch point should have 2 children"
    );
    assert!(branch_node.children.contains(&id_a1));
    assert!(branch_node.children.contains(&id_b1));

    let _ = tokio::fs::remove_dir_all(&dir).await;
}

// ---------------------------------------------------------------------------
// Test 3: large session stress test (1000 entries)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn large_session_merge_stress() {
    let dir = temp_dir("large-session");

    // Create source session with many entries
    let mut source = SessionManager::new(dir.clone());
    source
        .create_session("/tmp/work")
        .await
        .expect("create source");

    let entry_count = 1000;
    for i in 0..entry_count {
        source
            .append_message(AgentMessage::from_llm(Message::user(format!("msg-{i}"))))
            .await
            .expect("append source entry");
    }

    let source_path = source.session_path().expect("source path").to_path_buf();

    // Verify source tree is intact
    let source_tree = source.get_tree().await.expect("source tree");
    assert_eq!(source_tree.len(), entry_count);

    // Create target and merge
    let mut target = SessionManager::new(dir.clone());
    target
        .create_session("/tmp/work")
        .await
        .expect("create target");

    target
        .append_message(AgentMessage::from_llm(Message::user("target-root")))
        .await
        .expect("target root");

    let merged = target
        .merge(&source_path)
        .await
        .expect("merge large session");
    assert_eq!(merged, entry_count);

    // Verify merged tree integrity
    let merged_tree = target.get_tree().await.expect("merged tree");
    assert_eq!(merged_tree.len(), entry_count + 1); // source entries + 1 target root

    // Verify all IDs are unique
    let ids: std::collections::HashSet<String> =
        merged_tree.iter().map(|n| n.entry_id.clone()).collect();
    assert_eq!(ids.len(), merged_tree.len(), "all IDs must be unique");

    let _ = tokio::fs::remove_dir_all(&dir).await;
}

// ---------------------------------------------------------------------------
// Test 4: merge two forks into the same target
// ---------------------------------------------------------------------------
#[tokio::test]
async fn merge_two_forks_into_same_target() {
    let dir = temp_dir("two-forks");

    // Fork A
    let mut fork_a = SessionManager::new(dir.clone());
    fork_a.create_session("/tmp/work").await.expect("fork A");
    for i in 0..5 {
        fork_a
            .append_message(AgentMessage::from_llm(Message::user(format!("fork-A-{i}"))))
            .await
            .expect("fork A entry");
    }
    let fork_a_path = fork_a.session_path().expect("fork A path").to_path_buf();

    // Fork B
    let mut fork_b = SessionManager::new(dir.clone());
    fork_b.create_session("/tmp/work").await.expect("fork B");
    for i in 0..3 {
        fork_b
            .append_message(AgentMessage::from_llm(Message::user(format!("fork-B-{i}"))))
            .await
            .expect("fork B entry");
    }
    let fork_b_path = fork_b.session_path().expect("fork B path").to_path_buf();

    // Target
    let mut target = SessionManager::new(dir.clone());
    target.create_session("/tmp/work").await.expect("target");
    target
        .append_message(AgentMessage::from_llm(Message::user("target-root")))
        .await
        .expect("target root");

    // Merge both forks
    let merged_a = target.merge(&fork_a_path).await.expect("merge A");
    let merged_b = target.merge(&fork_b_path).await.expect("merge B");

    assert_eq!(merged_a, 5);
    assert_eq!(merged_b, 3);

    let tree = target.get_tree().await.expect("get_tree");
    assert_eq!(tree.len(), 9, "1 target + 5 from A + 3 from B");

    // All IDs unique
    let ids: std::collections::HashSet<String> = tree.iter().map(|n| n.entry_id.clone()).collect();
    assert_eq!(ids.len(), 9);

    let _ = tokio::fs::remove_dir_all(&dir).await;
}
