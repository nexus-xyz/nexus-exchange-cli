//! `nexus examples`: find and fetch apps from the examples catalog (ENG-17337).
//!
//! The catalog is `catalog.json` at the root of `nexus-exchange-examples`, one
//! entry per example. It is read from the repo at run time rather than compiled
//! in, so a new example shows up without a CLI release.
//!
//! Transport is `git`, not HTTP. The CLI carries no transport of its own (every
//! API call goes through the `nexus-exchange` SDK), and this module keeps that
//! true: a shallow, blobless, sparse clone fetches only `catalog.json`, or only
//! the one example directory `get` asked for. The cost is that these commands
//! need `git` on PATH, and they say so when it is missing.
//!
//! Every example is self-contained in its own directory (standard #1 of the
//! examples repo's CONTRIBUTING.md), which is what makes copying one directory
//! out a complete download.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Where the catalog lives. `NEXUS_EXAMPLES_REPO` overrides it, which is how the
/// tests point at a local repository instead of the network.
pub const DEFAULT_REPO: &str = "https://github.com/nexus-xyz/nexus-exchange-examples.git";

/// The only catalog schema this build understands.
const SCHEMA: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    pub examples: Vec<Example>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Example {
    pub id: String,
    pub track: String,
    pub language: String,
    pub path: String,
    pub summary: String,
    pub credentials: String,
    pub writes: bool,
    pub toolchain: Vec<String>,
    pub setup: Vec<String>,
    pub run: String,
}

/// The repository to read from: `NEXUS_EXAMPLES_REPO`, else the public repo.
pub fn repo() -> String {
    std::env::var("NEXUS_EXAMPLES_REPO")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string())
}

/// A scratch directory removed on drop, so a failed clone leaves nothing behind.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("nexus-examples-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = match cmd.output() {
        Ok(out) => out,
        Err(e) if e.kind() == io::ErrorKind::NotFound => bail!(
            "`nexus examples` needs `git` on your PATH to download examples, and none was found. \
             Install git, or browse the catalog at {DEFAULT_REPO}"
        ),
        Err(e) => return Err(e).context("running git"),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}

/// A sparse, shallow, blobless checkout of `git_ref`: only top-level files
/// (so `catalog.json`) until a directory is added to the sparse set.
fn sparse_clone(repo: &str, git_ref: &str, into: &Path) -> Result<PathBuf> {
    let dest = into.join("repo");
    let dest_str = dest.to_string_lossy();
    git(
        &[
            "clone",
            "--quiet",
            "--depth",
            "1",
            "--filter=blob:none",
            "--sparse",
            "--branch",
            git_ref,
            repo,
            &dest_str,
        ],
        None,
    )
    .with_context(|| format!("fetching the examples catalog from {repo} at `{git_ref}`"))?;
    Ok(dest)
}

pub fn parse_catalog(text: &str) -> Result<Catalog> {
    let catalog: Catalog = serde_json::from_str(text).context("catalog.json is not valid")?;
    if catalog.schema != SCHEMA {
        bail!(
            "catalog.json uses schema {}, and this nexus understands schema {SCHEMA}. \
             Upgrade nexus-exchange-cli.",
            catalog.schema
        );
    }
    Ok(catalog)
}

/// Fetch and parse the catalog at `git_ref`.
pub fn fetch_catalog(git_ref: &str) -> Result<Catalog> {
    let scratch = Scratch::new()?;
    let checkout = sparse_clone(&repo(), git_ref, &scratch.0)?;
    let text = fs::read_to_string(checkout.join("catalog.json"))
        .context("the examples repository has no catalog.json at that ref")?;
    parse_catalog(&text)
}

/// Normalise a `--lang` value to the catalog's `language` spelling.
pub fn normalize_lang(lang: &str) -> Result<&'static str> {
    Ok(match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rust",
        "typescript" | "ts" | "node" | "js" => "typescript",
        "python" | "py" => "python",
        "shell" | "sh" | "bash" | "cli" => "shell",
        other => bail!("unknown --lang `{other}`: use rust, ts, python or shell"),
    })
}

/// Filter for `list`.
pub fn filter<'a>(
    catalog: &'a Catalog,
    track: Option<&str>,
    lang: Option<&str>,
) -> Result<Vec<&'a Example>> {
    let lang = lang.map(normalize_lang).transpose()?;
    Ok(catalog
        .examples
        .iter()
        .filter(|e| track.is_none_or(|t| e.track == t))
        .filter(|e| lang.is_none_or(|l| e.language == l))
        .collect())
}

/// Resolve `get`/`show`'s argument: a full `track/id` path, or an id narrowed by
/// `--lang`. An ambiguous id is an error that lists the choices; it never guesses.
pub fn resolve<'a>(catalog: &'a Catalog, id: &str, lang: Option<&str>) -> Result<&'a Example> {
    if id.contains('/') {
        return catalog
            .examples
            .iter()
            .find(|e| e.path == id)
            .ok_or_else(|| anyhow!("no example at `{id}`. Run `nexus examples list`."));
    }
    let lang = lang.map(normalize_lang).transpose()?;
    let matches: Vec<&Example> = catalog
        .examples
        .iter()
        .filter(|e| e.id == id)
        .filter(|e| lang.is_none_or(|l| e.language == l))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => {
            let near: Vec<&str> = catalog
                .examples
                .iter()
                .filter(|e| e.id.contains(id) || id.contains(e.id.as_str()))
                .map(|e| e.path.as_str())
                .collect();
            if near.is_empty() {
                bail!("no example named `{id}`. Run `nexus examples list`.")
            }
            bail!(
                "no example named `{id}`. Did you mean: {}?",
                near.join(", ")
            )
        }
        many => {
            let options: Vec<String> = many
                .iter()
                .map(|e| format!("{} (--lang {})", e.path, e.language))
                .collect();
            bail!(
                "`{id}` exists in more than one language: {}. Pick one with --lang, or pass the full path.",
                options.join(", ")
            )
        }
    }
}

/// Copy one example directory out of a sparse checkout into `dest`.
pub fn get(example: &Example, git_ref: &str, dest: &Path) -> Result<()> {
    if dest.exists()
        && fs::read_dir(dest)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        bail!(
            "{} already exists and is not empty. Pass --dir to choose another location.",
            dest.display()
        );
    }
    let scratch = Scratch::new()?;
    let checkout = sparse_clone(&repo(), git_ref, &scratch.0)?;
    git(&["sparse-checkout", "set", &example.path], Some(&checkout))
        .with_context(|| format!("checking out {}", example.path))?;
    let src = checkout.join(&example.path);
    if !src.is_dir() {
        bail!(
            "the catalog lists {}, but that directory is not in the repository at `{git_ref}`",
            example.path
        );
    }
    copy_dir(&src, dest)
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to).with_context(|| format!("writing {}", to.display()))?;
        }
    }
    Ok(())
}

fn credentials_note(e: &Example) -> &'static str {
    match e.credentials.as_str() {
        "none" => "no credentials needed",
        "optional" => "runs without credentials; a testnet API key unlocks the rest",
        "required" => "needs a testnet API key (see .env.example)",
        _ => "see the README for credentials",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

pub fn render_list(examples: &[&Example]) -> String {
    if examples.is_empty() {
        return "No examples match.".to_string();
    }
    let mut out = format!(
        "{:<20}  {:<12}  {:<10}  {:<8}  {:<6}  {}\n",
        "ID", "TRACK", "LANGUAGE", "CREDS", "WRITES", "SUMMARY"
    );
    for e in examples {
        out.push_str(&format!(
            "{:<20}  {:<12}  {:<10}  {:<8}  {:<6}  {}\n",
            e.id,
            e.track,
            e.language,
            e.credentials,
            if e.writes { "yes" } else { "no" },
            truncate(&e.summary, 70),
        ));
    }
    out.push_str(&format!(
        "\n{} example(s). `nexus examples show <id>` for details, `nexus examples get <id>` to download one.",
        examples.len()
    ));
    out
}

pub fn render_show(e: &Example, id_is_unique: bool) -> String {
    let mut out = format!(
        "{}  ({}, {})\n\n{}\n\n",
        e.id, e.path, e.language, e.summary
    );
    out.push_str(&format!("Credentials: {}\n", credentials_note(e)));
    out.push_str(&format!(
        "Writes:      {}\n",
        if e.writes {
            "yes: it can place or cancel orders or change account state (testnet)"
        } else {
            "no: read-only"
        }
    ));
    out.push_str(&format!("Toolchain:   {}\n", e.toolchain.join(", ")));
    out.push_str(&render_steps(e));
    out.push_str(&format!(
        "\nGet it:  nexus examples get {}",
        if id_is_unique { &e.id } else { &e.path }
    ));
    out
}

fn render_steps(e: &Example) -> String {
    let mut out = String::new();
    if !e.setup.is_empty() {
        out.push_str("\nSetup:\n");
        for step in &e.setup {
            out.push_str(&format!("  {step}\n"));
        }
    }
    out.push_str(&format!("\nRun:\n  {}\n", e.run));
    out
}

pub fn render_get(e: &Example, dest: &Path) -> String {
    let mut out = format!("Downloaded {} to {}\n", e.path, dest.display());
    out.push_str(&format!("{}.\n", capitalize(credentials_note(e))));
    if e.writes {
        out.push_str(
            "It can place or cancel orders on testnet; read its README before running it live.\n",
        );
    }
    out.push_str(&format!("\n  cd {}\n", dest.display()));
    out.push_str(&render_steps(e).replace("\n  ", "\n    "));
    out.push_str("\nFull instructions are in its README.md.");
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, track: &str, language: &str) -> Example {
        Example {
            id: id.into(),
            track: track.into(),
            language: language.into(),
            path: format!("{track}/{id}"),
            summary: "s".into(),
            credentials: "none".into(),
            writes: false,
            toolchain: vec![],
            setup: vec![],
            run: "npm start".into(),
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            schema: 1,
            examples: vec![
                entry("risk-guard", "sdk-rust", "rust"),
                entry("risk-guard", "sdk-ts", "typescript"),
                entry("agent-enrollment", "sdk-python", "python"),
            ],
        }
    }

    #[test]
    fn resolves_a_unique_id() {
        let c = catalog();
        assert_eq!(
            resolve(&c, "agent-enrollment", None).unwrap().path,
            "sdk-python/agent-enrollment"
        );
    }

    #[test]
    fn refuses_to_guess_an_ambiguous_id() {
        let c = catalog();
        let err = resolve(&c, "risk-guard", None).unwrap_err().to_string();
        assert!(
            err.contains("sdk-rust/risk-guard") && err.contains("sdk-ts/risk-guard"),
            "{err}"
        );
    }

    #[test]
    fn lang_and_full_path_disambiguate() {
        let c = catalog();
        assert_eq!(
            resolve(&c, "risk-guard", Some("ts")).unwrap().path,
            "sdk-ts/risk-guard"
        );
        assert_eq!(
            resolve(&c, "sdk-rust/risk-guard", None).unwrap().language,
            "rust"
        );
    }

    #[test]
    fn unknown_id_suggests_near_matches() {
        let c = catalog();
        let err = resolve(&c, "risk", None).unwrap_err().to_string();
        assert!(err.contains("Did you mean"), "{err}");
    }

    #[test]
    fn rejects_an_unknown_schema() {
        let err = parse_catalog(r#"{"schema": 2, "examples": []}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema 2"), "{err}");
    }

    #[test]
    fn filters_by_track_and_lang() {
        let c = catalog();
        assert_eq!(filter(&c, Some("sdk-rust"), None).unwrap().len(), 1);
        assert_eq!(filter(&c, None, Some("py")).unwrap().len(), 1);
        assert!(filter(&c, None, Some("cobol")).is_err());
    }

    #[test]
    fn get_refuses_a_non_empty_destination() {
        let scratch = Scratch::new().unwrap();
        fs::write(scratch.0.join("x"), "x").unwrap();
        let err = get(&entry("a", "b", "rust"), "main", &scratch.0)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not empty"), "{err}");
    }
}
