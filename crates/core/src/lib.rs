//! Independent local-first Markdown core backed by SQLite FTS5.
//!
//! ```no_run
//! use mdwell_core::{Core, Result};
//! fn example() -> Result<()> {
//!     let core = Core::open("/tmp/mdwell-data")?;
//!     let workspace = core.add_local("/home/user/notes")?;
//!     let documents = core.list_documents(workspace.id)?;
//!     if let Some(summary) = documents.first() {
//!         let document = core.read_document(workspace.id, &summary.path)?;
//!         core.save_document(workspace.id, &document.path, "# Updated", &document.revision)?;
//!     }
//!     let hits = core.search("\"lunar module\"", Some(workspace.id))?;
//!     for hit in hits { println!("{}:{} {}", hit.path, hit.source_start, hit.snippet); }
//!     Ok(())
//! }
//! ```
//!
//! All source offsets are UTF-8 byte offsets, not character indices. Paths exposed by
//! document APIs are normalized, slash-separated workspace-relative paths. Persisted
//! search is a snapshot: call `refresh_workspace` after edits outside this core.
//!
//! Rendering supports GFM, hides leading YAML frontmatter, and disables raw HTML.
//! Relative Markdown images become inert `img` elements with `data-local-src` and
//! `alt`, never `src`. Desktop hosts must validate these paths with `resolve_link`,
//! load bounded raster bytes with `read_asset`, and supply their own blob URLs. SVG
//! is deliberately unsupported. GFM alert classes survive sanitization for host CSS;
//! inline styles and unsafe link schemes do not. Treat snippets as plain text.
//!
//! Methods are synchronous; desktop hosts should call them off the UI thread. Git is
//! the only external tool dependency; OAuth helpers re-invoke the host executable.
//! GitHub import/sync are the only network operations.
//! Revisions detect stale saves, atomic rename preserves permissions, and mutation
//! locks serialize cooperating instances using the same data directory. Uncooperative
//! external filesystem writers can still race the final revision check and rename;
//! canonical path checks likewise assume directories are not adversarially swapped
//! during an operation. Filesystem writes and SQLite commits are not one transaction:
//! after a database failure following a save, refresh repairs the index from disk.
mod git;
mod markdown;
mod paths;
mod search;
mod types;

pub use git::handle_git_credential_request;
pub use paths::{MAX_ASSET_BYTES, MAX_DOCUMENT_BYTES};
pub use types::*;

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub struct Core {
    data_dir: PathBuf,
    db: Mutex<Connection>,
    mutation: Mutex<()>,
    mutation_file: fs::File,
}

struct MutationGuard<'a> {
    _local: MutexGuard<'a, ()>,
    file: &'a fs::File,
}

impl Drop for MutationGuard<'_> {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| Error::LockPoisoned)
}

fn workspace_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    let kind: String = row.get(1)?;
    Ok(Workspace {
        id: row.get(0)?,
        kind: if kind == "github" {
            WorkspaceKind::Github
        } else {
            WorkspaceKind::Local
        },
        name: row.get(2)?,
        root: row.get(3)?,
        url: row.get(4)?,
    })
}

impl Core {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(data_dir.as_ref())?;
        let data_dir = data_dir.as_ref().canonicalize()?;
        let mutation_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(data_dir.join("mutation.lock"))?;
        let mut db = Connection::open(data_dir.join("index.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS workspaces (
                id INTEGER PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL,
                root TEXT NOT NULL UNIQUE, url TEXT UNIQUE
            );
            CREATE TABLE IF NOT EXISTS documents (
                workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
                path TEXT NOT NULL, title TEXT NOT NULL, revision TEXT NOT NULL,
                PRIMARY KEY(workspace_id, path)
            );
            CREATE TABLE IF NOT EXISTS document_blocks (
                workspace_id INTEGER NOT NULL, path TEXT NOT NULL,
                source_start INTEGER NOT NULL, content TEXT NOT NULL,
                PRIMARY KEY(workspace_id,path,source_start),
                FOREIGN KEY(workspace_id,path) REFERENCES documents(workspace_id,path) ON DELETE CASCADE
            );",
        )?;
        search::migrate(&mut db)?;
        Ok(Self {
            data_dir,
            db: Mutex::new(db),
            mutation: Mutex::new(()),
            mutation_file,
        })
    }

    fn lock_mutation(&self) -> Result<MutationGuard<'_>> {
        let local = lock(&self.mutation)?;
        self.mutation_file.lock()?;
        Ok(MutationGuard {
            _local: local,
            file: &self.mutation_file,
        })
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let db = lock(&self.db)?;
        let mut stmt = db.prepare("SELECT id,kind,name,root,url FROM workspaces ORDER BY id")?;
        let rows = stmt.query_map([], workspace_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn workspace(&self, id: i64) -> Result<Workspace> {
        lock(&self.db)?
            .query_row(
                "SELECT id,kind,name,root,url FROM workspaces WHERE id=?1",
                [id],
                workspace_row,
            )
            .optional()?
            .ok_or(Error::WorkspaceNotFound(id))
    }

    fn root(&self, workspace: &Workspace) -> Result<PathBuf> {
        let root = PathBuf::from(&workspace.root);
        // A replaced workspace root must not silently redirect access elsewhere.
        if !root.is_dir() || root.canonicalize()? != root {
            return Err(Error::UnsafePath(workspace.root.clone()));
        }
        Ok(root)
    }

    fn register(&self, root: &Path, kind: WorkspaceKind, url: Option<&str>) -> Result<Workspace> {
        let root = root
            .to_str()
            .ok_or_else(|| Error::UnsafePath("non-UTF-8 workspace root".into()))?;
        let name = url
            .and_then(|url| url.rsplit('/').next())
            .map(|name| name.trim_end_matches(".git"))
            .or_else(|| Path::new(root).file_name().and_then(|v| v.to_str()))
            .unwrap_or("workspace");
        let db = lock(&self.db)?;
        db.execute(
            "INSERT OR IGNORE INTO workspaces(kind,name,root,url) VALUES(?1,?2,?3,?4)",
            params![
                if kind == WorkspaceKind::Local {
                    "local"
                } else {
                    "github"
                },
                name,
                root,
                url
            ],
        )?;
        Ok(db.query_row(
            "SELECT id,kind,name,root,url FROM workspaces WHERE root=?1",
            [root],
            workspace_row,
        )?)
    }

    pub fn add_local(&self, path: impl AsRef<Path>) -> Result<Workspace> {
        let _guard = self.lock_mutation()?;
        let root = path.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(Error::UnsafePath(root.display().to_string()));
        }
        let workspace = self.register(&root, WorkspaceKind::Local, None)?;
        self.refresh_inner(&workspace)?;
        Ok(workspace)
    }

    /// Clone a GitHub HTTPS or SSH repository, including private repositories.
    /// Uses trusted system/global credential helpers or the user's SSH agent/config;
    /// never prompts for credentials. See docs/AUTHENTICATION.md.
    pub fn add_github(&self, url: &str) -> Result<Workspace> {
        self.add_github_with_token(url, None)
    }

    /// Optional in-memory OAuth credentials for HTTPS github.com only. SSH ignores
    /// the token; `None` retains trusted credential helpers. The host executable
    /// must dispatch `handle_git_credential_request` before normal startup.
    pub fn add_github_with_token(&self, url: &str, token: Option<&str>) -> Result<Workspace> {
        let url = git::validate_url(url)?;
        let _guard = self.lock_mutation()?;
        if let Some(existing) = self
            .list_workspaces()?
            .into_iter()
            .find(|w| w.url.as_deref() == Some(&url))
        {
            return Ok(existing);
        }
        let repositories = self.data_dir.join("repositories");
        fs::create_dir_all(&repositories)?;
        let staging = tempfile::tempdir_in(&repositories)?;
        let checkout = staging.path().join("checkout");
        let checkout_text = checkout
            .to_str()
            .ok_or_else(|| Error::UnsafePath("non-UTF-8 checkout path".into()))?;
        git::run_with_token(
            &repositories,
            &[
                "clone",
                "--quiet",
                "--depth=1",
                "--single-branch",
                "--no-tags",
                "--",
                &url,
                checkout_text,
            ],
            &url,
            token,
        )?;
        let destination = repositories.join(revision(url.as_bytes()));
        if destination.exists() {
            return Err(Error::Git(
                "unregistered checkout already exists; refusing to overwrite it".into(),
            ));
        }
        fs::rename(&checkout, &destination)?;
        let workspace = self.register(
            &destination.canonicalize()?,
            WorkspaceKind::Github,
            Some(&url),
        )?;
        self.refresh_inner(&workspace)?;
        Ok(workspace)
    }

    /// Local workspaces are refreshed; GitHub workspaces first fetch and fast-forward
    /// using their saved transport and trusted external authentication configuration.
    pub fn sync_workspace(&self, id: i64) -> Result<RefreshReport> {
        self.sync_workspace_with_token(id, None)
    }

    /// Fetch with optional HTTPS GitHub OAuth credentials, without persisting them
    /// or falling back to external helpers on rejection. Local workspaces ignore it.
    pub fn sync_workspace_with_token(&self, id: i64, token: Option<&str>) -> Result<RefreshReport> {
        let _guard = self.lock_mutation()?;
        let workspace = self.workspace(id)?;
        if workspace.kind == WorkspaceKind::Github {
            let root = self.root(&workspace)?;
            let url = git::validate_url(workspace.url.as_deref().unwrap_or(""))?;
            git::sync_with_token(&root, &url, token)?;
        }
        self.refresh_inner(&workspace)
    }

    pub fn refresh_workspace(&self, id: i64) -> Result<RefreshReport> {
        let _guard = self.lock_mutation()?;
        self.refresh_inner(&self.workspace(id)?)
    }

    fn refresh_inner(&self, workspace: &Workspace) -> Result<RefreshReport> {
        let root = self.root(workspace)?;
        let old: HashMap<_, _> = self
            .list_documents(workspace.id)?
            .into_iter()
            .map(|doc| (doc.path, doc.revision))
            .collect();
        let mut seen = HashSet::new();
        let mut changed = Vec::new();
        let mut report = RefreshReport::default();
        // Never follow directory symlinks. File symlinks are accepted only within root.
        let walker = walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || !paths::excluded(&entry.file_name().to_string_lossy())
            });
        for entry in walker {
            let entry = entry.map_err(|error| Error::Io(error.into()))?;
            if entry.file_type().is_dir() || !paths::markdown(entry.path()) {
                continue;
            }
            let path = paths::relative_path(
                entry
                    .path()
                    .strip_prefix(&root)
                    .map_err(|_| Error::UnsafePath(entry.path().display().to_string()))?,
            )?;
            let safe = match paths::safe_file(&root, &path) {
                Ok(safe) => safe,
                Err(Error::UnsafePath(_)) => continue,
                // Broken links are not documents; remove any stale index entry.
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if !paths::markdown(&safe) {
                continue;
            }
            let bytes = paths::read_limited(&safe, MAX_DOCUMENT_BYTES)?;
            let hash = revision(&bytes);
            seen.insert(path.clone());
            if old.get(&path) == Some(&hash)
                && !search::needs_rebuild(&*lock(&self.db)?, workspace.id, &path)?
            {
                report.unchanged += 1;
                continue;
            }
            let source = String::from_utf8(bytes)?;
            let fallback = Path::new(&path)
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or(&path);
            let rendered = markdown::render(&source, fallback);
            changed.push((path, hash, rendered));
        }
        let mut db = lock(&self.db)?;
        let tx = db.transaction()?;
        for (path, hash, rendered) in changed {
            index_document(&tx, workspace.id, &path, &hash, &rendered)?;
            report.indexed += 1;
        }
        for path in old.keys().filter(|path| !seen.contains(*path)) {
            search::remove(&tx, workspace.id, path)?;
            tx.execute(
                "DELETE FROM documents WHERE workspace_id=?1 AND path=?2",
                params![workspace.id, path],
            )?;
            report.removed += 1;
        }
        tx.commit()?;
        Ok(report)
    }

    pub fn list_documents(&self, workspace_id: i64) -> Result<Vec<DocumentSummary>> {
        self.workspace(workspace_id)?;
        let db = lock(&self.db)?;
        let mut stmt = db.prepare(
            "SELECT path,title,revision FROM documents WHERE workspace_id=?1 ORDER BY path",
        )?;
        let rows = stmt.query_map([workspace_id], |row| {
            Ok(DocumentSummary {
                workspace_id,
                path: row.get(0)?,
                title: row.get(1)?,
                revision: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Read current filesystem content, not a potentially stale index snapshot.
    pub fn read_document(&self, workspace_id: i64, path: &str) -> Result<Document> {
        let workspace = self.workspace(workspace_id)?;
        let root = self.root(&workspace)?;
        let safe = paths::safe_file(&root, path)?;
        if !paths::markdown(Path::new(path)) || !paths::markdown(&safe) {
            return Err(Error::Unsupported(path.into()));
        }
        let bytes = paths::read_limited(&safe, MAX_DOCUMENT_BYTES)?;
        let hash = revision(&bytes);
        let content = String::from_utf8(bytes)?;
        Ok(make_document(workspace_id, path, content, hash))
    }

    /// Save an existing document with optimistic concurrency. The revision is a SHA-256
    /// of its exact bytes, not mtime. A conflict leaves both file and index untouched.
    /// Cooperating Core instances sharing a data directory are serialized. External
    /// editors are checked immediately before rename, but a filesystem cannot provide
    /// an atomic compare-and-swap against writers that do not take our lock.
    pub fn save_document(
        &self,
        workspace_id: i64,
        path: &str,
        content: &str,
        expected_revision: &str,
    ) -> Result<Document> {
        if content.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(Error::TooLarge(MAX_DOCUMENT_BYTES));
        }
        let _guard = self.lock_mutation()?;
        let workspace = self.workspace(workspace_id)?;
        let root = self.root(&workspace)?;
        let safe = paths::safe_file(&root, path)?;
        if !paths::markdown(Path::new(path)) || !paths::markdown(&safe) {
            return Err(Error::Unsupported(path.into()));
        }
        let verify = || -> Result<()> {
            if paths::safe_file(&root, path)? != safe {
                return Err(Error::UnsafePath(path.into()));
            }
            let actual = revision(&paths::read_limited(&safe, MAX_DOCUMENT_BYTES)?);
            if actual != expected_revision {
                return Err(Error::Conflict {
                    expected: expected_revision.into(),
                    actual,
                });
            }
            Ok(())
        };
        verify()?;
        let permissions = fs::metadata(&safe)?.permissions();
        let parent = safe
            .parent()
            .ok_or_else(|| Error::UnsafePath(path.into()))?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(content.as_bytes())?;
        temp.as_file().set_permissions(permissions)?;
        temp.as_file().sync_all()?;
        verify()?;
        temp.persist(&safe)
            .map_err(|error| Error::Io(error.error))?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        let hash = revision(content.as_bytes());
        let fallback = Path::new(path)
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or(path);
        let rendered = markdown::render(content, fallback);
        let mut aliases = Vec::new();
        for summary in self.list_documents(workspace_id)? {
            if summary.path != path
                && paths::safe_file(&root, &summary.path).ok().as_ref() == Some(&safe)
            {
                let alias_title = Path::new(&summary.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&summary.path);
                aliases.push((summary.path.clone(), markdown::render(content, alias_title)));
            }
        }
        let mut db = lock(&self.db)?;
        let tx = db.transaction()?;
        index_document(&tx, workspace_id, path, &hash, &rendered)?;
        for (alias, rendered) in aliases {
            index_document(&tx, workspace_id, &alias, &hash, &rendered)?;
        }
        tx.commit()?;
        Ok(Document {
            workspace_id,
            path: path.into(),
            title: rendered.title,
            revision: hash,
            content: content.into(),
            html: rendered.html,
            toc: rendered.toc,
        })
    }

    /// Literal AND search with phrases, final-token prefixes and bounded vocabulary typos.
    /// Returns at most 50 documents, each represented by its best section.
    pub fn search(&self, query: &str, workspace: Option<i64>) -> Result<Vec<SearchHit>> {
        if let Some(id) = workspace {
            self.workspace(id)?;
        }
        search::query(&*lock(&self.db)?, query, workspace)
    }

    /// Resolve a link relative to an existing Markdown document. Local targets must
    /// exist and remain inside this workspace; only HTTP(S)/mailto external URLs pass.
    pub fn resolve_link(&self, workspace_id: i64, path: &str, href: &str) -> Result<ResolvedLink> {
        let workspace = self.workspace(workspace_id)?;
        let root = self.root(&workspace)?;
        let source = paths::safe_file(&root, path)?;
        if !paths::markdown(&source) {
            return Err(Error::Unsupported(path.into()));
        }
        if href.starts_with("//") {
            return Err(Error::InvalidLink(href.into()));
        }
        if let Ok(url) = url::Url::parse(href) {
            if matches!(url.scheme(), "http" | "https" | "mailto")
                && url.username().is_empty()
                && url.password().is_none()
            {
                return Ok(ResolvedLink::External {
                    url: url.to_string(),
                });
            }
            return Err(Error::InvalidLink(href.into()));
        }
        // URL joining handles ../ and percent escapes; decoded paths are validated again.
        let base = url::Url::from_file_path(root.join(paths::relative(path)?))
            .map_err(|_| Error::InvalidLink(path.into()))?;
        let joined = base
            .join(href)
            .map_err(|_| Error::InvalidLink(href.into()))?;
        if joined.scheme() != "file" || joined.query().is_some() {
            return Err(Error::InvalidLink(href.into()));
        }
        let target = joined
            .to_file_path()
            .map_err(|_| Error::InvalidLink(href.into()))?;
        let target_path = paths::relative_path(
            target
                .strip_prefix(&root)
                .map_err(|_| Error::UnsafePath(href.into()))?,
        )?;
        let safe = paths::safe_file(&root, &target_path)?;
        let fragment = joined.fragment().map(str::to_owned);
        if paths::markdown(&safe) && paths::markdown(Path::new(&target_path)) {
            Ok(ResolvedLink::Document {
                path: target_path,
                fragment,
            })
        } else if paths::asset_mime(&safe).is_some()
            && paths::asset_mime(Path::new(&target_path)).is_some()
        {
            Ok(ResolvedLink::Asset {
                path: target_path,
                fragment,
            })
        } else {
            Err(Error::Unsupported(target_path))
        }
    }

    pub fn read_asset(&self, workspace_id: i64, path: &str) -> Result<Asset> {
        let workspace = self.workspace(workspace_id)?;
        let root = self.root(&workspace)?;
        let safe = paths::safe_file(&root, path)?;
        let mime_type =
            paths::asset_mime(Path::new(path)).ok_or_else(|| Error::Unsupported(path.into()))?;
        if paths::asset_mime(&safe) != Some(mime_type) {
            return Err(Error::Unsupported(path.into()));
        }
        let bytes = paths::read_limited(&safe, MAX_ASSET_BYTES)?;
        Ok(Asset {
            path: path.into(),
            mime_type: mime_type.into(),
            bytes,
        })
    }
}

fn make_document(workspace_id: i64, path: &str, content: String, revision: String) -> Document {
    let fallback = Path::new(path)
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or(path);
    let rendered = markdown::render(&content, fallback);
    Document {
        workspace_id,
        path: path.into(),
        content,
        revision,
        title: rendered.title,
        html: rendered.html,
        toc: rendered.toc,
    }
}

fn index_document(
    db: &Connection,
    workspace_id: i64,
    path: &str,
    hash: &str,
    rendered: &markdown::Rendered,
) -> Result<()> {
    search::index(db, workspace_id, path, hash, rendered)
}
