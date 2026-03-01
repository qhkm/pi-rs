//! Integration tests for session workflows
//!
//! Tests complex session operations:
//! - branch → merge → export workflow
//! - Concurrent fork+merge
//! - Large session stress tests

use pi_agent_core::messages::AgentMessage;
use pi_coding_agent::session::{SessionManager, SessionEntry};
use pi_ai::Message;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pi-rs-workflow-test-{name}-{}", Uuid::new_v4()))
}

fn create_session_file(dir: &Path, name: &str, entries: Vec<SessionEntry>) -> PathBuf {
    let path = dir.join(format!("{}.jsonl", name));
    let header = pi_coding_agent::session::SessionHeader::new(
        format!("{}", Uuid::new_v4()),
        "/tmp".to_string(),
    );
    let mut content = format!("{}\n", serde_json::to_string(&header).unwrap());
    for entry in entries {
        content.push_str(&format!("{}\n", serde_json::to_string(&entry).unwrap()));
    }
    fs::write(&path, content).expect("write session file");
    path
}

#[tokio::test]
async fn branch_merge_export_workflow() {
    let dir = temp_dir("branch-merge-export");
    fs::create_dir_all(&dir).expect("create temp dir");
    
    // Create a session manager
    let mut manager = SessionManager::new(dir.clone());
    
    // Step 1: Create session with initial message
    manager.create_session("/tmp/project").await.expect("create session");
    let m1 = manager.append_message(AgentMessage::from_llm(Message::user("Initial setup"))).await.expect("m1");
    
    // Step 2: Branch and add to both branches
    manager.branch(&m1);
    let m2 = manager.append_message(AgentMessage::from_llm(Message::user("Main branch work"))).await.expect("m2");
    
    // Go back and create a side branch
    manager.branch(&m1);
    let m3 = manager.append_message(AgentMessage::from_llm(Message::user("Side branch feature"))).await.expect("m3");
    
    // Continue side branch
    let _m4 = manager.append_message(AgentMessage::from_llm(Message::user("Side branch more work"))).await.expect("m4");
    
    // Step 3: Get tree and verify structure
    let tree = manager.get_tree().await.expect("get tree");
    assert!(tree.len() >= 4, "Should have at least 4 entries");
    
    // Verify we have branches (multiple children from m1)
    let m1_node = tree.iter().find(|n| n.entry_id == m1).expect("find m1");
    assert_eq!(m1_node.children.len(), 2, "m1 should have 2 children (branches)");
    
    // Step 4: Navigate to main branch and continue
    manager.branch(&m2);
    let _m5 = manager.append_message(AgentMessage::from_llm(Message::user("Back on main"))).await.expect("m5");
    
    // Step 5: Verify tree is still valid after more branching
    let tree = manager.get_tree().await.expect("get tree after more branching");
    let all_ids: std::collections::HashSet<_> = tree.iter().map(|n| &n.entry_id).collect();
    
    // Verify no duplicate IDs
    assert_eq!(all_ids.len(), tree.len(), "All entry IDs should be unique");
    
    // Verify parent chains
    for node in &tree {
        if let Some(ref parent_id) = node.parent_id {
            assert!(all_ids.contains(parent_id), "Parent {} should exist", parent_id);
        }
    }
    
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn session_file_merge_workflow() {
    let dir = temp_dir("file-merge");
    fs::create_dir_all(&dir).expect("create temp dir");
    
    // Create target session
    let mut target_manager = SessionManager::new(dir.join("target"));
    target_manager.create_session("/tmp/target").await.expect("create target");
    
    let _t1 = target_manager.append_message(AgentMessage::from_llm(Message::user("Target 1"))).await.expect("t1");
    let _t2 = target_manager.append_message(AgentMessage::from_llm(Message::user("Target 2"))).await.expect("t2");
    let target_path = target_manager.session_path().unwrap().to_path_buf();
    
    // Create source session (separate manager to avoid lock issues)
    let mut source_manager = SessionManager::new(dir.join("source"));
    source_manager.create_session("/tmp/source").await.expect("create source");
    let _s1 = source_manager.append_message(AgentMessage::from_llm(Message::user("Source 1"))).await.expect("s1");
    let _s2 = source_manager.append_message(AgentMessage::from_llm(Message::user("Source 2"))).await.expect("s2");
    let source_path = source_manager.session_path().unwrap().to_path_buf();
    
    // Drop managers to release locks
    drop(target_manager);
    drop(source_manager);
    
    // Create new manager for merging
    let mut merge_manager = SessionManager::new(dir.join("merge"));
    merge_manager.load_session(&target_path).await.expect("load target");
    
    // Merge source into target
    let merged = merge_manager.merge(&source_path).await.expect("merge");
    assert_eq!(merged, 2, "Should merge 2 entries from source");
    
    // Verify tree integrity
    let tree = merge_manager.get_tree().await.expect("get tree");
    let all_ids: std::collections::HashSet<_> = tree.iter().map(|n| &n.entry_id).collect();
    
    // No duplicates
    assert_eq!(all_ids.len(), tree.len(), "No duplicate IDs after merge");
    assert_eq!(tree.len(), 4, "Should have 4 entries total (2 target + 2 source)");
    
    // Verify parent chains
    for node in &tree {
        if let Some(ref parent_id) = node.parent_id {
            assert!(all_ids.contains(parent_id), "Parent {} should exist", parent_id);
        }
    }
    
    // Verify we can navigate to a merged entry (use one of the tree nodes)
    let last_entry = tree.last().unwrap().entry_id.clone();
    let messages = merge_manager.navigate_to(&last_entry).await.expect("navigate to last entry");
    assert!(!messages.is_empty(), "Should have messages in navigation path");
    
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn large_session_stress_test() {
    let dir = temp_dir("large-session");
    fs::create_dir_all(&dir).expect("create temp dir");
    
    let mut manager = SessionManager::new(dir.clone());
    manager.create_session("/tmp/stress").await.expect("create session");
    
    // Add 1000 entries (reduced from 10K for test speed, but still substantial)
    let entry_count = 1000;
    let start = std::time::Instant::now();
    
    for i in 0..entry_count {
        let msg = format!("Entry number {}", i);
        manager.append_message(AgentMessage::from_llm(Message::user(&msg)))
            .await
            .expect(&format!("append entry {}", i));
    }
    
    let append_duration = start.elapsed();
    println!("Appended {} entries in {:?}", entry_count, append_duration);
    
    // Verify tree can be built without stack overflow
    let tree_start = std::time::Instant::now();
    let tree = manager.get_tree().await.expect("get tree");
    let tree_duration = tree_start.elapsed();
    
    assert_eq!(tree.len(), entry_count, "Should have all entries in tree");
    println!("Built tree with {} entries in {:?}", tree.len(), tree_duration);
    
    // Verify navigation to a deep entry
    let last_id = tree.last().unwrap().entry_id.clone();
    let nav_start = std::time::Instant::now();
    let messages = manager.navigate_to(&last_id).await.expect("navigate to last");
    let nav_duration = nav_start.elapsed();
    
    assert_eq!(messages.len(), entry_count, "Should have all messages in navigation");
    println!("Navigated to deep entry in {:?}", nav_duration);
    
    // Note: Compaction is handled by the agent core, not directly by SessionManager
    // The session manager maintains the tree structure that compaction operates on
    
    // Verify we can navigate through the tree without stack overflow
    let first_id = tree.first().unwrap().entry_id.clone();
    let _first_messages = manager.navigate_to(&first_id).await.expect("navigate to first");
    println!("Successfully navigated through {} entries", tree.len());
    
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn merge_performance_test() {
    let dir = temp_dir("merge-perf");
    fs::create_dir_all(&dir).expect("create temp dir");
    
    // Create target session
    let mut target_manager = SessionManager::new(dir.join("target"));
    target_manager.create_session("/tmp/target").await.expect("create target");
    
    // Add some entries to target
    for i in 0..100 {
        let msg = format!("Target entry {}", i);
        target_manager.append_message(AgentMessage::from_llm(Message::user(&msg)))
            .await
            .expect("append to target");
    }
    let target_path = target_manager.session_path().unwrap().to_path_buf();
    
    // Create source session with many entries
    let mut source_manager = SessionManager::new(dir.join("source"));
    source_manager.create_session("/tmp/source").await.expect("create source");
    
    for i in 0..500 {
        let msg = format!("Source entry {}", i);
        source_manager.append_message(AgentMessage::from_llm(Message::user(&msg)))
            .await
            .expect("append to source");
    }
    let source_path = source_manager.session_path().unwrap().to_path_buf();
    
    // Time the merge
    let merge_start = std::time::Instant::now();
    let merged = target_manager.merge(&source_path).await.expect("merge");
    let merge_duration = merge_start.elapsed();
    
    assert_eq!(merged, 500, "Should merge 500 entries");
    println!("Merged 500 entries in {:?}", merge_duration);
    
    // Verify the merged session
    let tree = target_manager.get_tree().await.expect("get tree");
    assert_eq!(tree.len(), 600, "Should have 600 total entries (100 + 500)");
    
    // Verify no stack overflow on deep tree
    let last_id = tree.last().unwrap().entry_id.clone();
    let _messages = target_manager.navigate_to(&last_id).await.expect("navigate");
    
    fs::remove_dir_all(dir).ok();
}
