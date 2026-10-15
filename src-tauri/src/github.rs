//! Blocking GitHub App device authentication. Call network methods on spawn_blocking.
//! Notify the UI with the public user code, then poll here; device codes and tokens never
//! leave this module except for the access token used by native git operations.

use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, Zeroizing};

const CLIENT_ID: &str = "Iv23liS0HYbIuk9WY8B0";
const APP_SLUG: &str = "vellistry";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const EXPIRED: &str = "GitHub device code expired. Start Vellistry sign-in again.";
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

struct RepositoryGrant {
    repository: Repository,
    contents_readable: bool,
}

/// Safe to send over a Tauri Channel. The device code is deliberately private.
#[derive(Serialize, Clone)]
pub struct DeviceAuthorization {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}
fn default_interval() -> u64 {
    5
}
impl Drop for DeviceResponse {
    fn drop(&mut self) {
        self.device_code.zeroize();
    }
}
impl DeviceResponse {
    fn authorization(&self) -> Result<DeviceAuthorization, String> {
        if self.device_code.is_empty()
            || self.device_code.len() > 4096
            || !self.device_code.bytes().all(|b| b.is_ascii_graphic())
            || self.user_code.is_empty()
            || self.user_code.len() > 128
            || !self
                .user_code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || self.verification_uri != "https://github.com/login/device"
            || self.expires_in == 0
            || self.expires_in > 86400
            || self.interval == 0
            || self.interval > 86400
        {
            return Err("GitHub returned an invalid device authorization.".into());
        }
        Ok(DeviceAuthorization {
            user_code: self.user_code.clone(),
            verification_uri: self.verification_uri.clone(),
            expires_in: self.expires_in,
            interval: self.interval,
        })
    }
}

struct Config {
    client_id: String,
    device_url: Url,
    token_url: Url,
    app_slug: String,
}
impl Config {
    fn compiled() -> Result<Self, String> {
        Ok(Self {
            client_id: CLIENT_ID.into(),
            device_url: Url::parse("https://github.com/login/device/code")
                .map_err(|_| "Invalid GitHub device URL.")?,
            token_url: Url::parse("https://github.com/login/oauth/access_token")
                .map_err(|_| "Invalid GitHub token URL.")?,
            app_slug: APP_SLUG.into(),
        })
    }
}

// Transport errors are classified without retaining URLs, bodies, or library errors.
enum PollError {
    Transient,
    Fatal(String),
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
    interval: Option<u64>,
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
            return Err("GitHub rejected the token grant. Sign in to Vellistry again.".into());
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
            return Err("GitHub returned an invalid token response.".into());
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

// Errors crossing these native boundaries must be sanitized, user-facing guidance.
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

    const LOCKED: &str = "GitHub credential storage is locked or requires a prompt. Unlock your desktop keyring outside Vellistry and retry.";
    const UNAVAILABLE: &str = "GitHub Secret Service is unavailable or timed out. Check your desktop keyring outside Vellistry and retry.";
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
            return Err("Multiple GitHub keyring entries match. Resolve them in your desktop keyring outside Vellistry.".into());
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
            .contains("outside Vellistry"));
        assert!(require_unlocked(false).is_ok());
        assert_eq!(error(dbus_secret_service::Error::Prompt), LOCKED);
        assert_eq!(error(dbus_secret_service::Error::Locked), LOCKED);
        assert_eq!(error(dbus_secret_service::Error::Unavailable), UNAVAILABLE);
    }
}

// Never return raw response bodies, request URLs, credentials, or library errors.
trait Transport: Send + Sync {
    fn device(&self, _url: &Url, _client_id: &str) -> Result<DeviceResponse, String> {
        Err("GitHub device authorization is unavailable.".into())
    }
    fn poll_token(
        &self,
        url: &Url,
        fields: &[(&str, &str)],
        _remaining: Duration,
    ) -> Result<TokenResponse, PollError> {
        self.token(url, fields).map_err(PollError::Fatal)
    }
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
                .user_agent("vellistry-desktop/0.1")
                .build()
                .map_err(|_| "Cannot initialize GitHub HTTPS client.".into()),
            avatar_client: Client::builder()
                .https_only(true)
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(5))
                .user_agent("vellistry-desktop/0.1")
                .build()
                .map_err(|_| "Cannot initialize GitHub avatar client.".into()),
        }
    }
    fn auth_post<T: DeserializeOwned>(
        &self,
        url: &Url,
        fields: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<T, PollError> {
        let response = self
            .client()
            .map_err(PollError::Fatal)?
            .post(url.clone())
            .header("Accept", "application/json")
            .form(fields)
            .timeout(timeout.min(Duration::from_secs(15)))
            .send()
            .map_err(|_| PollError::Transient)?;
        if response.status().is_server_error() || response.status().as_u16() == 429 {
            return Err(PollError::Transient);
        }
        auth_response_json(response)
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
    auth_response_json(response).map_err(|error| match error {
        PollError::Transient => "Cannot read GitHub service response.".into(),
        PollError::Fatal(error) => error,
    })
}
fn auth_response_json<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
) -> Result<T, PollError> {
    let status = response.status();
    if !status.is_success() {
        // Never include response bodies, URLs, or underlying library errors: they may contain secrets.
        return Err(PollError::Fatal(match status.as_u16() {
            401 => "GitHub authentication was rejected. Sign in again.".into(),
            403 | 429 => {
                "GitHub request denied or rate limited. Check app permissions or retry later."
                    .into()
            }
            _ => format!("GitHub service request failed (HTTP {}).", status.as_u16()),
        }));
    }
    if response.content_length().is_some_and(|n| n > MAX_BODY) {
        return Err(PollError::Fatal(
            "GitHub response exceeded the size limit.".into(),
        ));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PollError::Transient)?;
    if bytes.len() as u64 > MAX_BODY {
        return Err(PollError::Fatal(
            "GitHub response exceeded the size limit.".into(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| PollError::Fatal("GitHub service returned an invalid response.".into()))
}
impl Transport for Http {
    fn device(&self, url: &Url, client_id: &str) -> Result<DeviceResponse, String> {
        self.auth_post(url, &[("client_id", client_id)], Duration::from_secs(15))
            .map_err(|error| match error {
                PollError::Transient => {
                    "GitHub device authorization is unreachable or rate limited. Retry later."
                        .into()
                }
                PollError::Fatal(error) => error,
            })
    }
    fn poll_token(
        &self,
        url: &Url,
        fields: &[(&str, &str)],
        remaining: Duration,
    ) -> Result<TokenResponse, PollError> {
        self.auth_post(url, fields, remaining)
    }
    fn avatar(&self, url: &Url) -> Result<Avatar, String> {
        response_avatar(
            self.avatar_request(url)?
                .send()
                .map_err(|_| "GitHub avatar is unreachable or timed out.".to_string())?,
        )
    }
    fn token(&self, url: &Url, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
        self.auth_post(url, fields, Duration::from_secs(15)).map_err(|error| match error {
            PollError::Transient => "GitHub token service is unreachable or rate limited. Sign in to Vellistry again.".into(),
            PollError::Fatal(error) => error,
        })
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
            Err("Unsafe GitHub credentials could not be cleaned up. Unlock your keyring outside Vellistry, then disconnect or sign in again.".into())
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
    /// Request a device code, notify once, then poll on the calling blocking worker.
    /// Cancellation interrupts waits within POLL; an already in-flight HTTP request is
    /// bounded by its 15-second timeout (or remaining device lifetime, if shorter).
    pub fn start(
        &self,
        notify: impl FnOnce(DeviceAuthorization) -> Result<(), String>,
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
        self.check_generation(generation)?;
        let device = self
            .transport
            .device(&config.device_url, &config.client_id)?;
        self.check_generation(generation)?;
        let authorization = device.authorization()?;
        let deadline = Instant::now() + Duration::from_secs(authorization.expires_in);
        notify(authorization)
            .map_err(|_| "Cannot display the GitHub device code in Vellistry.".to_string())?;
        let response = self.poll_device(config, &device, deadline, generation)?;
        self.check_generation(generation)?;
        let mut token = response.into_saved(String::new(), now()?)?;
        self.check_generation(generation)?;
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
            .and_then(|url| {
                self.check_generation(generation).ok()?;
                self.transport.avatar(&url).ok()
            })
            .and_then(|avatar| avatar.data_url().ok());
        self.commit_sign_in(&config.client_id, token, generation)
    }
    fn wait_poll(&self, delay: Duration, deadline: Instant, generation: u64) -> Result<(), String> {
        let next = Instant::now()
            .checked_add(delay)
            .unwrap_or(deadline)
            .min(deadline);
        loop {
            self.check_generation(generation)?;
            let now = Instant::now();
            if now >= deadline {
                return Err(EXPIRED.into());
            }
            if now >= next {
                return Ok(());
            }
            thread::sleep(POLL.min(next - now));
        }
    }
    fn poll_device(
        &self,
        config: &Config,
        device: &DeviceResponse,
        deadline: Instant,
        generation: u64,
    ) -> Result<TokenResponse, String> {
        let mut interval = device.interval;
        let mut delay = interval;
        loop {
            // Wait before the first poll as well as every subsequent attempt. Backoff is
            // measured from response completion, never from request start.
            self.wait_poll(Duration::from_secs(delay), deadline, generation)?;
            self.check_generation(generation)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EXPIRED.into());
            }
            let result = self.transport.poll_token(
                &config.token_url,
                &[
                    ("client_id", &config.client_id),
                    ("device_code", &device.device_code),
                    ("grant_type", DEVICE_GRANT),
                ],
                remaining,
            );
            self.check_generation(generation)?;
            if Instant::now() >= deadline {
                return Err(EXPIRED.into());
            }
            match result {
                Err(PollError::Transient) => {
                    delay = delay.saturating_mul(2).max(interval).min(86400);
                }
                Err(PollError::Fatal(error)) => return Err(error),
                Ok(response) => {
                    if response.error.is_some() && response.access_token.is_some() {
                        return Err("GitHub returned an invalid token response.".into());
                    }
                    match response.error.as_deref() {
                        None => return Ok(response),
                        Some("authorization_pending") => {
                            delay = interval;
                        }
                        Some("slow_down") => {
                            interval = slowed_interval(interval, response.interval)?;
                            delay = interval;
                        }
                        Some("expired_token") => return Err(EXPIRED.into()),
                        Some("access_denied") => {
                            return Err(
                                "GitHub authorization was denied. Start Vellistry sign-in again."
                                    .into(),
                            )
                        }
                        Some("incorrect_device_code" | "invalid_grant" | "invalid_device_code") => {
                            return Err(
                                "GitHub device code is invalid. Start Vellistry sign-in again."
                                    .into(),
                            )
                        }
                        Some(_) => return Err(
                            "GitHub rejected device authorization. Start Vellistry sign-in again."
                                .into(),
                        ),
                    }
                }
            }
        }
    }
    fn commit_sign_in(
        &self,
        client_id: &str,
        mut token: SavedToken,
        generation: u64,
    ) -> Result<AccountStatus, String> {
        let mut guard = self.sign_in_lock(generation)?;
        if guard.cleanup_required {
            self.store.delete(client_id)?;
            guard.cleanup_required = false;
        }
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
                return Err("GitHub sign-in cancelled, but secure credential cleanup failed. Unlock your keyring outside Vellistry and disconnect before restarting.".into());
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
        let mut guard = self.lock()?;
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
            // A lost network/storage reply may still have consumed the refresh token.
            // Never retry it after an ambiguous failure, even if deleting the keyring fails.
            guard.cleanup_required = true;
            let rotated = (|| {
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
                self.save_profile(&config.client_id, &mut refreshed)?;
                Ok::<_, String>(refreshed)
            })();
            match rotated {
                Ok(refreshed) => {
                    token = refreshed;
                    guard.cleanup_required = false;
                }
                Err(error) => {
                    if self.store.delete(&config.client_id).is_ok() {
                        guard.cleanup_required = false;
                    }
                    return Err(error);
                }
            }
        }
        Ok(Some(token.access_token.clone()))
    }
    /// Native add/sync preflight. None leaves authentication to external Git helpers.
    pub fn token_for_repository(&self, url: &str) -> Result<Option<String>, String> {
        let target = repository_name(url)?;
        let Some(token) = self
            .access_token()
            .map_err(|error| format!("GitHub authentication or token refresh failed: {error}"))?
        else {
            return Ok(None);
        };
        let token = Zeroizing::new(token);
        // Discovery must use this exact token, not reload or refresh credentials again.
        let grants = self.repository_grants_with_token(&token).map_err(|error| {
            format!("Could not verify GitHub installation repository access: {error}")
        })?;
        let mut matching = grants
            .iter()
            .filter(|grant| grant.repository.full_name.eq_ignore_ascii_case(&target))
            .peekable();
        if matching.peek().is_none() {
            return Err("GitHub repository permission is missing. Install or configure the Vellistry GitHub App to include this repository, and ensure your signed-in account has access.".into());
        }
        if !matching.any(|grant| grant.contents_readable) {
            return Err("GitHub App Contents read permission is missing for this repository. Ask the installation owner to approve the Vellistry GitHub App Contents Read-only permission update, then reauthorize Vellistry by disconnecting and signing in again.".into());
        }
        Ok(Some(token.to_string()))
    }
    pub fn repositories(&self) -> Result<Vec<Repository>, String> {
        let access_token = Zeroizing::new(
            self.access_token()?
                .ok_or_else(|| "Connect a GitHub account first.".to_string())?,
        );
        Ok(self
            .repository_grants_with_token(&access_token)?
            .into_iter()
            .map(|grant| grant.repository)
            .collect())
    }
    fn repository_grants_with_token(
        &self,
        access_token: &str,
    ) -> Result<Vec<RepositoryGrant>, String> {
        let mut repositories = BTreeMap::new();
        for installation in self.pages("/user/installations", "installations", access_token)? {
            let id = installation
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "GitHub returned an invalid installation.".to_string())?;
            // Older responses/fixtures may omit permissions entirely. Once present,
            // metadata visibility alone is not evidence that Git can read Contents.
            let contents_readable = installation.get("permissions").is_none_or(|permissions| {
                matches!(
                    permissions.get("contents").and_then(|v| v.as_str()),
                    Some("read" | "write")
                )
            });
            for value in self.pages(
                &format!("/user/installations/{id}/repositories"),
                "repositories",
                access_token,
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
                let grant = repositories
                    .entry(repository.id)
                    .or_insert(RepositoryGrant {
                        repository,
                        contents_readable: false,
                    });
                grant.contents_readable |= contents_readable;
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

fn repository_name(input: &str) -> Result<String, String> {
    let invalid = || {
        "Invalid GitHub repository URL. Use https://github.com/owner/repo without credentials, query, or fragment.".to_string()
    };
    let url = Url::parse(input).map_err(|_| invalid())?;
    let (authority, raw_path) = input
        .strip_prefix("https://")
        .and_then(|value| value.split_once('/'))
        .ok_or_else(invalid)?;
    if !authority.eq_ignore_ascii_case("github.com")
        || url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !input.bytes().all(|b| b.is_ascii_graphic() && b != b'\\')
        // Reject URL-parser normalization (notably dot segments) rather than authorize
        // a repository different from the one written in the original URL.
        || url.path().strip_prefix('/') != Some(raw_path)
    {
        return Err(invalid());
    }
    let path = raw_path.strip_suffix('/').unwrap_or(raw_path);
    let (owner, repo) = path.split_once('/').ok_or_else(invalid)?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if owner.is_empty()
        || owner.len() > 39
        || owner.starts_with('-')
        || owner.ends_with('-')
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || repo.is_empty()
        || repo.len() > 100
        || matches!(repo, "." | "..")
        || !repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(invalid());
    }
    Ok(format!("{owner}/{repo}"))
}

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| "System clock is invalid for GitHub authentication.".into())
}
fn slowed_interval(interval: u64, server: Option<u64>) -> Result<u64, String> {
    let interval = interval.saturating_add(5).max(server.unwrap_or(0));
    if interval > 86400 {
        return Err("GitHub returned an invalid polling interval.".into());
    }
    Ok(interval)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        net::TcpListener,
        sync::{atomic::AtomicUsize, Arc},
    };

    #[derive(Default)]
    struct MemoryStore {
        json: Mutex<Option<String>>,
        fail_load: AtomicBool,
        fail_save: AtomicBool,
        fail_avatar: AtomicBool,
    }
    impl Store for Arc<MemoryStore> {
        fn load(&self, _: &str) -> Result<Option<SavedToken>, String> {
            if self.fail_load.load(Ordering::SeqCst) {
                return Err(
                    "Cannot read native GitHub credential storage. Unlock your keychain and retry."
                        .into(),
                );
            }
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
        get_tokens: Mutex<Vec<String>>,
        no_installations: AtomicBool,
        installations: Mutex<Option<serde_json::Value>>,
        api_fail: AtomicBool,
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
                get_tokens: Mutex::new(Vec::new()),
                no_installations: AtomicBool::new(false),
                installations: Mutex::new(None),
                api_fail: AtomicBool::new(false),
                avatar_fail: AtomicBool::new(false),
                avatar_url: Mutex::new("https://avatars.githubusercontent.com/u/1?v=4".into()),
                avatars: AtomicUsize::new(0),
            }
        }
    }
    fn device_fixture() -> DeviceResponse {
        serde_json::from_value(
            serde_json::json!({"device_code":"private-device-code", "user_code":"ABCD-EFGH",
            "verification_uri":"https://github.com/login/device", "expires_in":60, "interval":1}),
        )
        .unwrap()
    }
    fn notify_device(value: DeviceAuthorization) -> Result<(), String> {
        assert_eq!(value.user_code, "ABCD-EFGH");
        assert_eq!(value.verification_uri, "https://github.com/login/device");
        Ok(())
    }
    impl Transport for Arc<MockTransport> {
        fn device(&self, url: &Url, client_id: &str) -> Result<DeviceResponse, String> {
            assert_eq!(url.as_str(), "https://github.com/login/device/code");
            assert_eq!(client_id, "client");
            Ok(device_fixture())
        }
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
                return Err("token service unavailable".into());
            }
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            assert!(fields.iter().any(|(k, _)| *k == "client_id"));
            Ok(serde_json::from_value(
                serde_json::json!({"access_token":"new-access", "refresh_token":"new-refresh",
                "token_type":"bearer", "expires_in":3600, "refresh_token_expires_in":7200}),
            )
            .unwrap())
        }
        fn get(&self, path: &str, token: &str) -> Result<serde_json::Value, String> {
            self.paths.lock().unwrap().push(path.into());
            self.get_tokens.lock().unwrap().push(token.into());
            if self.api_fail.load(Ordering::SeqCst) {
                return Err("GitHub API is unreachable or timed out.".into());
            }
            if path == "/user" {
                return Ok(
                    serde_json::json!({"login":"octocat", "avatar_url": self.avatar_url.lock().unwrap().clone()}),
                );
            }
            if path.starts_with("/user/installations?") {
                if let Some(installations) = self.installations.lock().unwrap().as_ref() {
                    return Ok(serde_json::json!({"installations": installations}));
                }
                if self.no_installations.load(Ordering::SeqCst) {
                    return Ok(serde_json::json!({"installations": []}));
                }
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
                notify_device(url)
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
        let github = Github {
            config: Ok(Config {
                client_id: "client".into(),
                ..Config::compiled().unwrap()
            }),
            store: Box::new(store.clone()),
            transport: Box::new(transport.clone()),
            credentials: Mutex::new(CredentialState::default()),
            pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        };
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
        let status = github.start(notify_device).unwrap();
        assert!(status.connected);
        assert!(status.avatar_url.is_none());
        assert_eq!(transport.avatars.load(Ordering::SeqCst), 1);
        let (github, _, transport) = fixture();
        *transport.avatar_url.lock().unwrap() = "https://evil.example/avatar.png".into();
        let status = github.start(notify_device).unwrap();
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
        let status = github.start(notify_device).unwrap();
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
        github.config = Err("GitHub configuration unavailable in Vellistry.".into());
        assert!(github.access_token().unwrap().is_none());
        assert!(!github.status().unwrap().configured);
        assert!(github
            .start(notify_device)
            .err()
            .unwrap()
            .contains("configuration unavailable"));
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
    fn refresh_or_storage_failure_never_reuses_consumed_token() {
        for network in [true, false] {
            let (github, store, transport) = fixture();
            store.save("client", &saved(1)).unwrap();
            transport.fail.store(network, Ordering::SeqCst);
            store.fail_save.store(!network, Ordering::SeqCst);
            assert!(github.access_token().is_err());
            assert!(store.load("client").unwrap().is_none());
            assert!(github.access_token().unwrap().is_none());
        }
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
    fn preflight_without_account_preserves_external_git_helpers() {
        let (mut github, store, transport) = fixture();
        assert_eq!(
            github
                .token_for_repository("https://github.com/owner/repo.git")
                .unwrap(),
            None
        );
        store.save("client", &saved(1)).unwrap();
        github.config = Err("unconfigured".into());
        assert_eq!(
            github
                .token_for_repository("https://github.com/owner/repo")
                .unwrap(),
            None
        );
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
        assert!(transport.paths.lock().unwrap().is_empty());
    }

    #[test]
    fn preflight_allows_private_repository_with_same_refreshed_token_before_git() {
        let (github, store, transport) = fixture();
        store.save("client", &saved(1)).unwrap();
        // The caller only receives a Git credential after refresh and grant discovery.
        let token = github
            .token_for_repository("https://github.com/OWNER/REPO.git/")
            .unwrap()
            .unwrap();
        assert_eq!(token, "new-access");
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 1);
        let tokens = transport.get_tokens.lock().unwrap();
        assert!(!tokens.is_empty());
        assert!(tokens.iter().all(|value| value == &token));
        let paths = transport.paths.lock().unwrap();
        assert!(paths
            .iter()
            .any(|path| path == "/user/installations/1/repositories?per_page=100&page=2"));
        assert!(paths
            .iter()
            .all(|path| path.starts_with("/user/installations")));
    }

    #[test]
    fn preflight_allows_unexpired_token_without_refresh() {
        let (github, store, transport) = fixture();
        store.save("client", &saved(now().unwrap() + 3600)).unwrap();
        assert_eq!(
            github
                .token_for_repository("https://github.com/owner/repo")
                .unwrap()
                .as_deref(),
            Some("old-access")
        );
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
        assert!(transport
            .get_tokens
            .lock()
            .unwrap()
            .iter()
            .all(|token| token == "old-access"));
    }

    #[test]
    fn preflight_missing_repository_or_installation_requires_grant_not_public_access() {
        for no_installations in [false, true] {
            let (github, store, transport) = fixture();
            store.save("client", &saved(now().unwrap() + 3600)).unwrap();
            transport
                .no_installations
                .store(no_installations, Ordering::SeqCst);
            let error = github
                .token_for_repository("https://github.com/owner/public-but-not-granted")
                .unwrap_err();
            assert!(error.contains("repository permission is missing"));
            assert!(error.contains("Install or configure"));
            assert!(!error.contains("old-access"));
            assert!(!error.contains("refresh failed"));
            assert!(transport
                .paths
                .lock()
                .unwrap()
                .iter()
                .all(|path| path.starts_with("/user/installations")));
        }
    }

    #[test]
    fn preflight_auth_failure_stops_before_discovery_and_is_not_missing_permission() {
        for expired_refresh in [false, true] {
            let (github, store, transport) = fixture();
            let mut token = saved(1);
            if expired_refresh {
                token.refresh_expires_at = Some(1);
            } else {
                transport.fail.store(true, Ordering::SeqCst);
            }
            store.save("client", &token).unwrap();
            let error = github
                .token_for_repository("https://github.com/owner/repo")
                .unwrap_err();
            assert!(error.contains("authentication or token refresh failed"));
            if expired_refresh {
                assert!(error.contains("authorization expired. Sign in again."));
            } else {
                assert!(error.contains("token service unavailable"));
            }
            assert!(!error.contains("permission is missing"));
            assert!(!error.contains("old-access"));
            assert!(!error.contains("old-refresh"));
            assert!(transport.paths.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn preflight_discovery_failure_is_sanitized_and_not_a_missing_grant() {
        let (github, store, transport) = fixture();
        store.save("client", &saved(now().unwrap() + 3600)).unwrap();
        transport.api_fail.store(true, Ordering::SeqCst);
        let error = github
            .token_for_repository("https://github.com/owner/repo")
            .unwrap_err();
        assert_eq!(error, "Could not verify GitHub installation repository access: GitHub API is unreachable or timed out.");
        assert!(!error.contains("permission is missing"));
        assert!(!error.contains("secret"));
    }

    #[test]
    fn preflight_metadata_only_grants_stay_visible_but_require_contents_approval() {
        for permissions in [
            serde_json::json!({"metadata": "read"}),
            serde_json::json!({}),
            serde_json::json!({"contents": "none"}),
            serde_json::json!({"contents": null}),
            serde_json::json!({"contents": "untrusted-secret"}),
            serde_json::Value::Null,
        ] {
            let (github, store, transport) = fixture();
            store.save("client", &saved(1)).unwrap();
            *transport.installations.lock().unwrap() = Some(serde_json::json!([
                {"id": 1, "permissions": permissions},
                {"id": 2, "permissions": permissions}
            ]));
            let error = github
                .token_for_repository("https://github.com/owner/repo")
                .unwrap_err();
            assert!(error.contains("Contents read permission is missing"));
            assert!(error.contains("installation owner"));
            assert!(error.contains("approve"));
            assert!(error.contains("Contents Read-only permission update"));
            assert!(error.contains("reauthorize"));
            assert!(!error.contains("secret"));
            assert!(!error.contains("new-access"));
            assert!(!error.contains("old-refresh"));
            assert_eq!(transport.refreshes.load(Ordering::SeqCst), 1);
            assert!(transport
                .get_tokens
                .lock()
                .unwrap()
                .iter()
                .all(|token| token == "new-access"));
            let repositories = github.repositories().unwrap();
            assert_eq!(repositories.len(), 1);
            assert!(repositories[0].private);
        }
    }

    #[test]
    fn preflight_contents_read_or_write_allows_private_repository_after_refresh() {
        for contents in ["read", "write"] {
            let (github, store, transport) = fixture();
            store.save("client", &saved(1)).unwrap();
            // A later metadata-only duplicate must not erase the readable grant.
            *transport.installations.lock().unwrap() = Some(serde_json::json!([
                {"id": 1, "permissions": {"contents": contents}},
                {"id": 2, "permissions": {"metadata": "read"}}
            ]));
            assert_eq!(
                github
                    .token_for_repository("https://github.com/owner/repo")
                    .unwrap()
                    .as_deref(),
                Some("new-access")
            );
            assert_eq!(transport.refreshes.load(Ordering::SeqCst), 1);
            let tokens = transport.get_tokens.lock().unwrap();
            assert!(!tokens.is_empty());
            assert!(tokens.iter().all(|token| token == "new-access"));
        }
    }

    #[test]
    fn preflight_preserves_keychain_unlock_guidance_without_discovery() {
        let (github, store, transport) = fixture();
        store.fail_load.store(true, Ordering::SeqCst);
        let error = github
            .token_for_repository("https://github.com/owner/repo")
            .unwrap_err();
        assert!(error.starts_with("GitHub authentication or token refresh failed:"));
        assert!(error.contains("Unlock your keychain and retry."));
        assert!(!error.contains("Sign in"));
        assert!(transport.paths.lock().unwrap().is_empty());
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn preflight_rejects_unsafe_urls_before_credentials_or_network() {
        let (github, store, transport) = fixture();
        store.save("client", &saved(1)).unwrap();
        for url in [
            "http://github.com/owner/repo",
            "git@github.com:owner/repo.git",
            "https://github.com.evil.test/owner/repo",
            "https://evil.test/owner/repo",
            "https://secret@github.com/owner/repo",
            "https://github.com:443/owner/repo",
            "https://github.com/owner/repo?secret",
            "https://github.com/owner/repo#secret",
            "https://github.com/owner/repo/tree/main",
            "https://github.com/owner",
            "https://github.com//repo",
            "https://github.com/owner/.git",
            "https://github.com/owner/%72epo",
            "https://github.com/owner/../repo",
            "https://github.com/owner/./repo",
            "https://github.com/owner/repo//",
            "https://github.com/owner/repo\\other",
            " https://github.com/owner/repo",
            "https://github.com/owner/repo\n",
            "https://github.com/-owner/repo",
        ] {
            let error = github.token_for_repository(url).unwrap_err();
            assert!(
                error.starts_with("Invalid GitHub repository URL"),
                "{url:?}"
            );
            assert!(!error.contains("secret"));
        }
        assert_eq!(transport.refreshes.load(Ordering::SeqCst), 0);
        assert!(transport.paths.lock().unwrap().is_empty());
        assert_eq!(
            repository_name("https://GITHUB.COM/Owner/my_repo.test.git").unwrap(),
            "Owner/my_repo.test"
        );
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
                notify_device(url)?;
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
            .contains("Cannot display"));
    }
    #[test]
    fn sign_in_roundtrip_persists_profile() {
        let (github, store, _) = fixture();
        let status = github.start(notify_device).unwrap();
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
    #[test]
    fn disconnect_during_exchange_cannot_resurrect_credentials() {
        struct PausedExchange {
            inner: Arc<MockTransport>,
            entered: Arc<std::sync::Barrier>,
            resume: Arc<std::sync::Barrier>,
        }
        impl Transport for PausedExchange {
            fn device(&self, url: &Url, id: &str) -> Result<DeviceResponse, String> {
                self.inner.device(url, id)
            }
            fn avatar(&self, url: &Url) -> Result<Avatar, String> {
                self.inner.avatar(url)
            }
            fn token(&self, url: &Url, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
                assert!(fields.contains(&("grant_type", DEVICE_GRANT)));
                assert!(fields.contains(&("device_code", "private-device-code")));
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
        github.transport = Box::new(PausedExchange {
            inner: transport,
            entered: entered.clone(),
            resume: resume.clone(),
        });
        let github = Arc::new(github);
        let worker = github.clone();
        let sign_in = thread::spawn(move || worker.start(notify_device));
        entered.wait();
        github.disconnect().unwrap();
        resume.wait();
        assert_eq!(sign_in.join().unwrap().err().unwrap(), CANCELLED);
        assert!(store.load("client").unwrap().is_none());
        assert!(!github.pending.load(Ordering::SeqCst));
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
    fn oversized_token_response_is_rejected_before_reading_body() {
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
    fn real_http_token_errors_and_redirects_do_not_leak_secrets() {
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
            // HTTP is allowed only for this directly injected test endpoint; production Config uses fixed HTTPS endpoints.
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
    // Local HTTP fixtures exercise the actual form encoder and response decoder. Only
    // these tests inject HTTP endpoints; production URLs are fixed GitHub HTTPS URLs.
    struct LocalServer {
        base: String,
        requests: Arc<Mutex<Vec<(Instant, String)>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl LocalServer {
        fn new(replies: Vec<(u16, serde_json::Value)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let record = requests.clone();
            let stopping = stop.clone();
            let worker = thread::spawn(move || {
                let mut replies: std::collections::VecDeque<_> = replies.into();
                while !stopping.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("mock accept: {error}"),
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut chunk = [0; 4096];
                    loop {
                        let n = stream.read(&mut chunk).unwrap();
                        if n == 0 {
                            break;
                        }
                        bytes.extend_from_slice(&chunk[..n]);
                        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                            let header = String::from_utf8_lossy(&bytes[..end]);
                            let length: usize = header
                                .lines()
                                .filter_map(|line| line.split_once(':'))
                                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse().unwrap())
                                .unwrap_or(0);
                            if bytes.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    record
                        .lock()
                        .unwrap()
                        .push((Instant::now(), String::from_utf8(bytes).unwrap()));
                    let (status, body) = replies
                        .pop_front()
                        .unwrap_or((500, serde_json::json!({"error":"unexpected request"})));
                    let body = body.to_string();
                    let _ = write!(stream, "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }
            });
            Self {
                base,
                requests,
                stop,
                worker: Some(worker),
            }
        }
        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }
    impl Drop for LocalServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.worker.take().unwrap().join().unwrap();
        }
    }
    struct LocalTransport {
        http: Http,
        profile: Arc<MockTransport>,
    }
    impl Transport for LocalTransport {
        fn device(&self, url: &Url, id: &str) -> Result<DeviceResponse, String> {
            self.http.device(url, id)
        }
        fn token(&self, url: &Url, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
            self.http.token(url, fields)
        }
        fn poll_token(
            &self,
            url: &Url,
            fields: &[(&str, &str)],
            remaining: Duration,
        ) -> Result<TokenResponse, PollError> {
            self.http.poll_token(url, fields, remaining)
        }
        fn get(&self, path: &str, token: &str) -> Result<serde_json::Value, String> {
            self.profile.get(path, token)
        }
        fn avatar(&self, url: &Url) -> Result<Avatar, String> {
            self.profile.avatar(url)
        }
    }
    fn device_json() -> serde_json::Value {
        serde_json::json!({
            "device_code":"private-device-code", "user_code":"ABCD-EFGH",
            "verification_uri":"https://github.com/login/device", "expires_in":60, "interval":1
        })
    }
    fn local_fixture(server: &LocalServer) -> (Github, Arc<MemoryStore>, Arc<MockTransport>) {
        let (mut github, store, profile) = fixture();
        github.config = Ok(Config {
            client_id: CLIENT_ID.into(),
            device_url: Url::parse(&format!("{}/device", server.base)).unwrap(),
            token_url: Url::parse(&format!("{}/token", server.base)).unwrap(),
            app_slug: APP_SLUG.into(),
        });
        github.transport = Box::new(LocalTransport {
            http: Http::new(),
            profile: profile.clone(),
        });
        (github, store, profile)
    }
    #[test]
    fn fixed_public_configuration_and_safe_notification() {
        let config = Config::compiled().unwrap();
        assert_eq!(config.client_id, CLIENT_ID);
        assert_eq!(config.app_slug, "vellistry");
        assert_eq!(
            config.device_url.as_str(),
            "https://github.com/login/device/code"
        );
        assert_eq!(
            config.token_url.as_str(),
            "https://github.com/login/oauth/access_token"
        );
        let mut value = device_json();
        value.as_object_mut().unwrap().remove("interval");
        let device: DeviceResponse = serde_json::from_value(value).unwrap();
        let authorization = device.authorization().unwrap();
        assert_eq!(authorization.interval, 5);
        let public = serde_json::to_value(authorization.clone()).unwrap();
        assert_eq!(public.as_object().unwrap().len(), 4);
        assert!(public.get("device_code").is_none());
        assert!(!public.to_string().contains("private-device-code"));
        let (github, _, _) = fixture();
        assert_eq!(
            github.management_url().unwrap(),
            "https://github.com/apps/vellistry/installations/new"
        );
    }
    #[test]
    fn device_response_validation_and_slow_down_are_bounded() {
        for (key, bad) in [
            ("device_code", serde_json::json!("")),
            ("device_code", serde_json::json!("secret\n")),
            ("user_code", serde_json::json!("<script>")),
            (
                "verification_uri",
                serde_json::json!("https://evil.example/login/device"),
            ),
            (
                "verification_uri",
                serde_json::json!("http://github.com/login/device"),
            ),
            ("expires_in", serde_json::json!(0)),
            ("expires_in", serde_json::json!(u64::MAX)),
            ("interval", serde_json::json!(0)),
            ("interval", serde_json::json!(u64::MAX)),
        ] {
            let mut value = device_json();
            value[key] = bad;
            let device: DeviceResponse = serde_json::from_value(value).unwrap();
            assert!(device.authorization().is_err());
        }
        assert_eq!(slowed_interval(5, None).unwrap(), 10);
        assert_eq!(slowed_interval(10, Some(2)).unwrap(), 15);
        assert_eq!(slowed_interval(10, Some(30)).unwrap(), 30);
        assert!(slowed_interval(5, Some(u64::MAX)).is_err());
    }
    #[test]
    fn local_device_pending_transient_retry_and_success_use_direct_forms() {
        let server = LocalServer::new(vec![
            (200, device_json()),
            (200, serde_json::json!({"error":"authorization_pending"})),
            (503, serde_json::json!({"error":"private-secret"})),
            (
                200,
                serde_json::json!({"access_token":"new-access", "token_type":"bearer"}),
            ),
        ]);
        let (github, store, _) = local_fixture(&server);
        let notified = Instant::now();
        let status = github
            .start(|authorization| {
                assert_eq!(server.count(), 1);
                notify_device(authorization)
            })
            .unwrap();
        assert!(status.connected);
        assert_eq!(
            store.load(CLIENT_ID).unwrap().unwrap().access_token,
            "new-access"
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests[1].0.duration_since(notified) >= Duration::from_secs(1));
        assert!(requests[2].0.duration_since(requests[1].0) >= Duration::from_secs(1));
        assert!(requests[3].0.duration_since(requests[2].0) >= Duration::from_secs(2));
        for (_, request) in requests.iter() {
            let lower = request.to_ascii_lowercase();
            assert!(lower.starts_with("post "));
            assert!(lower.contains("content-type: application/x-www-form-urlencoded"));
            assert!(lower.contains("accept: application/json"));
            assert!(!lower.contains("client_secret"));
            assert!(!lower.contains("authorization:"));
            assert!(!lower.contains("scope="));
            let body = request.split_once("\r\n\r\n").unwrap().1;
            let url = Url::parse(&format!("https://example.test/?{body}")).unwrap();
            let fields: BTreeMap<_, _> = url.query_pairs().collect();
            assert_eq!(fields.get("client_id").unwrap(), CLIENT_ID);
            if request.starts_with("POST /device ") {
                assert_eq!(fields.len(), 1);
            } else {
                assert_eq!(fields.len(), 3);
                assert_eq!(fields.get("device_code").unwrap(), "private-device-code");
                assert_eq!(fields.get("grant_type").unwrap(), DEVICE_GRANT);
            }
        }
    }
    #[test]
    fn local_terminal_device_errors_do_not_persist_or_fetch_profile() {
        for (code, expected) in [
            ("access_denied", "denied"),
            ("expired_token", "expired"),
            ("incorrect_device_code", "invalid"),
            ("invalid_grant", "invalid"),
            ("invalid_device_code", "invalid"),
            ("secret-unknown-error", "rejected"),
        ] {
            let server = LocalServer::new(vec![
                (200, device_json()),
                (200, serde_json::json!({"error":code})),
            ]);
            let (github, store, profile) = local_fixture(&server);
            let error = github.start(notify_device).err().unwrap();
            assert!(error.contains(expected));
            assert!(!error.contains("secret"));
            assert!(store.load(CLIENT_ID).unwrap().is_none());
            assert!(profile.paths.lock().unwrap().is_empty());
            assert_eq!(server.count(), 2);
            assert!(!github.pending.load(Ordering::SeqCst));
        }
    }
    #[test]
    fn local_expiry_and_notification_failure_never_poll() {
        let mut device = device_json();
        device["expires_in"] = serde_json::json!(1);
        device["interval"] = serde_json::json!(2);
        let server = LocalServer::new(vec![(200, device)]);
        let (github, _, _) = local_fixture(&server);
        assert_eq!(github.start(notify_device).err().unwrap(), EXPIRED);
        assert_eq!(server.count(), 1);
        let server = LocalServer::new(vec![(200, device_json())]);
        let (github, _, _) = local_fixture(&server);
        let error = github
            .start(|_| Err("secret-channel-error".into()))
            .err()
            .unwrap();
        assert!(error.contains("Cannot display"));
        assert!(!error.contains("secret"));
        assert_eq!(server.count(), 1);
    }
    #[test]
    fn local_slow_down_wait_is_cancellable_without_further_requests() {
        let server = LocalServer::new(vec![
            (200, device_json()),
            (200, serde_json::json!({"error":"slow_down", "interval":30})),
        ]);
        let (github, store, _) = local_fixture(&server);
        let github = Arc::new(github);
        let signer = github.clone();
        let worker = thread::spawn(move || signer.start(notify_device));
        let deadline = Instant::now() + Duration::from_secs(3);
        while server.count() < 2 && Instant::now() < deadline {
            thread::sleep(POLL);
        }
        assert_eq!(server.count(), 2);
        // Allow the response to arrive and enter the long server-specified wait.
        thread::sleep(Duration::from_millis(100));
        let begin = Instant::now();
        github.cancel().unwrap();
        assert_eq!(worker.join().unwrap().err().unwrap(), CANCELLED);
        assert!(begin.elapsed() < Duration::from_millis(500));
        assert_eq!(server.count(), 2);
        assert!(store.load(CLIENT_ID).unwrap().is_none());
    }
    #[test]
    fn failed_rotation_cleanup_quarantines_until_disconnect() {
        let (github, store, _, _) = blocked_fixture(false, false);
        store.memory.save("client", &saved(1)).unwrap();
        store.memory.fail_save.store(true, Ordering::SeqCst);
        store.fail_delete.store(true, Ordering::SeqCst);
        assert!(github.access_token().is_err());
        assert!(github.access_token().unwrap_err().contains("cleaned up"));
        assert!(github.status().err().unwrap().contains("cleaned up"));
        store.fail_delete.store(false, Ordering::SeqCst);
        github.disconnect().unwrap();
        assert!(github.access_token().unwrap().is_none());
    }
    #[test]
    fn local_slow_down_adds_five_and_remains_in_force_after_pending() {
        let server = LocalServer::new(vec![
            (200, device_json()),
            (200, serde_json::json!({"error":"slow_down"})),
            (200, serde_json::json!({"error":"authorization_pending"})),
            (
                200,
                serde_json::json!({"access_token":"new-access", "token_type":"bearer"}),
            ),
        ]);
        let (github, _, _) = local_fixture(&server);
        assert!(github.start(notify_device).unwrap().connected);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for index in [2, 3] {
            assert!(
                requests[index].0.duration_since(requests[index - 1].0) >= Duration::from_secs(6)
            );
        }
    }
    #[test]
    fn http_poll_classifies_transient_and_terminal_failures_without_secrets() {
        for (status, transient) in [(429, true), (503, true), (403, false), (302, false)] {
            let server =
                LocalServer::new(vec![(status, serde_json::json!({"error":"secret-error"}))]);
            let result = Http::new().poll_token(
                &Url::parse(&server.base).unwrap(),
                &[("device_code", "secret-device-code")],
                Duration::from_secs(1),
            );
            match result {
                Err(PollError::Transient) => assert!(transient),
                Err(PollError::Fatal(error)) => {
                    assert!(!transient);
                    assert!(!error.contains("secret"));
                }
                Ok(_) => panic!("HTTP failure must not yield tokens"),
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        drop(listener);
        assert!(matches!(
            Http::new().poll_token(&endpoint, &[], Duration::from_secs(1)),
            Err(PollError::Transient)
        ));
    }
    #[test]
    fn local_invalid_device_and_ambiguous_token_payloads_fail_closed() {
        let server = LocalServer::new(vec![(
            200,
            serde_json::json!({"error":"secret-server-error"}),
        )]);
        let (github, _, profile) = local_fixture(&server);
        let error = github
            .start(|_| panic!("must not notify for invalid device response"))
            .err()
            .unwrap();
        assert!(!error.contains("secret"));
        assert_eq!(server.count(), 1);
        assert!(profile.paths.lock().unwrap().is_empty());
        for payload in [
            serde_json::json!({"access_token":"secret", "error":"authorization_pending"}),
            serde_json::json!({"access_token":"secret", "token_type":"basic"}),
            serde_json::json!({"error":"slow_down", "interval":u64::MAX}),
        ] {
            let server = LocalServer::new(vec![(200, device_json()), (200, payload)]);
            let (github, store, profile) = local_fixture(&server);
            let error = github.start(notify_device).err().unwrap();
            assert!(!error.contains("secret"));
            assert_eq!(server.count(), 2);
            assert!(store.load(CLIENT_ID).unwrap().is_none());
            assert!(profile.paths.lock().unwrap().is_empty());
        }
    }
    #[test]
    fn refresh_lost_storage_reply_and_invalid_rotation_delete_stale_tokens() {
        let (github, store, _, _) = blocked_fixture(false, false);
        store.memory.save("client", &saved(1)).unwrap();
        store.fail_after_save.store(true, Ordering::SeqCst);
        assert!(github.access_token().is_err());
        assert!(store.memory.load("client").unwrap().is_none());
        assert!(github.access_token().unwrap().is_none());

        let server = LocalServer::new(vec![(
            200,
            serde_json::json!({"access_token":"secret", "token_type":"basic"}),
        )]);
        let (github, store, _) = local_fixture(&server);
        store.save(CLIENT_ID, &saved(1)).unwrap();
        assert!(github.access_token().is_err());
        assert!(store.load(CLIENT_ID).unwrap().is_none());
        assert!(github.access_token().unwrap().is_none());
        assert_eq!(server.count(), 1);
        let requests = server.requests.lock().unwrap();
        let body = requests[0].1.split_once("\r\n\r\n").unwrap().1;
        assert!(body.contains("grant_type=refresh_token"));
        assert!(body.contains("refresh_token=old-refresh"));
        assert!(!body.contains("client_secret"));
    }
}
