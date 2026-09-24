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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// The catalog as it was when this binary was built: the last-resort fallback
/// when the repository can't be reached and nothing is cached. Refresh it with
/// `scripts/sync_examples_catalog.sh`.
const SNAPSHOT: &str = include_str!("examples_catalog.json");

/// The examples-repo tag `SNAPSHOT` was taken from (`catalog-YYYY.MM.DD`), or
/// `untagged` before the first pin.
const SNAPSHOT_REF: &str = include_str!("examples_catalog.ref");

/// `SNAPSHOT_REF`, or `None` when the snapshot was never pinned to a tag.
pub fn snapshot_ref() -> Option<&'static str> {
    let r = SNAPSHOT_REF.trim();
    r.starts_with("catalog-").then_some(r)
}

/// The ref whose catalog is cached. Only the default branch is: a cached copy of
/// some other ref standing in for `main` would be wrong in a way nobody sees.
pub const DEFAULT_REF: &str = "main";

/// Where the catalog came from, so the caller can say so when it isn't live.
#[derive(Debug)]
pub enum Source {
    Live,
    /// The last catalog fetched from `main`, and how long ago it was saved.
    Cache(Option<Duration>),
    /// The copy built into this binary.
    Snapshot,
}

/// `$XDG_CACHE_HOME/nexus/examples-catalog.json`, falling back to
/// `$HOME/.cache/nexus/examples-catalog.json` (the config file's convention).
pub fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    Some(base.join("nexus").join("examples-catalog.json"))
}

/// Best-effort: a cache that can't be written costs a fallback later, never the
/// command that is running now.
fn save_cache(git_ref: &str, text: &str) {
    if git_ref != DEFAULT_REF {
        return;
    }
    if let Some(path) = cache_path() {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

fn read_catalog_file(checkout: &Path) -> Result<String> {
    fs::read_to_string(checkout.join("catalog.json"))
        .context("the examples repository has no catalog.json at that ref")
}

/// The catalog for `list` and `show`: live from the repository, else (for the
/// default ref only) the cached copy, else the copy built into this binary.
/// `offline` skips the network. A non-default `--ref` that can't be fetched is
/// an error: falling back to `main`'s catalog would answer a different question.
pub fn load_catalog(git_ref: &str, offline: bool) -> Result<(Catalog, Source)> {
    let live_err = if offline {
        None
    } else {
        let fetched = (|| -> Result<Catalog> {
            let scratch = Scratch::new()?;
            let checkout = sparse_clone(&repo(), git_ref, &scratch.0)?;
            let text = read_catalog_file(&checkout)?;
            let catalog = parse_catalog(&text)?;
            save_cache(git_ref, &text);
            Ok(catalog)
        })();
        match fetched {
            Ok(catalog) => return Ok((catalog, Source::Live)),
            Err(e) if git_ref != DEFAULT_REF => return Err(e),
            Err(e) => Some(e),
        }
    };
    if git_ref != DEFAULT_REF {
        bail!("--offline only has the default catalog (`{DEFAULT_REF}`); drop --ref or go online");
    }
    if let Some(path) = cache_path() {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(catalog) = parse_catalog(&text) {
                let age = fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok());
                return Ok((catalog, Source::Cache(age)));
            }
        }
    }
    match parse_catalog(SNAPSHOT) {
        Ok(catalog) => Ok((catalog, Source::Snapshot)),
        Err(snapshot_err) => Err(live_err.unwrap_or(snapshot_err)),
    }
}

/// The stderr note for a catalog that isn't live, or `None` when it is.
pub fn source_note(source: &Source, offline: bool) -> Option<String> {
    let why = if offline {
        "offline"
    } else {
        "could not reach the examples repository"
    };
    match source {
        Source::Live => None,
        Source::Cache(age) => Some(format!(
            "note: {why}; showing the catalog cached {}.",
            age.map(ago).unwrap_or_else(|| "earlier".to_string())
        )),
        Source::Snapshot => Some(format!(
            "note: {why} and nothing is cached; showing the catalog built into nexus {}{}. \
             It may be missing newer examples.",
            env!("CARGO_PKG_VERSION"),
            snapshot_ref()
                .map(|t| format!(" (examples {t})"))
                .unwrap_or_default()
        )),
    }
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=119 => "just now".to_string(),
        120..=7199 => format!("{} minutes ago", s / 60),
        7200..=172_799 => format!("{} hours ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
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

fn ensure_empty(dest: &Path) -> Result<()> {
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
    Ok(())
}

/// Download one example into `dir` (default `./<id>`), from a single sparse
/// clone: read the catalog, resolve the id against it, then check out and copy
/// only that example's directory. Needs the network; there is nothing to copy
/// from a cache.
pub fn get(
    id: &str,
    lang: Option<&str>,
    git_ref: &str,
    dir: Option<PathBuf>,
) -> Result<(Example, PathBuf)> {
    if let Some(dest) = &dir {
        ensure_empty(dest)?;
    }
    let scratch = Scratch::new()?;
    let checkout = sparse_clone(&repo(), git_ref, &scratch.0)?;
    let text = read_catalog_file(&checkout)?;
    let catalog = parse_catalog(&text)?;
    save_cache(git_ref, &text);
    let example = resolve(&catalog, id, lang)?.clone();
    let dest = dir.unwrap_or_else(|| PathBuf::from(&example.id));
    ensure_empty(&dest)?;
    git(&["sparse-checkout", "set", &example.path], Some(&checkout))
        .with_context(|| format!("checking out {}", example.path))?;
    let src = checkout.join(&example.path);
    if !src.is_dir() {
        bail!(
            "the catalog lists {}, but that directory is not in the repository at `{git_ref}`",
            example.path
        );
    }
    copy_dir(&src, &dest)?;
    Ok((example, dest))
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
    fn get_refuses_a_non_empty_destination_before_fetching() {
        let scratch = Scratch::new().unwrap();
        fs::write(scratch.0.join("x"), "x").unwrap();
        let err = get("a", None, "main", Some(scratch.0.clone()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not empty"), "{err}");
    }

    #[test]
    fn snapshot_ref_is_a_catalog_tag_or_untagged() {
        let r = SNAPSHOT_REF.trim();
        assert!(r == "untagged" || r.starts_with("catalog-"), "{r}");
        assert_eq!(snapshot_ref().is_some(), r.starts_with("catalog-"));
    }

    #[test]
    fn the_built_in_snapshot_parses() {
        let catalog = parse_catalog(SNAPSHOT).expect("snapshot is a valid schema-1 catalog");
        assert!(!catalog.examples.is_empty());
    }
}
