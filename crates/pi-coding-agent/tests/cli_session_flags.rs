use pi_coding_agent::session::SessionHeader;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pi-rs-cli-test-{name}-{}", Uuid::new_v4()))
}

fn parse_header(path: &Path) -> SessionHeader {
    let content = fs::read_to_string(path).expect("session file should exist");
    let first_line = content
        .lines()
        .next()
        .expect("session file should have a header line");
    serde_json::from_str(first_line).expect("header line should be valid JSON")
}

fn run_pi(args: &[&str], workdir: &Path) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi"));
    cmd.arg("--provider").arg("anthropic-messages");
    cmd.args(args)
        .env("ANTHROPIC_API_KEY", "test-key")
        .current_dir(workdir)
        .output()
        .expect("pi command should run")
}

#[test]
fn session_flag_creates_new_session_file() {
    let dir = temp_dir("create");
    fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("custom-session.jsonl");

    let output = run_pi(
        &[
            "--mode",
            "rpc",
            "--session",
            path.to_str().expect("utf8 path"),
        ],
        &dir,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(path.exists());

    let header = parse_header(&path);
    assert_eq!(header.entry_type, "session");
    assert_eq!(header.version, 3);

    fs::remove_dir_all(dir).ok();
}

#[test]
fn session_flag_recovers_corrupted_file() {
    let dir = temp_dir("recover");
    fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("corrupted.jsonl");
    fs::write(&path, "garbage\n").expect("write corrupted content");

    let output = run_pi(
        &[
            "--mode",
            "rpc",
            "--session",
            path.to_str().expect("utf8 path"),
        ],
        &dir,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let content = fs::read_to_string(&path).expect("read recovered file");
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 1);
    let header = parse_header(&path);
    assert_eq!(header.entry_type, "session");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn resume_with_existing_sessions_does_not_create_extra_file() {
    let dir = temp_dir("resume-existing");
    fs::create_dir_all(&dir).expect("create temp dir");

    let older = dir.join("older.jsonl");
    let newer = dir.join("newer.jsonl");
    let old_header = SessionHeader::new("old-id".to_string(), "/tmp/work".to_string());
    fs::write(
        &older,
        format!("{}\n", serde_json::to_string(&old_header).expect("json")),
    )
    .expect("write old session");
    std::thread::sleep(std::time::Duration::from_millis(10));
    let new_header = SessionHeader::new("new-id".to_string(), "/tmp/work".to_string());
    fs::write(
        &newer,
        format!("{}\n", serde_json::to_string(&new_header).expect("json")),
    )
    .expect("write new session");

    let output = run_pi(
        &[
            "--mode",
            "rpc",
            "--resume",
            "--session-dir",
            dir.to_str().expect("utf8 path"),
        ],
        &dir,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let jsonl_count = fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|ext| ext == "jsonl").unwrap_or(false))
        .count();
    assert_eq!(jsonl_count, 2);

    fs::remove_dir_all(dir).ok();
}

#[test]
fn resume_with_empty_session_dir_creates_one_file() {
    let dir = temp_dir("resume-empty");
    fs::create_dir_all(&dir).expect("create temp dir");

    let output = run_pi(
        &[
            "--mode",
            "rpc",
            "--resume",
            "--session-dir",
            dir.to_str().expect("utf8 path"),
        ],
        &dir,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let jsonl_files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|ext| ext == "jsonl").unwrap_or(false))
        .collect();
    assert_eq!(jsonl_files.len(), 1);
    let header = parse_header(&jsonl_files[0]);
    assert_eq!(header.entry_type, "session");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn no_session_flag_does_not_create_session_artifacts() {
    let dir = temp_dir("no-session");
    fs::create_dir_all(&dir).expect("create temp dir");

    let output = run_pi(
        &[
            "--mode",
            "rpc",
            "--no-session",
            "--session-dir",
            dir.to_str().expect("utf8 path"),
        ],
        &dir,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let is_empty = fs::read_dir(&dir).expect("read dir").next().is_none();
    assert!(is_empty);

    fs::remove_dir_all(dir).ok();
}
