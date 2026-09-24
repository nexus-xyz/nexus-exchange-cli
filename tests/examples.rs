//! `nexus examples` end to end against a throwaway local git repository
//! (ENG-17337). `NEXUS_EXAMPLES_REPO` points the command at it, so this runs
//! offline and exercises the real sparse-clone path, not a stub.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Std;

use assert_cmd::Command;
use serde_json::Value;

const CATALOG: &str = r#"{
  "schema": 1,
  "repository": "local",
  "examples": [
    {"id": "hello", "track": "sdk-ts", "language": "typescript", "path": "sdk-ts/hello",
     "summary": "Says hello.", "credentials": "none", "writes": false,
     "toolchain": ["node >= 22"], "setup": ["npm install"], "run": "npm start"},
    {"id": "guard", "track": "sdk-rust", "language": "rust", "path": "sdk-rust/guard",
     "summary": "Guards.", "credentials": "required", "writes": true,
     "toolchain": ["rust >= 1.86"], "setup": ["cp .env.example .env"], "run": "cargo run"},
    {"id": "guard", "track": "sdk-python", "language": "python", "path": "sdk-python/guard",
     "summary": "Guards.", "credentials": "required", "writes": true,
     "toolchain": ["python >= 3.12"], "setup": [], "run": "python guard.py"}
  ]
}"#;

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("nexus-examples-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Std::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git runs")
        .success();
    assert!(ok, "git {args:?} failed");
}

/// A repo with the catalog above and each example's files.
fn catalog_repo(name: &str) -> PathBuf {
    let repo = scratch(name);
    fs::write(repo.join("catalog.json"), CATALOG).unwrap();
    for dir in ["sdk-ts/hello", "sdk-rust/guard", "sdk-python/guard"] {
        fs::create_dir_all(repo.join(dir).join("src")).unwrap();
        fs::write(repo.join(dir).join("README.md"), format!("# {dir}\n")).unwrap();
        fs::write(repo.join(dir).join("src/main.txt"), "body").unwrap();
    }
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "catalog"]);
    repo
}

/// Each test gets its own cache directory (`cwd/.cache`) so they never share one.
fn nexus(repo: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("nexus").unwrap();
    cmd.env("NEXUS_EXAMPLES_REPO", format!("file://{}", repo.display()))
        .env("XDG_CACHE_HOME", cwd.join(".cache"))
        .env_remove("NEXUS_OUTPUT")
        .current_dir(cwd);
    cmd
}

fn stdout_of(cmd: &mut Command) -> (String, String) {
    let out = cmd.assert().success().get_output().clone();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn list_reads_the_catalog_and_filters() {
    let repo = catalog_repo("list");
    let cwd = scratch("list-cwd");
    let out = nexus(&repo, &cwd)
        .args(["--output", "json", "examples", "list", "--lang", "rust"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["path"], "sdk-rust/guard");
}

#[test]
fn get_copies_exactly_one_example_directory() {
    let repo = catalog_repo("get");
    let cwd = scratch("get-cwd");
    nexus(&repo, &cwd)
        .args(["examples", "get", "hello"])
        .assert()
        .success();
    assert!(cwd.join("hello/README.md").is_file());
    assert_eq!(
        fs::read_to_string(cwd.join("hello/src/main.txt")).unwrap(),
        "body"
    );
    assert!(
        !cwd.join("hello/catalog.json").exists(),
        "only the example directory is copied"
    );
}

#[test]
fn get_refuses_an_ambiguous_id_and_accepts_lang() {
    let repo = catalog_repo("ambiguous");
    let cwd = scratch("ambiguous-cwd");
    let err = nexus(&repo, &cwd)
        .args(["examples", "get", "guard"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&err);
    assert!(
        err.contains("sdk-rust/guard") && err.contains("sdk-python/guard"),
        "{err}"
    );
    assert!(
        !cwd.join("guard").exists(),
        "nothing is written when the id is ambiguous"
    );

    nexus(&repo, &cwd)
        .args(["examples", "get", "guard", "--lang", "py", "--dir", "g"])
        .assert()
        .success();
    assert!(cwd.join("g/README.md").is_file());
}

#[test]
fn an_unreachable_repo_falls_back_to_the_cache_then_the_built_in_copy() {
    let repo = catalog_repo("fallback");
    let cwd = scratch("fallback-cwd");
    let gone = cwd.join("no-such-repo");

    // Nothing cached yet: the built-in copy answers, and says so.
    let (out, err) = stdout_of(nexus(&gone, &cwd).args(["examples", "list"]));
    assert!(err.contains("built into nexus"), "{err}");
    assert!(
        out.contains("agent-enrollment"),
        "the real catalog is built in: {out}"
    );

    // A live list caches the catalog...
    let (_, err) = stdout_of(nexus(&repo, &cwd).args(["examples", "list"]));
    assert!(err.is_empty(), "a live read prints no note: {err}");
    assert!(cwd.join(".cache/nexus/examples-catalog.json").is_file());

    // ...which then answers when the repository can't be reached, and for --offline.
    let (out, err) = stdout_of(nexus(&gone, &cwd).args(["examples", "list"]));
    assert!(err.contains("cached"), "{err}");
    assert!(
        out.contains("hello"),
        "the cached (test) catalog, not the built-in one: {out}"
    );
    let (_, err) = stdout_of(nexus(&repo, &cwd).args(["examples", "show", "hello", "--offline"]));
    assert!(err.contains("offline") && err.contains("cached"), "{err}");
}

#[test]
fn a_non_default_ref_never_falls_back_to_mains_catalog() {
    let repo = catalog_repo("ref");
    let cwd = scratch("ref-cwd");
    nexus(&repo, &cwd)
        .args(["examples", "list", "--ref", "no-such-branch"])
        .assert()
        .failure();
}
