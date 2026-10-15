use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Local,
    Github,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: i64,
    pub kind: WorkspaceKind,
    pub name: String,
    /// Canonical absolute path. Treat this as metadata, not as an access capability.
    pub root: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub workspace_id: i64,
    /// Slash-separated, workspace-relative path.
    pub path: String,
    pub title: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TocEntry {
    pub level: u8,
    pub id: String,
    pub text: String,
    /// UTF-8 byte offset into the Markdown source.
    pub source_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub workspace_id: i64,
    pub path: String,
    pub title: String,
    pub revision: String,
    pub content: String,
    pub html: String,
    pub toc: Vec<TocEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub workspace_id: i64,
    pub path: String,
    pub title: String,
    /// Weighted section BM25 (lower is better), after deterministic ranking tiers.
    pub score: f64,
    /// UTF-8 source byte offset of the matching block (or section fallback), not the token.
    pub source_start: usize,
    /// Exclusive end of that block/section range; unavailable for migrated legacy caches.
    #[serde(default)]
    pub source_end: Option<usize>,
    /// Inherited heading path of the best matching section.
    #[serde(default)]
    pub section_title: String,
    /// Normalized query terms, including vocabulary-selected typo corrections.
    /// Hosts may highlight these in the plain-text snippet, with accent/case folding.
    #[serde(default)]
    pub matched_terms: Vec<String>,
    /// Plain text only; never HTML or FTS marker markup.
    pub snippet: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshReport {
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedLink {
    Document {
        path: String,
        fragment: Option<String>,
    },
    Asset {
        path: String,
        fragment: Option<String>,
    },
    External {
        url: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("workspace {0} not found")]
    WorkspaceNotFound(i64),
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    #[error("unsupported document or asset: {0}")]
    Unsupported(String),
    #[error("revision conflict (expected {expected}, actual {actual})")]
    Conflict { expected: String, actual: String },
    #[error("invalid GitHub URL: {0}")]
    InvalidGithubUrl(String),
    #[error("git failed: {0}")]
    Git(String),
    #[error(
        "Git authentication failed; configure a trusted credential helper or load your SSH key"
    )]
    GitAuthentication,
    #[error("GitHub repository is unavailable; check the repository name, account access, and organization authorization (private repositories may appear not found)")]
    GitPermissionDenied,
    #[error(
        "Git network or TLS connection failed; check connectivity, proxy, and trusted certificates"
    )]
    GitNetwork,
    #[error(
        "SSH host verification failed; verify GitHub's host key outside Mdwell before retrying"
    )]
    GitSshHostVerification,
    #[error("checkout is dirty; sync refused")]
    DirtyCheckout,
    #[error("file exceeds the {0}-byte limit")]
    TooLarge(u64),
    #[error("invalid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("invalid link: {0}")]
    InvalidLink(String),
    #[error("internal lock poisoned")]
    LockPoisoned,
}

/// Stable transport representation; OS/database error objects stay inside Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    Database,
    WorkspaceNotFound,
    UnsafePath,
    Unsupported,
    Conflict,
    InvalidGithubUrl,
    Git,
    GitAuthentication,
    GitPermissionDenied,
    GitNetwork,
    GitSshHostVerification,
    DirtyCheckout,
    TooLarge,
    Utf8,
    InvalidLink,
    LockPoisoned,
}

impl Error {
    pub fn info(&self) -> ErrorInfo {
        let code = match self {
            Self::Io(_) => ErrorCode::Io,
            Self::Database(_) => ErrorCode::Database,
            Self::WorkspaceNotFound(_) => ErrorCode::WorkspaceNotFound,
            Self::UnsafePath(_) => ErrorCode::UnsafePath,
            Self::Unsupported(_) => ErrorCode::Unsupported,
            Self::Conflict { .. } => ErrorCode::Conflict,
            Self::InvalidGithubUrl(_) => ErrorCode::InvalidGithubUrl,
            Self::Git(_) => ErrorCode::Git,
            Self::GitAuthentication => ErrorCode::GitAuthentication,
            Self::GitPermissionDenied => ErrorCode::GitPermissionDenied,
            Self::GitNetwork => ErrorCode::GitNetwork,
            Self::GitSshHostVerification => ErrorCode::GitSshHostVerification,
            Self::DirtyCheckout => ErrorCode::DirtyCheckout,
            Self::TooLarge(_) => ErrorCode::TooLarge,
            Self::Utf8(_) => ErrorCode::Utf8,
            Self::InvalidLink(_) => ErrorCode::InvalidLink,
            Self::LockPoisoned => ErrorCode::LockPoisoned,
        };
        let (expected_revision, actual_revision) = match self {
            Self::Conflict { expected, actual } => (Some(expected.clone()), Some(actual.clone())),
            _ => (None, None),
        };
        ErrorInfo {
            code,
            message: self.to_string(),
            expected_revision,
            actual_revision,
        }
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.info().serialize(serializer)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
