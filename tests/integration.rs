//! Black-box tests that drive the real `bqg` binary, the way a user would.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    // target/debug/bqg next to the test binary's target dir.
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // test exe
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("bqg");
    p
}

fn tmpdir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!("bqg_it_{}_{}", std::process::id(), tag));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn gen_renders_queries() {
    let d = tmpdir("gen");
    let csv = d.join("in.csv");
    fs::write(&csv, "id,name\n1,ada\n2,bob\n").unwrap();

    let out = Command::new(bin())
        .args(["gen", "-t", "SELECT {{id}} -- {{name}}"])
        .arg(&csv)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "SELECT 1 -- ada\nSELECT 2 -- bob\n");
}

#[test]
fn gen_rejects_missing_column() {
    let d = tmpdir("genmiss");
    let csv = d.join("in.csv");
    fs::write(&csv, "id\n1\n").unwrap();
    let out = Command::new(bin())
        .args(["gen", "-t", "{{nope}}"])
        .arg(&csv)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing column"));
}

#[test]
fn run_executes_then_resumes() {
    let d = tmpdir("run");
    let csv = d.join("in.csv");
    fs::write(&csv, "id,name\n1,a\n2,b\n3,c\n").unwrap();
    let results = d.join("out.jsonl");
    let journal = d.join("j.journal");

    // First run against the offline echo target.
    let out = Command::new(bin())
        .args(["run", "-t", "Q{{id}}", "--target", "echo", "--no-wakelock"])
        .arg("-o")
        .arg(&results)
        .arg("-c")
        .arg(&journal)
        .arg(&csv)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = fs::read_to_string(&results).unwrap();
    assert_eq!(body.lines().count(), 3, "got: {body}");
    assert!(body.contains("\"ok\":true"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("ok 3"));

    // Second run: the journal should make every row skip.
    let out2 = Command::new(bin())
        .args(["run", "-t", "Q{{id}}", "--target", "echo", "--no-wakelock"])
        .arg("-o")
        .arg(d.join("out2.jsonl"))
        .arg("-c")
        .arg(&journal)
        .arg(&csv)
        .output()
        .unwrap();
    assert!(out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(stderr2.contains("skipped 3"), "stderr: {stderr2}");
    assert!(stderr2.contains("resuming"), "stderr: {stderr2}");
}
