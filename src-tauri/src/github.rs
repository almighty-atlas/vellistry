//! Blocking native GitHub App authentication. Call network methods on spawn_blocking.
//! The caller opens the supplied authorization URL with the system browser; never a webview.
//! Register exactly http://127.0.0.1:PORT/callback with the GitHub App, where PORT is
//! the required compile-time MDWELL_GITHUB_CALLBACK_PORT. No dynamic-port fallback is used.
//! The HTTPS broker must keep the app secret server-side, enforce the configured client ID
//! and loopback redirect policy, and never log codes, verifiers, or tokens.

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use rand::{rngs::OsRng, RngCore};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(25);
const MAX_BODY: u64 = 2 * 1024 * 1024;
const MAX_PAGES: usize = 1000;
const MAX_AVATAR_BYTES: u64 = 64 * 1024;
const CANCELLED: &str = "GitHub sign-in cancelled.";

#[derive(Serialize)]
pub struct AccountStatus {
    pub connected: bool,
    pub configured: bool,
    pub login: Option<String>,
    /// A locally cached raster data URL; never a remote URL.
    pub avatar_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Repository {
    pub id: u64,
    pub full_name: String,
    pub private: bool,
    pub clone_url: String,
}

struct Config {
    client_id: String,
    token_url: Url,
    app_slug: String,
    callback_port: u16,
}

impl Config {
    fn compiled() -> Result<Self, String> {
        Self::parse(
            option_env!("MDWELL_GITHUB_CLIENT_ID"),
            option_env!("MDWELL_GITHUB_BROKER_URL"),
            option_env!("MDWELL_GITHUB_APP_SLUG"),
            option_env!("MDWELL_GITHUB_CALLBACK_PORT"),
        )
    }

    fn parse(
        id: Option<&str>,
        broker: Option<&str>,
        slug: Option<&str>,
        port: Option<&str>,
    ) -> Result<Self, String> {
        let required = |value: Option<&str>, name: &str| -> Result<String, String> {
            value
                .filter(|v| !v.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("Missing compile-time GitHub configuration: {name}."))
        };
        let client_id = required(id, "MDWELL_GITHUB_CLIENT_ID")?;
        let broker = required(broker, "MDWELL_GITHUB_BROKER_URL")?;
        let app_slug = required(slug, "MDWELL_GITHUB_APP_SLUG")?;
        let port = required(port, "MDWELL_GITHUB_CALLBACK_PORT")?;
        let callback_port = port.parse::<u16>().ok()
            .filter(|value| *value >= 1024 && port.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| "Invalid MDWELL_GITHUB_CALLBACK_PORT: expected an unprivileged decimal port from 1024 to 65535.".to_string())?;
        if !client_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !app_slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("Invalid GitHub client ID or app slug configuration.".into());
        }
        let mut token_url =
            Url::parse(&broker).map_err(|_| "Invalid GitHub broker URL.".to_string())?;
        if token_url.scheme() != "https"
            || token_url.host_str().is_none()
            || !token_url.username().is_empty()
            || token_url.password().is_some()
            || token_url.query().is_some()
            || token_url.fragment().is_some()
        {
            return Err(
                "GitHub broker must be an HTTPS URL without credentials, query, or fragment."
                    .into(),
            );
        }
        // A broker base such as https://host/github becomes https://host/github/token.
        let path = format!("{}/token", token_url.path().trim_end_matches('/'));
        token_url.set_path(&path);
        Ok(Self {
            client_id,
            token_url,
            app_slug,
            callback_port,
        })
    }
}

// These types intentionally do not implement Debug. Only the OS credential store persists them.
#[derive(Serialize, Deserialize)]
struct SavedToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    refresh_expires_at: Option<u64>,
    login: String,
    #[serde(default)]
    avatar_data_url: Option<String>,
}

impl Drop for SavedToken {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(token) = &mut self.refresh_token {
            token.zeroize();
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    refresh_token_expires_in: Option<u64>,
    token_type: Option<String>,
    error: Option<String>,
}

impl Drop for TokenResponse {
    fn drop(&mut self) {
        if let Some(token) = &mut self.access_token {
            token.zeroize();
        }
        if let Some(token) = &mut self.refresh_token {
            token.zeroize();
        }
    }
}

impl TokenResponse {
    fn into_saved(mut self, login: String, now: u64) -> Result<SavedToken, String> {
        if self.error.is_some() {
            return Err("GitHub broker rejected the token grant. Sign in again.".into());
        }
        let valid_secret =
            |s: &str| !s.is_empty() && s.len() <= 4096 && s.bytes().all(|b| b.is_ascii_graphic());
        if !self
            .token_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("bearer"))
            || !self.access_token.as_deref().is_some_and(valid_secret)
            || self
                .refresh_token
                .as_deref()
                .is_some_and(|t| !valid_secret(t))
            || self.expires_in == Some(0)
            || self.refresh_token_expires_in == Some(0)
            || (self.expires_in.is_some()
                && (self.refresh_token.is_none() || self.refresh_token_expires_in.is_none()))
        {
            return Err("GitHub broker returned an invalid token response.".into());
        }
        let expiry = |seconds: Option<u64>| -> Result<Option<u64>, String> {
            seconds
                .map(|s| {
                    now.checked_add(s)
                        .ok_or_else(|| "Invalid GitHub token expiry.".to_string())
                })
                .transpose()
        };
        Ok(SavedToken {
            access_token: self.access_token.take().unwrap_or_default(),
            refresh_token: self.refresh_token.take(),
            expires_at: expiry(self.expires_in)?,
            refresh_expires_at: expiry(self.refresh_token_expires_in)?,
            login,
            avatar_data_url: None,
        })
    }
}

trait Store: Send + Sync {
    fn load(&self, client_id: &str) -> Result<Option<SavedToken>, String>;
    fn save(&self, client_id: &str, token: &SavedToken) -> Result<(), String>;
    fn delete(&self, client_id: &str) -> Result<(), String>;
}

struct NativeStore;
#[cfg(not(target_os = "linux"))]
impl NativeStore {
    fn entry(client_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new("mdwell.github", client_id)
            .map_err(|_| "Native GitHub credential storage is unavailable.".into())
    }
}
#[cfg(not(target_os = "linux"))]
impl Store for NativeStore {
    fn load(&self, client_id: &str) -> Result<Option<SavedToken>, String> {
        match Self::entry(client_id)?.get_password() {
            Ok(json) => serde_json::from_str(&Zeroizing::new(json))
                .map(Some)
                .map_err(|_| {
                    "Stored GitHub credentials are invalid. Disconnect and sign in again.".into()
                }),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(
                "Cannot read native GitHub credential storage. Unlock your keychain and retry."
                    .into(),
            ),
        }
    }
    fn save(&self, client_id: &str, token: &SavedToken) -> Result<(), String> {
        let json = Zeroizing::new(
            serde_json::to_string(token)
                .map_err(|_| "Cannot encode GitHub credentials.".to_string())?,
        );
        Self::entry(client_id)?.set_password(&json).map_err(|_| {
            "Cannot save native GitHub credentials. Unlock your keychain and retry.".into()
        })
    }
    fn delete(&self, client_id: &str) -> Result<(), String> {
        match Self::entry(client_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(
                "Cannot delete native GitHub credentials. Unlock your keychain and retry.".into(),
            ),
        }
    }
}

#[cfg(target_os = "linux")]
impl Store for NativeStore {
    fn load(&self, client_id: &str) -> Result<Option<SavedToken>, String> {
        linux_store::load(client_id)
    }
    fn save(&self, client_id: &str, token: &SavedToken) -> Result<(), String> {
        linux_store::save(client_id, token)
    }
    fn delete(&self, client_id: &str) -> Result<(), String> {
        linux_store::delete(client_id)
    }
}

#[cfg(target_os = "linux")]
mod linux_store {
    use super::*;
    use dbus_secret_service::{Collection, EncryptionType, Item, SecretService};
    use std::{collections::HashMap, sync::mpsc};

    const LOCKED: &str = "GitHub credential storage is locked or requires a prompt. Unlock your desktop keyring outside mdwell and retry.";
    const UNAVAILABLE: &str = "GitHub Secret Service is unavailable or timed out. Check your desktop keyring outside mdwell and retry.";
    static CONNECTING: AtomicBool = AtomicBool::new(false);

    fn error(error: dbus_secret_service::Error) -> String {
        match error {
            dbus_secret_service::Error::Locked | dbus_secret_service::Error::Prompt => {
                LOCKED.into()
            }
            _ => UNAVAILABLE.into(),
        }
    }

    fn connect() -> Result<SecretService, String> {
        // Secret Service methods in dbus-secret-service 4.1 use 2-second DBus timeouts.
        // Bus connection/registration is a separate libdbus operation: bound the caller's
        // wait too. A late connection may finish, but this worker never reads/writes secrets.
        // Allow at most one unfinished connector, rather than accumulating hung workers.
        if CONNECTING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(UNAVAILABLE.into());
        }
        let (send, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("mdwell-keyring-connect".into())
            .spawn(move || {
                struct Connecting;
                impl Drop for Connecting {
                    fn drop(&mut self) {
                        CONNECTING.store(false, Ordering::SeqCst);
                    }
                }
                let _connecting = Connecting;
                // DH encrypts secret transfer; zero disables Prompt execution, not just its wait.
                let service = SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 0);
                let _ = send.send(service);
            });
        if worker.is_err() {
            CONNECTING.store(false, Ordering::SeqCst);
            return Err(UNAVAILABLE.into());
        }
        receive
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| UNAVAILABLE.to_string())?
            .map_err(error)
    }

    fn require_unlocked(locked: bool) -> Result<(), String> {
        if locked {
            Err(LOCKED.into())
        } else {
            Ok(())
        }
    }

    fn collection(service: &SecretService) -> Result<Collection<'_>, String> {
        // Never create/unlock a collection or fall back to a session-only collection.
        let collection = service.get_default_collection().map_err(error)?;
        require_unlocked(collection.is_locked().map_err(error)?)?;
        Ok(collection)
    }

    fn attributes(client_id: &str) -> HashMap<&str, &str> {
        // Match existing keyring v3 entries, preserving credentials from older mdwell builds.
        HashMap::from([
            ("service", "mdwell.github"),
            ("username", client_id),
            ("target", "default"),
        ])
    }

    fn item<'a>(
        service: &'a SecretService,
        collection: &'a Collection<'a>,
        client_id: &str,
    ) -> Result<Option<Item<'a>>, String> {
        let found = service.search_items(attributes(client_id)).map_err(error)?;
        if !found.locked.is_empty() {
            return Err(LOCKED.into());
        }
        let mut items = found.unlocked;
        if items.is_empty() {
            // Match keyring's legacy v1 fallback, only in the already-unlocked default collection.
            items = collection
                .search_items(HashMap::from([
                    ("service", "mdwell.github"),
                    ("username", client_id),
                ]))
                .map_err(error)?;
        }
        if items.len() > 1 {
            return Err("Multiple GitHub keyring entries match. Resolve them in your desktop keyring outside mdwell.".into());
        }
        let item = items.pop();
        if let Some(item) = &item {
            require_unlocked(item.is_locked().map_err(error)?)?;
        }
        Ok(item)
    }

    pub(super) fn load(client_id: &str) -> Result<Option<SavedToken>, String> {
        let service = connect()?;
        let collection = collection(&service)?;
        let Some(item) = item(&service, &collection, client_id)? else {
            return Ok(None);
        };
        // get_secret/set_secret do not unlock. A lock race fails, rather than prompting.
        let bytes = Zeroizing::new(item.get_secret().map_err(error)?);
        serde_json::from_slice(&bytes).map(Some).map_err(|_| {
            "Stored GitHub credentials are invalid. Disconnect and sign in again.".into()
        })
    }

    pub(super) fn save(client_id: &str, token: &SavedToken) -> Result<(), String> {
        let service = connect()?;
        let collection = collection(&service)?;
        let json = Zeroizing::new(
            serde_json::to_vec(token)
                .map_err(|_| "Cannot encode GitHub credentials.".to_string())?,
        );
        if let Some(item) = item(&service, &collection, client_id)? {
            item.set_secret(&json, "text/plain").map_err(error)
        } else {
            let mut attributes = attributes(client_id);
            attributes.insert("application", "rust-keyring");
            collection
                .create_item(
                    "mdwell GitHub account",
                    attributes,
                    &json,
                    true,
                    "text/plain",
                )
                .map(|_| ())
                .map_err(error)
        }
    }

    pub(super) fn delete(client_id: &str) -> Result<(), String> {
        let service = connect()?;
        let collection = collection(&service)?;
        match item(&service, &collection, client_id)? {
            Some(item) => item.delete().map_err(error),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    #[test]
    fn locked_and_prompt_errors_require_external_unlock() {
        assert!(require_unlocked(true)
            .unwrap_err()
            .contains("outside mdwell"));
        assert!(require_unlocked(false).is_ok());
        assert_eq!(error(dbus_secret_service::Error::Prompt), LOCKED);
        assert_eq!(error(dbus_secret_service::Error::Locked), LOCKED);
        assert_eq!(error(dbus_secret_service::Error::Unavailable), UNAVAILABLE);
    }
}

trait Transport: Send + Sync {
    fn token(&self, url: &Url, fields: &[(&str, &str)]) -> Result<TokenResponse, String>;
    fn get(&self, path: &str, access_token: &str) -> Result<serde_json::Value, String>;
    fn avatar(&self, url: &Url) -> Result<Avatar, String>;
}

struct Avatar {
    content_type: String,
    bytes: Vec<u8>,
}

impl Avatar {
    fn data_url(self) -> Result<String, String> {
        if self.bytes.len() as u64 > MAX_AVATAR_BYTES
            || !raster_matches(&self.content_type, &self.bytes)
        {
            return Err("GitHub avatar is not a supported bounded raster image.".into());
        }
        Ok(format!(
            "data:{};base64,{}",
            self.content_type,
            STANDARD.encode(self.bytes)
        ))
    }
}

fn raster_matches(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

fn validated_avatar_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "Invalid GitHub avatar URL.".to_string())?;
    if url.scheme() != "https"
        || url.host_str() != Some("avatars.githubusercontent.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || url.fragment().is_some()
    {
        return Err("Invalid GitHub avatar URL.".into());
    }
    Ok(url)
}

fn cached_avatar(value: &str) -> Option<&str> {
    // Validate persisted data too: old/corrupt credentials must never produce a remote UI URL.
    if value.len() > 64 + (MAX_AVATAR_BYTES as usize).div_ceil(3) * 4 {
        return None;
    }
    let (mime, encoded) = value.strip_prefix("data:")?.split_once(";base64,")?;
    let bytes = STANDARD.decode(encoded).ok()?;
    (bytes.len() as u64 <= MAX_AVATAR_BYTES && raster_matches(mime, &bytes)).then_some(value)
}

fn response_avatar(response: reqwest::blocking::Response) -> Result<Avatar, String> {
    if !response.status().is_success() {
        return Err("GitHub avatar request failed.".into());
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| {
            matches!(
                v.as_str(),
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            )
        })
        .ok_or_else(|| "Unsupported GitHub avatar content type.".to_string())?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_AVATAR_BYTES)
    {
        return Err("GitHub avatar exceeded the size limit.".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_AVATAR_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read GitHub avatar.".to_string())?;
    if bytes.len() as u64 > MAX_AVATAR_BYTES || !raster_matches(&content_type, &bytes) {
        return Err("GitHub avatar is not a supported bounded raster image.".into());
    }
    Ok(Avatar {
        content_type,
        bytes,
    })
}
struct Http {
    client: Result<Client, String>,
    // Separate client: no credential headers or cookie store, even if API setup changes later.
    avatar_client: Result<Client, String>,
}
impl Http {
    fn new() -> Self {
        Self {
            client: Client::builder()
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .user_agent("mdwell-desktop/0.1")
                .build()
                .map_err(|_| "Cannot initialize GitHub HTTPS client.".into()),
            avatar_client: Client::builder()
                .https_only(true)
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(5))
                .user_agent("mdwell-desktop/0.1")
                .build()
                .map_err(|_| "Cannot initialize GitHub avatar client.".into()),
        }
    }
    fn client(&self) -> Result<&Client, String> {
        self.client.as_ref().map_err(Clone::clone)
    }
    fn avatar_request(&self, url: &Url) -> Result<reqwest::blocking::RequestBuilder, String> {
        let url = validated_avatar_url(url.as_str())?;
        Ok(self
            .avatar_client
            .as_ref()
            .map_err(Clone::clone)?
            .get(url)
            .header("Accept", "image/png, image/jpeg, image/gif, image/webp"))
    }
}
fn response_json<T: DeserializeOwned>(response: reqwest::blocking::Response) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        // Never include response bodies, URLs, or underlying library errors: they may contain secrets.
        return Err(match status.as_u16() {
            401 => "GitHub authentication was rejected. Sign in again.".into(),
            403 | 429 => {
                "GitHub request denied or rate limited. Check app permissions or retry later."
                    .into()
            }
            _ => format!("GitHub service request failed (HTTP {}).", status.as_u16()),
        });
    }
    if response.content_length().is_some_and(|n| n > MAX_BODY) {
        return Err("GitHub response exceeded the size limit.".into());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read GitHub service response.".to_string())?;
    if bytes.len() as u64 > MAX_BODY {
        return Err("GitHub response exceeded the size limit.".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "GitHub service returned an invalid response.".into())
}
impl Transport for Http {
    fn avatar(&self, url: &Url) -> Result<Avatar, String> {
        response_avatar(
            self.avatar_request(url)?
                .send()
                .map_err(|_| "GitHub avatar is unreachable or timed out.".to_string())?,
        )
    }
    fn token(&self, url: &Url, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
        let fields: BTreeMap<_, _> = fields.iter().copied().collect();
        response_json(
            self.client()?
                .post(url.clone())
                .header("Accept", "application/json")
                .json(&fields)
                .send()
                .map_err(|_| "GitHub token broker is unreachable or timed out.".to_string())?,
        )
    }
    fn get(&self, path: &str, access_token: &str) -> Result<serde_json::Value, String> {
        // Paths are constructed locally, never from pagination links or server-provided URLs.
        response_json(
            self.client()?
                .get(format!("https://api.github.com{path}"))
                .bearer_auth(access_token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .map_err(|_| "GitHub API is unreachable or timed out.".to_string())?,
        )
    }
}

#[derive(Default)]
struct CredentialState {
    cleanup_required: bool,
}
impl CredentialState {
    fn ensure_clean(&self) -> Result<(), String> {
        if self.cleanup_required {
            Err("Cancelled GitHub credentials could not be cleaned up. Unlock your keyring outside mdwell, then disconnect or sign in again.".into())
        } else {
            Ok(())
        }
    }
}

pub struct Github {
    config: Result<Config, String>,
    store: Box<dyn Store>,
    transport: Box<dyn Transport>,
    // Serializes credential I/O, refresh, disconnect, and final sign-in commit.
    credentials: Mutex<CredentialState>,
    pending: AtomicBool,
    generation: AtomicU64,
}
impl Default for Github {
    fn default() -> Self {
        Self::new()
    }
}
impl Github {
    pub fn new() -> Self {
        Self {
            config: Config::compiled(),
            store: Box::new(NativeStore),
            transport: Box::new(Http::new()),
            credentials: Mutex::new(CredentialState::default()),
            pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        }
    }
    fn config(&self) -> Result<&Config, String> {
        self.config.as_ref().map_err(Clone::clone)
    }
    fn lock(&self) -> Result<MutexGuard<'_, CredentialState>, String> {
        self.credentials
            .lock()
            .map_err(|_| "GitHub credential operation failed. Restart the application.".into())
    }
    fn sign_in_lock(&self, generation: u64) -> Result<MutexGuard<'_, CredentialState>, String> {
        loop {
            self.check_generation(generation)?;
            match self.credentials.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(std::sync::TryLockError::WouldBlock) => thread::sleep(POLL),
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(
                        "GitHub credential operation failed. Restart the application.".into(),
                    )
                }
            }
        }
    }
    fn check_generation(&self, generation: u64) -> Result<(), String> {
        if self.generation.load(Ordering::SeqCst) != generation {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }
    fn account(&self, token: Option<&SavedToken>) -> AccountStatus {
        AccountStatus {
            connected: token.is_some(),
            configured: self.config.is_ok(),
            login: token.map(|t| t.login.clone()),
            avatar_url: token
                .and_then(|t| t.avatar_data_url.as_deref())
                .and_then(cached_avatar)
                .map(str::to_owned),
        }
    }
    /// Offline: reports locally saved authorization, not current server validity.
    pub fn status(&self) -> Result<AccountStatus, String> {
        let Ok(config) = &self.config else {
            return Ok(self.account(None));
        };
        let guard = self.lock()?;
        guard.ensure_clean()?;
        Ok(self.account(self.store.load(&config.client_id)?.as_ref()))
    }
    pub fn start(
        &self,
        open: impl FnOnce(String) -> Result<(), String>,
    ) -> Result<AccountStatus, String> {
        let config = self.config()?;
        let generation = self.generation.load(Ordering::SeqCst);
        if self.pending.swap(true, Ordering::SeqCst) {
            return Err("GitHub sign-in is already in progress.".into());
        }
        struct Pending<'a>(&'a AtomicBool);
        impl Drop for Pending<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _pending = Pending(&self.pending);
        let port = config.callback_port;
        // Establish the exact registered callback endpoint before opening the browser.
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AddrInUse {
                    format!("GitHub callback port {port} is busy. Close the application using it and retry; no fallback port is used.")
                } else {
                    format!("Cannot bind GitHub callback listener at 127.0.0.1:{port}. Check local network permissions and retry.")
                }
            })?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "Cannot initialize GitHub callback listener.".to_string())?;
        let redirect = format!("http://127.0.0.1:{port}/callback");
        let verifier = Zeroizing::new(random_secret()?);
        let state = Zeroizing::new(random_secret()?);
        let mut url = Url::parse("https://github.com/login/oauth/authorize")
            .map_err(|_| "Invalid GitHub authorization URL.".to_string())?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("state", state.as_str()),
            ("code_challenge", challenge(&verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("scope", "offline_access"),
        ]);
        self.check_generation(generation)?;
        open(url.into())
            .map_err(|_| "Cannot open the system browser for GitHub sign-in.".to_string())?;
        let deadline = Instant::now() + CALLBACK_TIMEOUT;
        let code = Zeroizing::new(wait_callback(&listener, port, &state, deadline, || {
            self.check_generation(generation)
        })?);
        self.check_generation(generation)?;
        let response = self.transport.token(
            &config.token_url,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &config.client_id),
                ("code", &code),
                ("code_verifier", &verifier),
                ("redirect_uri", &redirect),
            ],
        )?;
        self.check_generation(generation)?;
        let mut token = response.into_saved(String::new(), now()?)?;
        let profile = self.transport.get("/user", &token.access_token)?;
        token.login = profile
            .get("login")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty() && s.len() <= 100)
            .ok_or_else(|| "GitHub returned an invalid account profile.".to_string())?
            .to_owned();
        self.check_generation(generation)?;
        token.avatar_data_url = profile
            .get("avatar_url")
            .and_then(|v| v.as_str())
            .and_then(|value| validated_avatar_url(value).ok())
            .and_then(|url| self.transport.avatar(&url).ok())
            .and_then(|avatar| avatar.data_url().ok());
        self.commit_sign_in(&config.client_id, token, generation)
    }
    fn commit_sign_in(
        &self,
        client_id: &str,
        mut token: SavedToken,
        generation: u64,
    ) -> Result<AccountStatus, String> {
        let mut guard = self.sign_in_lock(generation)?;
        let mut previous = self.store.load(client_id)?;
        self.check_generation(generation)?;
        let saved = self.save_profile(client_id, &mut token);
        if self.check_generation(generation).is_err() {
            // Keep the credential mutex through rollback: no newer sign-in, refresh, or
            // disconnect can write here and then be accidentally removed by this cleanup.
            // A failed write can have taken effect (e.g. a lost DBus reply), so roll back
            // even on a storage error. Cancellation linearizes at this final generation check.
            let cleanup = match previous.as_mut() {
                Some(previous) => self.save_profile(client_id, previous),
                None => self.store.delete(client_id),
            };
            if cleanup.is_err() {
                // Fail closed in this process if secure storage cannot undo the write.
                guard.cleanup_required = true;
                return Err("GitHub sign-in cancelled, but secure credential cleanup failed. Unlock your keyring outside mdwell and disconnect before restarting.".into());
            }
            return Err(CANCELLED.into());
        }
        saved?;
        guard.cleanup_required = false;
        Ok(self.account(Some(&token)))
    }
    /// Cancels pending sign-in, but does not disconnect an existing account.
    pub fn cancel(&self) -> Result<(), String> {
        // Never wait for keychain I/O or the refresh mutex to invalidate a pending sign-in.
        self.generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    /// Local disconnect only; revoke authorization at GitHub to invalidate issued tokens.
    pub fn disconnect(&self) -> Result<(), String> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.lock()?;
        self.store.delete(&self.config()?.client_id)?;
        guard.cleanup_required = false;
        Ok(())
    }
    fn save_profile(&self, client_id: &str, token: &mut SavedToken) -> Result<(), String> {
        if let Err(error) = self.store.save(client_id, token) {
            // Some native credential stores have small entry limits. An optional avatar must
            // not prevent securely saving tokens; retry without it, never outside the keyring.
            if token.avatar_data_url.take().is_none() {
                return Err(error);
            }
            self.store.save(client_id, token)?;
        }
        Ok(())
    }
    pub fn access_token(&self) -> Result<Option<String>, String> {
        let Ok(config) = &self.config else {
            return Ok(None);
        };
        let guard = self.lock()?;
        guard.ensure_clean()?;
        let Some(mut token) = self.store.load(&config.client_id)? else {
            return Ok(None);
        };
        let time = now()?;
        if token
            .expires_at
            .is_some_and(|expiry| expiry <= time.saturating_add(60))
        {
            let refresh = token
                .refresh_token
                .as_deref()
                .filter(|_| token.refresh_expires_at.is_some_and(|expiry| expiry > time))
                .ok_or_else(|| "GitHub authorization expired. Sign in again.".to_string())?;
            let response = self.transport.token(
                &config.token_url,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &config.client_id),
                    ("refresh_token", refresh),
                ],
            )?;
            let mut refreshed = response.into_saved(token.login.clone(), now()?)?;
            refreshed.avatar_data_url = token.avatar_data_url.take();
            token = refreshed;
            // Persist rotated credentials before returning them. No plaintext or in-memory fallback.
            self.save_profile(&config.client_id, &mut token)?;
        }
        Ok(Some(token.access_token.clone()))
    }
    pub fn repositories(&self) -> Result<Vec<Repository>, String> {
        let access_token = Zeroizing::new(
            self.access_token()?
                .ok_or_else(|| "Connect a GitHub account first.".to_string())?,
        );
        let mut repositories = BTreeMap::new();
        for installation in self.pages("/user/installations", "installations", &access_token)? {
            let id = installation
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "GitHub returned an invalid installation.".to_string())?;
            for value in self.pages(
                &format!("/user/installations/{id}/repositories"),
                "repositories",
                &access_token,
            )? {
                let repository: Repository = serde_json::from_value(value)
                    .map_err(|_| "GitHub returned an invalid repository.".to_string())?;
                let clone = Url::parse(&repository.clone_url)
                    .map_err(|_| "Invalid GitHub clone URL.".to_string())?;
                if clone.scheme() != "https"
                    || clone.host_str() != Some("github.com")
                    || !clone.username().is_empty()
                    || clone.password().is_some()
                    || clone.query().is_some()
                    || clone.fragment().is_some()
                    || clone.port().is_some()
                {
                    return Err("Invalid GitHub clone URL.".into());
                }
                repositories.insert(repository.id, repository);
            }
        }
        Ok(repositories.into_values().collect())
    }
    fn pages(
        &self,
        path: &str,
        field: &str,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut result = Vec::new();
        for page in 1..=MAX_PAGES {
            let value = self
                .transport
                .get(&format!("{path}?per_page=100&page={page}"), token)?;
            let items = value
                .get(field)
                .and_then(|v| v.as_array())
                .ok_or_else(|| "GitHub returned an invalid paginated response.".to_string())?;
            if items.len() > 100 {
                return Err("GitHub returned an invalid page size.".into());
            }
            result.extend(items.iter().cloned());
            if items.len() < 100 {
                return Ok(result);
            }
        }
        Err("GitHub pagination exceeded the safety limit.".into())
    }
    pub fn management_url(&self) -> Result<String, String> {
        Ok(format!(
            "https://github.com/apps/{}/installations/new",
            self.config()?.app_slug
        ))
    }
}

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| "System clock is invalid for GitHub authentication.".into())
}
fn random_secret() -> Result<String, String> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    OsRng
        .try_fill_bytes(bytes.as_mut())
        .map_err(|_| "Secure randomness is unavailable.".to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes.as_ref()))
}
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

enum Callback {
    Code(String),
    Denied,
}
fn parse_callback(header: &str, port: u16, expected_state: &str) -> Option<Callback> {
    let mut lines = header.split("\r\n");
    let mut request = lines.next()?.split_whitespace();
    if request.next()? != "GET" {
        return None;
    }
    let target = request.next()?;
    if request.next()? != "HTTP/1.1"
        || request.next().is_some()
        || !target.starts_with("/callback?")
    {
        return None;
    }
    let hosts: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .collect();
    if hosts.len() != 1 || hosts[0].1.trim() != format!("127.0.0.1:{port}") {
        return None;
    }
    let url = Url::parse(&format!("http://127.0.0.1:{port}{target}")).ok()?;
    if url.path() != "/callback" || url.fragment().is_some() {
        return None;
    }
    let params: Vec<_> = url.query_pairs().collect();
    let single = |key: &str| -> Option<&str> {
        let mut matches = params.iter().filter(|(k, _)| k == key);
        let value = matches.next()?.1.as_ref();
        if matches.next().is_some() {
            None
        } else {
            Some(value)
        }
    };
    let state = single("state")?;
    if !bool::from(state.as_bytes().ct_eq(expected_state.as_bytes())) {
        return None;
    }
    let codes = params.iter().filter(|(k, _)| k == "code").count();
    let errors = params.iter().filter(|(k, _)| k == "error").count();
    if codes == 0 && errors == 1 {
        return Some(Callback::Denied);
    }
    if codes != 1 || errors != 0 {
        return None;
    }
    let code = single("code")?;
    if code.is_empty() || code.len() > 4096 || !code.bytes().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    Some(Callback::Code(code.to_owned()))
}

fn wait_callback(
    listener: &TcpListener,
    port: u16,
    state: &str,
    deadline: Instant,
    check: impl Fn() -> Result<(), String>,
) -> Result<String, String> {
    loop {
        check()?;
        if Instant::now() >= deadline {
            return Err("GitHub sign-in timed out. Try again.".into());
        }
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                let header = read_callback(&mut stream, deadline, &check)?;
                let callback = header
                    .as_deref()
                    .and_then(|h| parse_callback(h, port, state));
                let (status, body) = if callback.is_some() {
                    (
                        "200 OK",
                        "GitHub callback received. You can close this window and return to mdwell.",
                    )
                } else {
                    ("400 Bad Request", "Invalid GitHub callback.")
                };
                // Fixed response, never reflect the code/state or load any external content.
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\n\r\n{body}", body.len());
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                let _ = stream.write_all(response.as_bytes());
                match callback {
                    Some(Callback::Code(code)) => {
                        check()?;
                        return Ok(code);
                    }
                    Some(Callback::Denied) => return Err("GitHub authorization was denied.".into()),
                    None => continue,
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => return Err("GitHub callback listener failed.".into()),
        }
    }
}
fn read_callback(
    stream: &mut TcpStream,
    deadline: Instant,
    check: &impl Fn() -> Result<(), String>,
) -> Result<Option<Zeroizing<String>>, String> {
    stream
        .set_nonblocking(true)
        .map_err(|_| "Cannot read GitHub callback.".to_string())?;
    let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
    let mut bytes = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 1024]);
    while bytes.len() < 8192 && Instant::now() < deadline {
        check()?;
        match stream.read(chunk.as_mut()) {
            Ok(0) => return Ok(None),
            Ok(count) => {
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    return Ok(std::str::from_utf8(&bytes[..end + 4])
                        .ok()
                        .map(|s| Zeroizing::new(s.to_owned())));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => return Ok(None),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, Arc};

    #[derive(Default)]
    struct MemoryStore {
        json: Mutex<Option<String>>,
        fail_save: AtomicBool,
        fail_avatar: AtomicBool,
    }
    impl Store for Arc<MemoryStore> {
        fn load(&self, _: &str) -> Result<Option<SavedToken>, String> {
            self.json
                .lock()
                .unwrap()
                .as_deref()
                .map(|s| serde_json::from_str(s).map_err(|_| "bad store".into()))
                .transpose()
        }
        fn save(&self, _: &str, token: &SavedToken) -> Result<(), String> {
            if self.fail_save.load(Ordering::SeqCst)
                || (self.fail_avatar.load(Ordering::SeqCst) && token.avatar_data_url.is_some())
            {
                return Err("storage unavailable".into());
            }
            *self.json.lock().unwrap() = Some(serde_json::to_string(token).unwrap());
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            *self.json.lock().unwrap() = None;
            Ok(())
        }
    }
    struct MockTransport {
        refreshes: AtomicUsize,
        fail: AtomicBool,
        paths: Mutex<Vec<String>>,
        avatar_fail: AtomicBool,
        avatar_url: Mutex<String>,
        avatars: AtomicUsize,
    }
    impl Default for MockTransport {
        fn default() -> Self {
            Self {
                refreshes: AtomicUsize::new(0),
                fail: AtomicBool::new(false),
                paths: Mutex::new(Vec::new()),
                avatar_fail: AtomicBool::new(false),
                avatar_url: Mutex::new("https://avatars.githubusercontent.com/u/1?v=4".into()),
                avatars: AtomicUsize::new(0),
            }
        }
    }
    impl Transport for Arc<MockTransport> {
        fn avatar(&self, url: &Url) -> Result<Avatar, String> {
            assert_eq!(url.host_str(), Some("avatars.githubusercontent.com"));
            self.avatars.fetch_add(1, Ordering::SeqCst);
            if self.avatar_fail.load(Ordering::SeqCst) {
                return Err("avatar unavailable".into());
            }
            Ok(Avatar {
                content_type: "image/png".into(),
                bytes: png(),
            })
        }
        fn token(&self, _: &Url, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("broker unavailable".into());
            }
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            assert!(fields.iter().any(|(k, _)| *k == "client_id"));
            Ok(serde_json::from_value(
                serde_json::json!({"access_token":"new-access", "refresh_token":"new-refresh",
                "token_type":"bearer", "expires_in":3600, "refresh_token_expires_in":7200}),
            )
            .unwrap())
        }
        fn get(&self, path: &str, _: &str) -> Result<serde_json::Value, String> {
            self.paths.lock().unwrap().push(path.into());
            if path == "/user" {
                return Ok(
                    serde_json::json!({"login":"octocat", "avatar_url": self.avatar_url.lock().unwrap().clone()}),
                );
            }
            if path.starts_with("/user/installations?") {
                return Ok(serde_json::json!({"installations": [{"id": 1}, {"id":2}]}));
            }
            let repo = serde_json::json!({"id": 7, "full_name":"owner/repo", "private":true, "clone_url":"https://github.com/owner/repo.git"});
            let items = if path.contains("/1/") && path.ends_with("page=1") {
                vec![repo; 100]
            } else if path.ends_with("page=2") {
                vec![]
            } else {
                vec![repo]
            };
            Ok(serde_json::json!({"repositories":items}))
        }
    }
    struct BlockedStore {
        memory: Arc<MemoryStore>,
        block_load: AtomicBool,
        block_save: AtomicBool,
        fail_delete: AtomicBool,
        fail_after_save: AtomicBool,
        entered: std::sync::mpsc::SyncSender<()>,
        resume: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl BlockedStore {
        fn pause(&self, flag: &AtomicBool) -> Result<(), String> {
            if flag.swap(false, Ordering::SeqCst) {
                self.entered
                    .send(())
                    .map_err(|_| "test worker disconnected".to_string())?;
                self.resume
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| "test storage timed out".to_string())?;
            }
            Ok(())
        }
    }
    impl Store for Arc<BlockedStore> {
        fn load(&self, client_id: &str) -> Result<Option<SavedToken>, String> {
            self.pause(&self.block_load)?;
            self.memory.load(client_id)
        }
        fn save(&self, client_id: &str, token: &SavedToken) -> Result<(), String> {
            self.pause(&self.block_save)?;
            self.memory.save(client_id, token)?;
            if self.fail_after_save.swap(false, Ordering::SeqCst) {
                Err("test lost storage reply after write".into())
            } else {
                Ok(())
            }
        }
        fn delete(&self, client_id: &str) -> Result<(), String> {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err("test cleanup unavailable".into());
            }
            self.memory.delete(client_id)
        }
    }
    fn blocked_fixture(
        block_load: bool,
        block_save: bool,
    ) -> (
        Arc<Github>,
        Arc<BlockedStore>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::SyncSender<()>,
    ) {
        let (mut github, memory, _) = fixture();
        let (entered_send, entered_receive) = std::sync::mpsc::sync_channel(1);
        let (resume_send, resume_receive) = std::sync::mpsc::sync_channel(1);
        let store = Arc::new(BlockedStore {
            memory,
            block_load: AtomicBool::new(block_load),
            block_save: AtomicBool::new(block_save),
            fail_delete: AtomicBool::new(false),
            fail_after_save: AtomicBool::new(false),
            entered: entered_send,
            resume: Mutex::new(resume_receive),
        });
        github.store = Box::new(store.clone());
        (Arc::new(github), store, entered_receive, resume_send)
    }

    #[test]
    fn cancel_is_prompt_while_keyring_load_holds_credentials_mutex() {
        let (github, store, entered, resume) = blocked_fixture(true, false);
        let reader = github.clone();
        let status = thread::spawn(move || reader.status());
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        let (opened_send, opened_receive) = std::sync::mpsc::sync_channel(1);
        let (result_send, result_receive) = std::sync::mpsc::sync_channel(1);
        let signer = github.clone();
        let sign_in = thread::spawn(move || {
            let result = signer.start(|url| {
                opened_send.send(()).unwrap();
                callback_browser(url)
            });
            result_send.send(result).unwrap();
        });
        opened_receive.recv_timeout(Duration::from_secs(1)).unwrap();
        let begin = Instant::now();
        github.cancel().unwrap();
        assert!(begin.elapsed() < Duration::from_millis(100));
        // The canceled sign-in must finish even while the fake keyring remains blocked.
        let result = result_receive.recv_timeout(Duration::from_millis(500));
        resume.send(()).unwrap();
        assert_eq!(result.unwrap().err().unwrap(), CANCELLED);
        status.join().unwrap().unwrap();
        sign_in.join().unwrap();
        assert!(store.memory.load("client").unwrap().is_none());
    }

    #[test]
    fn cancellation_during_save_removes_new_tokens_or_restores_previous_account() {
        for (had_previous, lost_reply) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let (github, store, entered, resume) = blocked_fixture(false, true);
            store.fail_after_save.store(lost_reply, Ordering::SeqCst);
            if had_previous {
                store
                    .memory
                    .save("client", &saved(now().unwrap() + 3600))
                    .unwrap();
            }
            let signer = github.clone();
            let sign_in = thread::spawn(move || {
                let mut token = saved(now().unwrap() + 3600);
                token.access_token = "cancelled-account-access".into();
                token.login = "cancelled-account".into();
                signer.commit_sign_in("client", token, 0)
            });
            entered.recv_timeout(Duration::from_secs(1)).unwrap();
            let begin = Instant::now();
            github.cancel().unwrap();
            assert!(begin.elapsed() < Duration::from_millis(100));
            resume.send(()).unwrap();
            assert_eq!(sign_in.join().unwrap().err().unwrap(), CANCELLED);
            let token = store.memory.load("client").unwrap();
            assert_eq!(token.is_some(), had_previous);
            if let Some(token) = token {
                assert_eq!(token.access_token, "old-access");
                assert_eq!(token.login, "octocat");
            }
        }
    }

    #[test]
    fn cancelled_save_cleanup_cannot_delete_a_newer_sign_in() {
        let (github, store, entered, resume) = blocked_fixture(false, true);
        let old_signer = github.clone();
        let old = thread::spawn(move || {
            old_signer.commit_sign_in("client", saved(now().unwrap() + 3600), 0)
        });
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        github.cancel().unwrap();
        let new_signer = github.clone();
        let generation = github.generation.load(Ordering::SeqCst);
        let (done_send, done_receive) = std::sync::mpsc::sync_channel(1);
        let new = thread::spawn(move || {
            let mut token = saved(now().unwrap() + 3600);
            token.access_token = "newer-account-access".into();
            let result = new_signer.commit_sign_in("client", token, generation);
            done_send.send(result).unwrap();
        });
        let early = done_receive.recv_timeout(Duration::from_millis(50));
        resume.send(()).unwrap();
        assert!(matches!(
            early,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(old.join().unwrap().err().unwrap(), CANCELLED);
        assert!(
            done_receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .connected
        );
        new.join().unwrap();
        assert_eq!(
            store.memory.load("client").unwrap().unwrap().access_token,
            "newer-account-access"
        );
    }

    #[test]
    fn failed_cancel_cleanup_quarantines_credentials_until_disconnect() {
        let (github, store, entered, resume) = blocked_fixture(false, true);
        store.fail_delete.store(true, Ordering::SeqCst);
        let signer = github.clone();
        let sign_in =
            thread::spawn(move || signer.commit_sign_in("client", saved(now().unwrap() + 3600), 0));
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        github.cancel().unwrap();
        resume.send(()).unwrap();
        assert!(sign_in
            .join()
            .unwrap()
            .err()
            .unwrap()
            .contains("cleanup failed"));
        assert!(github.status().err().unwrap().contains("cleaned up"));
        assert!(github.access_token().unwrap_err().contains("cleaned up"));
        store.fail_delete.store(false, Ordering::SeqCst);
        github.disconnect().unwrap();
        assert!(!github.status().unwrap().connected);
        assert!(github.access_token().unwrap().is_none());
    }

    fn fixture() -> (Github, Arc<MemoryStore>, Arc<MockTransport>) {
        let store = Arc::new(MemoryStore::default());
        let transport = Arc::new(MockTransport::default());
        // Tests choose an available port, then configure it explicitly just like a registered app.
        let (reservation, port) = listener();
        let port = port.to_string();
        let github = Github {
            config: Config::parse(
                Some("client"),
                Some("https://broker.example/base"),
                Some("test-app"),
                Some(&port),
            ),
            store: Box::new(store.clone()),
            transport: Box::new(transport.clone()),
            credentials: Mutex::new(CredentialState::default()),
            pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        };
        drop(reservation);
        (github, store, transport)
    }
    fn saved(expiry: u64) -> SavedToken {
        SavedToken {
            access_token: "old-access".into(),
            refresh_token: Some("old-refresh".into()),
            expires_at: Some(expiry),
            refresh_expires_at: Some(now().unwrap() + 7200),
            login: "octocat".into(),
            avatar_data_url: None,
        }
    }
    fn request(port: u16, query: &str) -> String {
        format!("GET /callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
    }
    fn listener() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }
    fn png() -> Vec<u8> {
        STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a5v8AAAAASUVORK5CYII=").unwrap()
    }

    #[test]
    fn avatar_requests_are_exact_host_https_and_unauthenticated() {
        let http = Http::new();
        let url = validated_avatar_url("https://avatars.githubusercontent.com/u/1?v=4").unwrap();
        let request = http.avatar_request(&url).unwrap().build().unwrap();
        assert_eq!(request.url(), &url);
        for header in ["authorization", "cookie", "proxy-authorization"] {
            assert!(!request.headers().contains_key(header));
        }
        for value in [
            "http://avatars.githubusercontent.com/u/1",
            "https://avatars.githubusercontent.com.evil.example/u/1",
            "https://evil.avatars.githubusercontent.com/u/1",
            "https://avatars.githubusercontent.com@evil.example/u/1",
            "https://user:secret@avatars.githubusercontent.com/u/1",
            "https://avatars.githubusercontent.com:444/u/1",
            "https://avatars.githubusercontent.com./u/1",
            "https://127.0.0.1/u/1",
            "https://avatars.githubusercontent.com/u/1#fragment",
        ] {
            assert!(validated_avatar_url(value).is_err());
            assert!(http.avatar_request(&Url::parse(value).unwrap()).is_err());
        }
    }

    #[test]
    fn optional_avatar_failures_and_disallowed_urls_do_not_block_sign_in() {
        let (github, _, transport) = fixture();
        transport.avatar_fail.store(true, Ordering::SeqCst);
        let status = github.start(callback_browser).unwrap();
        assert!(status.connected);
        assert!(status.avatar_url.is_none());
        assert_eq!(transport.avatars.load(Ordering::SeqCst), 1);
        let (github, _, transport) = fixture();
        *transport.avatar_url.lock().unwrap() = "https://evil.example/avatar.png".into();
        let status = github.start(callback_browser).unwrap();
        assert!(status.connected);
        assert!(status.avatar_url.is_none());
        assert_eq!(transport.avatars.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn avatar_survives_refresh_and_status_remains_offline() {
        let (github, store, transport) = fixture();
        let data = Avatar {
            content_type: "image/png".into(),
            bytes: png(),
        }
        .data_url()
        .unwrap();
        let mut token = saved(1);
        token.avatar_data_url = Some(data.clone());
        store.save("client", &token).unwrap();
        assert_eq!(
            github.access_token().unwrap().as_deref(),
            Some("new-access")
        );
        assert_eq!(
            store
                .load("client")
                .unwrap()
                .unwrap()
                .avatar_data_url
                .as_deref(),
            Some(data.as_str())
        );
        assert_eq!(
            github.status().unwrap().avatar_url.as_deref(),
            Some(data.as_str())
        );
        assert_eq!(transport.avatars.load(Ordering::SeqCst), 0);
        assert!(transport.paths.lock().unwrap().is_empty());
    }

    #[test]
    fn small_keyring_limits_drop_only_optional_avatar() {
        let (github, store, _) = fixture();
        store.fail_avatar.store(true, Ordering::SeqCst);
        let status = github.start(callback_browser).unwrap();
        assert!(status.connected);
        assert!(status.avatar_url.is_none());
        let saved = store.load("client").unwrap().unwrap();
        assert_eq!(saved.login, "octocat");
        assert_eq!(saved.access_token, "new-access");
        assert!(saved.avatar_data_url.is_none());
    }

    #[test]
    fn legacy_profiles_and_untrusted_cached_avatars_are_safe() {
        let json = serde_json::json!({"access_token":"old", "refresh_token":null,
            "expires_at":null, "refresh_expires_at":null, "login":"octocat"});
        let token: SavedToken = serde_json::from_value(json).unwrap();
        assert!(token.avatar_data_url.is_none());
        let (github, store, _) = fixture();
        for value in [
            "https://evil.example/avatar.png",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:image/png;base64,PHN2Zz4=",
            "data:image/png;base64,invalid!",
        ] {
            let mut token = saved(now().unwrap() + 3600);
            token.avatar_data_url = Some(value.into());
            store.save("client", &token).unwrap();
            assert!(github.status().unwrap().avatar_url.is_none());
        }
        assert!(Avatar {
            content_type: "image/png".into(),
            bytes: vec![0; MAX_AVATAR_BYTES as usize + 1]
        }
        .data_url()
        .is_err());
    }

    #[test]
    fn unconfigured_access_token_preserves_git_fallback() {
        let (mut github, store, transport) = fixture();
        store.save("client", &saved(1)).unwrap();
        github.config = Config::parse(None, None, None, None);
        assert!(github.access_token().unwrap().is_none());
        assert!(!github.status().unwrap().configured);
        assert!(github
            .start(callback_browser)
            .err()
            .unwrap()
            .contains("MDWELL_GITHUB_CLIENT_ID"));
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
        assert!(store.load("client").unwrap().is_some());
    }

    fn local_avatar_response(
        status: &str,
        content_type: &str,
        bytes: Vec<u8>,
        length: Option<u64>,
    ) -> Result<Avatar, String> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_owned();
        let content_type = content_type.to_owned();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let length = length
                .map(|n| format!("Content-Length: {n}\r\n"))
                .unwrap_or_default();
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{length}Location: http://127.0.0.1:1/leak\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(&bytes);
        });
        // Only the response decoder is tested with HTTP. Production requests enforce HTTPS and host.
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let result = response_avatar(client.get(format!("http://{addr}/avatar")).send().unwrap());
        server.join().unwrap();
        result
    }

    #[test]
    fn avatar_http_decoder_rejects_redirects_svg_mismatch_and_oversized_bodies() {
        assert!(local_avatar_response("302 Found", "image/png", png(), None).is_err());
        assert!(
            local_avatar_response("200 OK", "image/svg+xml", b"<svg/>".to_vec(), None).is_err()
        );
        assert!(local_avatar_response("200 OK", "image/png", b"<svg/>".to_vec(), None).is_err());
        assert!(
            local_avatar_response("200 OK", "image/png", vec![], Some(MAX_AVATAR_BYTES + 1))
                .is_err()
        );
        let mut oversized = png();
        oversized.resize(MAX_AVATAR_BYTES as usize + 1, 0);
        assert!(local_avatar_response("200 OK", "image/png", oversized, None).is_err());
        let avatar =
            local_avatar_response("200 OK", "image/png; charset=binary", png(), None).unwrap();
        assert_eq!(
            avatar.data_url().unwrap(),
            format!("data:image/png;base64,{}", STANDARD.encode(png()))
        );
    }

    #[test]
    fn pkce_rfc_vector_and_random_state() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let a = random_secret().unwrap();
        let b = random_secret().unwrap();
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
    }
    #[test]
    fn config_is_explicit_and_https_only() {
        assert!(Config::parse(None, None, None, None)
            .err()
            .unwrap()
            .contains("MDWELL_GITHUB_CLIENT_ID"));
        for broker in [
            "http://broker.example",
            "https://user:secret@broker.example",
            "https://broker.example/?secret=x",
            "https://broker.example/#x",
        ] {
            assert!(Config::parse(Some("id"), Some(broker), Some("app"), Some("1024")).is_err());
        }
        let (github, _, _) = fixture();
        assert_eq!(
            github.config().unwrap().token_url.as_str(),
            "https://broker.example/base/token"
        );
        assert_eq!(
            github.management_url().unwrap(),
            "https://github.com/apps/test-app/installations/new"
        );
    }
    #[test]
    fn callback_port_is_required_and_unprivileged() {
        let (mut github, _, _) = fixture();
        github.config = Config::parse(
            Some("client"),
            Some("https://broker.example"),
            Some("app"),
            None,
        );
        assert!(!github.status().unwrap().configured);
        assert!(github.access_token().unwrap().is_none());
        assert!(github
            .start(|_| panic!("missing port must not open browser"))
            .err()
            .unwrap()
            .contains("MDWELL_GITHUB_CALLBACK_PORT"));
        for port in [
            "", " ", "0", "1", "80", "443", "1023", "65536", "-1024", "+1024", "1.5", "invalid",
            " 1024", "1024 ",
        ] {
            assert!(Config::parse(
                Some("client"),
                Some("https://broker.example"),
                Some("app"),
                Some(port)
            )
            .is_err());
        }
        for port in ["1024", "65535"] {
            let config = Config::parse(
                Some("client"),
                Some("https://broker.example"),
                Some("app"),
                Some(port),
            )
            .unwrap();
            assert_eq!(config.callback_port.to_string(), port);
        }
    }

    #[test]
    fn compiled_config_without_callback_port_is_unconfigured() {
        if option_env!("MDWELL_GITHUB_CALLBACK_PORT").is_none() {
            assert!(Config::compiled().is_err());
            assert!(!Github::new().status().unwrap().configured);
        }
    }

    #[test]
    fn busy_callback_port_fails_before_browser_without_fallback() {
        let (mut github, store, transport) = fixture();
        let (occupied, port) = listener();
        github.config.as_mut().unwrap().callback_port = port;
        let error = github
            .start(|_| panic!("busy port must not open browser"))
            .err()
            .unwrap();
        assert!(error.contains(&format!("port {port} is busy")));
        assert!(error.contains("no fallback"));
        assert!(!github.pending.load(Ordering::SeqCst));
        assert!(store.load("client").unwrap().is_none());
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
        drop(occupied);
        let error = github
            .start(|url| {
                let url = Url::parse(&url).unwrap();
                let params: BTreeMap<_, _> = url.query_pairs().collect();
                assert_eq!(
                    params.get("redirect_uri").unwrap(),
                    &format!("http://127.0.0.1:{port}/callback")
                );
                // The callback listener is already live on the configured port before open is called.
                assert!(TcpListener::bind(("127.0.0.1", port)).is_err());
                github.cancel()?;
                Ok(())
            })
            .err()
            .unwrap();
        assert_eq!(error, CANCELLED);
    }

    #[test]
    fn callback_rejects_forgery_duplicates_and_wrong_host() {
        assert!(
            matches!(parse_callback(&request(1234,"code=abc&state=expected"),1234,"expected"),Some(Callback::Code(code)) if code == "abc")
        );
        for query in [
            "code=abc&state=wrong",
            "code=abc",
            "code=a&code=b&state=expected",
            "code=a&state=expected&state=expected",
            "code=a&error=x&state=expected",
            "code=&state=expected",
        ] {
            assert!(parse_callback(&request(1234, query), 1234, "expected").is_none());
        }
        assert!(
            parse_callback(&request(1234, "code=a&state=expected"), 4321, "expected").is_none()
        );
        assert!(matches!(
            parse_callback(
                &request(1234, "error=access_denied&state=expected"),
                1234,
                "expected"
            ),
            Some(Callback::Denied)
        ));
    }
    #[test]
    fn callback_timeout_and_cancellation_are_bounded() {
        let (listener, port) = listener();
        let begin = Instant::now();
        assert!(wait_callback(
            &listener,
            port,
            "state",
            begin + Duration::from_millis(50),
            || Ok(())
        )
        .unwrap_err()
        .contains("timed out"));
        assert!(begin.elapsed() < Duration::from_secs(1));
        assert_eq!(
            wait_callback(
                &listener,
                port,
                "state",
                Instant::now() + Duration::from_secs(10),
                || Err(CANCELLED.into())
            )
            .unwrap_err(),
            CANCELLED
        );
    }
    #[test]
    fn loopback_ignores_unrelated_callback_then_accepts_valid_one() {
        let (listener, port) = listener();
        let sender = thread::spawn(move || {
            for query in ["code=wrong&state=forged", "code=right&state=expected"] {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream.write_all(request(port, query).as_bytes()).unwrap();
                let mut reply = String::new();
                stream.read_to_string(&mut reply).unwrap();
                assert!(!reply.contains("expected"));
            }
        });
        assert_eq!(
            wait_callback(
                &listener,
                port,
                "expected",
                Instant::now() + Duration::from_secs(3),
                || Ok(())
            )
            .unwrap(),
            "right"
        );
        sender.join().unwrap();
    }
    #[test]
    fn offline_status_refresh_rotation_and_disconnect() {
        let (github, store, transport) = fixture();
        assert!(github.access_token().unwrap().is_none());
        store.save("client", &saved(1)).unwrap();
        assert_eq!(github.status().unwrap().login.as_deref(), Some("octocat"));
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
        assert_eq!(
            github.access_token().unwrap().as_deref(),
            Some("new-access")
        );
        assert_eq!(
            store
                .load("client")
                .unwrap()
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("new-refresh")
        );
        assert_eq!(
            github.access_token().unwrap().as_deref(),
            Some("new-access")
        );
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 1);
        github.disconnect().unwrap();
        assert!(!github.status().unwrap().connected);
    }
    #[test]
    fn concurrent_refresh_is_serialized() {
        let (github, store, transport) = fixture();
        store.save("client", &saved(1)).unwrap();
        let github = Arc::new(github);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let github = github.clone();
                thread::spawn(move || github.access_token().unwrap())
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap().as_deref(), Some("new-access"));
        }
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn broker_or_storage_failure_never_returns_unpersisted_token() {
        let (github, store, transport) = fixture();
        store.save("client", &saved(1)).unwrap();
        transport.fail.store(true, Ordering::SeqCst);
        assert!(github.access_token().is_err());
        transport.fail.store(false, Ordering::SeqCst);
        store.fail_save.store(true, Ordering::SeqCst);
        assert!(github.access_token().is_err());
        assert_eq!(
            store.load("client").unwrap().unwrap().access_token,
            "old-access"
        );
    }
    #[test]
    fn refresh_expiry_requires_sign_in_and_token_errors_are_sanitized() {
        let (github, store, transport) = fixture();
        let mut token = saved(1);
        token.refresh_expires_at = Some(1);
        store.save("client", &token).unwrap();
        assert!(github.access_token().unwrap_err().contains("expired"));
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
        let response: TokenResponse =
            serde_json::from_value(serde_json::json!({"error":"secret-code-secret-token"}))
                .unwrap();
        let error = response.into_saved("user".into(), 1).err().unwrap();
        assert!(!error.contains("secret"));
    }
    #[test]
    fn repositories_page_and_deduplicate() {
        let (github, store, transport) = fixture();
        store.save("client", &saved(now().unwrap() + 3600)).unwrap();
        let repositories = github.repositories().unwrap();
        assert_eq!(repositories.len(), 1);
        assert_eq!(repositories[0].id, 7);
        assert!(transport
            .paths
            .lock()
            .unwrap()
            .iter()
            .any(|p| p == "/user/installations/1/repositories?per_page=100&page=2"));
    }
    #[test]
    fn start_contract_cancellation_and_single_pending() {
        let (github, _, _) = fixture();
        let error = github
            .start(|url| {
                let url = Url::parse(&url).unwrap();
                let params: BTreeMap<_, _> = url.query_pairs().collect();
                assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
                assert_eq!(params.get("state").unwrap().len(), 43);
                assert!(github.start(|_| Ok(())).err().unwrap().contains("already"));
                github.cancel()?;
                Ok(())
            })
            .err()
            .unwrap();
        assert_eq!(error, CANCELLED);
        assert!(!github.pending.load(Ordering::SeqCst));
        assert!(github
            .start(|_| Err("secret browser error".into()))
            .err()
            .unwrap()
            .contains("system browser"));
    }
    #[test]
    fn sign_in_roundtrip_persists_profile() {
        let (github, store, _) = fixture();
        let status = github
            .start(|url| {
                let url = Url::parse(&url).unwrap();
                let params: BTreeMap<_, _> = url.query_pairs().collect();
                let redirect = Url::parse(params.get("redirect_uri").unwrap()).unwrap();
                let port = redirect.port().unwrap();
                let state = params.get("state").unwrap().to_string();
                thread::spawn(move || {
                    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                    stream
                        .write_all(
                            request(port, &format!("code=test-code&state={state}")).as_bytes(),
                        )
                        .unwrap();
                    let mut reply = String::new();
                    let _ = stream.read_to_string(&mut reply);
                });
                Ok(())
            })
            .unwrap();
        assert!(status.connected);
        let expected = Avatar {
            content_type: "image/png".into(),
            bytes: png(),
        }
        .data_url()
        .unwrap();
        assert_eq!(status.avatar_url.as_deref(), Some(expected.as_str()));
        assert_eq!(store.load("client").unwrap().unwrap().login, "octocat");
        assert_eq!(github.status().unwrap().avatar_url, status.avatar_url);
    }
    fn callback_browser(url: String) -> Result<(), String> {
        let url = Url::parse(&url).unwrap();
        let params: BTreeMap<_, _> = url.query_pairs().collect();
        let redirect = Url::parse(params.get("redirect_uri").unwrap()).unwrap();
        let port = redirect.port().unwrap();
        let state = params.get("state").unwrap().to_string();
        thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .write_all(request(port, &format!("code=test-code&state={state}")).as_bytes())
                .unwrap();
            let mut reply = String::new();
            let _ = stream.read_to_string(&mut reply);
        });
        Ok(())
    }

    #[test]
    fn disconnect_during_exchange_cannot_resurrect_credentials() {
        struct PausedBroker {
            inner: Arc<MockTransport>,
            entered: Arc<std::sync::Barrier>,
            resume: Arc<std::sync::Barrier>,
        }
        impl Transport for PausedBroker {
            fn avatar(&self, url: &Url) -> Result<Avatar, String> {
                self.inner.avatar(url)
            }
            fn token(&self, url: &Url, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
                assert!(fields.contains(&("grant_type", "authorization_code")));
                assert!(fields
                    .iter()
                    .any(|(key, value)| *key == "code_verifier" && value.len() == 43));
                assert!(fields
                    .iter()
                    .any(|(key, value)| *key == "redirect_uri"
                        && value.starts_with("http://127.0.0.1:")));
                self.entered.wait();
                self.resume.wait();
                self.inner.token(url, fields)
            }
            fn get(&self, path: &str, token: &str) -> Result<serde_json::Value, String> {
                self.inner.get(path, token)
            }
        }
        let (mut github, store, transport) = fixture();
        store.save("client", &saved(now().unwrap() + 3600)).unwrap();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        github.transport = Box::new(PausedBroker {
            inner: transport,
            entered: entered.clone(),
            resume: resume.clone(),
        });
        let github = Arc::new(github);
        let worker = github.clone();
        let sign_in = thread::spawn(move || worker.start(callback_browser));
        entered.wait();
        github.disconnect().unwrap();
        resume.wait();
        assert_eq!(sign_in.join().unwrap().err().unwrap(), CANCELLED);
        assert!(store.load("client").unwrap().is_none());
        assert!(!github.pending.load(Ordering::SeqCst));
    }

    #[test]
    fn incomplete_callback_does_not_block_cancellation() {
        let (listener, port) = listener();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(b"GET /callback?").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            signal.store(true, Ordering::SeqCst);
        });
        let begin = Instant::now();
        let result = wait_callback(
            &listener,
            port,
            "state",
            begin + Duration::from_secs(10),
            || {
                if cancelled.load(Ordering::SeqCst) {
                    Err(CANCELLED.into())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(begin.elapsed() < Duration::from_secs(1));
        worker.join().unwrap();
    }

    #[test]
    fn installation_list_is_paginated_too() {
        struct InstallationPages(Mutex<Vec<String>>);
        impl Transport for Arc<InstallationPages> {
            fn avatar(&self, _: &Url) -> Result<Avatar, String> {
                panic!("repository listing must not fetch avatars")
            }
            fn token(&self, _: &Url, _: &[(&str, &str)]) -> Result<TokenResponse, String> {
                panic!("unexpired access token must not be refreshed")
            }
            fn get(&self, path: &str, _: &str) -> Result<serde_json::Value, String> {
                self.0.lock().unwrap().push(path.to_owned());
                if path == "/user/installations?per_page=100&page=1" {
                    let installations: Vec<_> =
                        (1..=100).map(|id| serde_json::json!({"id":id})).collect();
                    Ok(serde_json::json!({"installations":installations}))
                } else if path == "/user/installations?per_page=100&page=2" {
                    Ok(serde_json::json!({"installations":[{"id":101}]}))
                } else {
                    Ok(serde_json::json!({"repositories":[]}))
                }
            }
        }
        let (mut github, store, _) = fixture();
        let transport = Arc::new(InstallationPages(Mutex::new(Vec::new())));
        github.transport = Box::new(transport.clone());
        store.save("client", &saved(now().unwrap() + 3600)).unwrap();
        assert!(github.repositories().unwrap().is_empty());
        let paths = transport.0.lock().unwrap();
        assert!(paths
            .iter()
            .any(|p| p == "/user/installations?per_page=100&page=2"));
        assert!(paths
            .iter()
            .any(|p| p == "/user/installations/101/repositories?per_page=100&page=1"));
        assert_eq!(paths.len(), 103);
    }

    #[test]
    fn invalid_token_responses_fail_closed() {
        for value in [
            serde_json::json!({"access_token":"secret", "token_type":"basic"}),
            serde_json::json!({"access_token":"secret", "token_type":"bearer", "expires_in":10}),
            serde_json::json!({"access_token":"", "token_type":"bearer"}),
            serde_json::json!({"access_token":"secret\r\ninjection", "token_type":"bearer"}),
            serde_json::json!({"access_token":"secret", "token_type":"bearer", "error":"secret-error"}),
        ] {
            let response: TokenResponse = serde_json::from_value(value).unwrap();
            let error = response.into_saved("user".into(), 1).err().unwrap();
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn oversized_broker_response_is_rejected_before_reading_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_BODY + 1
            )
            .unwrap();
        });
        let error = Http::new()
            .token(
                &Url::parse(&format!("http://{addr}/token")).unwrap(),
                &[("code", "test")],
            )
            .err()
            .unwrap();
        assert!(error.contains("size limit"));
        server.join().unwrap();
    }

    #[test]
    fn real_http_broker_errors_and_redirects_do_not_leak_secrets() {
        for (status, body) in [
            ("500 Internal Server Error", "secret-token"),
            ("200 OK", "not-json-secret"),
            ("302 Found", "secret-code"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                write!(stream,"HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/leak\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            });
            // HTTP is allowed only for this directly injected test endpoint; Config rejects it in production.
            let error = Http::new()
                .token(
                    &Url::parse(&format!("http://{addr}/token")).unwrap(),
                    &[("code", "secret-code")],
                )
                .err()
                .unwrap();
            assert!(!error.contains("secret"));
            assert!(!error.contains("127.0.0.1"));
            server.join().unwrap();
        }
    }
}
