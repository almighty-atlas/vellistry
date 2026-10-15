use crate::{Error, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HELPER_FLAG: &str = "--mdwell-git-credential";
const ENDPOINT_ENV: &str = "MDWELL_GIT_CREDENTIAL_ENDPOINT";
const CAPABILITY_ENV: &str = "MDWELL_GIT_CREDENTIAL_CAPABILITY";
const CREDENTIAL_LIMIT: usize = 16 * 1024;
const HELPER_START_FAILURE: &str = "mdwell: credential helper execution failed";
const HELPER_IPC_FAILURE: &str = "mdwell: credential helper protocol or IPC failed";

fn credential_error() -> Error {
    Error::GitCredentialIpc
}

/// Dispatch this before initializing the application, logging, or parsing normal
/// arguments. `Ok(false)` means normal startup; `Ok(true)` means exit successfully.
/// On `Err`, exit unsuccessfully without printing stdin or environment. Git calls
/// the same executable as `--mdwell-git-credential get|store|erase`. Store/erase
/// are deliberately no-ops: OAuth credentials never reach a persistent helper.
pub fn handle_git_credential_request() -> Result<bool> {
    let result = dispatch_git_credential_request();
    if result.is_err() {
        // A fixed marker distinguishes a running helper's protocol/IPC failure
        // from a loader/startup failure. Never print the underlying error or input.
        eprintln!("{HELPER_IPC_FAILURE}");
    }
    result
}

fn dispatch_git_credential_request() -> Result<bool> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|arg| arg != HELPER_FLAG) {
        return Ok(false);
    }
    if args.len() != 2 {
        return Err(credential_error());
    }
    match args[1].to_str() {
        Some("store" | "erase") => return Ok(true),
        Some("get") => {}
        _ => return Err(credential_error()),
    }
    use std::io::{BufRead, Read, Write};
    let mut request = Vec::new();
    let mut stdin = std::io::stdin().lock().take((CREDENTIAL_LIMIT + 1) as u64);
    loop {
        let start = request.len();
        if stdin
            .read_until(b'\n', &mut request)
            .map_err(|_| credential_error())?
            == 0
        {
            // Git terminates helper input with EOF; credential fill also accepts
            // a blank line. Normalize either form for the bounded IPC frame.
            if request.is_empty() {
                return Err(credential_error());
            }
            if !request.ends_with(b"\n") {
                request.push(b'\n');
            }
            request.push(b'\n');
            break;
        }
        if request.len() > CREDENTIAL_LIMIT {
            return Err(credential_error());
        }
        if &request[start..] == b"\n" {
            break;
        }
    }
    if request.len() > CREDENTIAL_LIMIT {
        return Err(credential_error());
    }
    if !github_credential_request(&request) {
        return Ok(true);
    }
    let endpoint: std::net::SocketAddr = std::env::var(ENDPOINT_ENV)
        .map_err(|_| credential_error())?
        .parse()
        .map_err(|_| credential_error())?;
    if endpoint.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) || endpoint.port() == 0
    {
        return Err(credential_error());
    }
    let capability = std::env::var(CAPABILITY_ENV).map_err(|_| credential_error())?;
    if capability.len() != 64 || !capability.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(credential_error());
    }
    let mut stream = std::net::TcpStream::connect_timeout(&endpoint, Duration::from_secs(2))
        .map_err(|_| credential_error())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| credential_error())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|_| credential_error())?;
    stream
        .write_all(format!("{capability}\n").as_bytes())
        .map_err(|_| credential_error())?;
    stream.write_all(&request).map_err(|_| credential_error())?;
    let response = credential_frame(&mut stream).map_err(|_| credential_error())?;
    std::io::stdout()
        .lock()
        .write_all(&response)
        .map_err(|_| credential_error())?;
    Ok(true)
}

fn github_credential_request(request: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(request) else {
        return false;
    };
    let mut protocol = None;
    let mut host = None;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        if value.bytes().any(|b| b.is_ascii_control()) {
            return false;
        }
        match key {
            "protocol" if protocol.is_none() => protocol = Some(value),
            "host" if host.is_none() => host = Some(value),
            // Challenges and capability announcements are not routing policy.
            // Modern Git's HTTP transport advertises capabilities even when
            // credential fill does not. Ignore them without opting into any.
            "path" | "wwwauth[]" | "capability[]" => {}
            "username" if value == "x-access-token" => {}
            _ => return false,
        }
    }
    protocol == Some("https") && host == Some("github.com")
}

// Bound total time as well as individual reads: a trickling local client must
// not keep a credential broker (and its in-memory token) alive indefinitely.
fn credential_frame(stream: &mut std::net::TcpStream) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let start = Instant::now();
    let mut frame = Vec::new();
    let mut chunk = [0; 1024];
    while start.elapsed() < Duration::from_secs(2) {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                frame.extend_from_slice(&chunk[..n]);
                if frame.len() > CREDENTIAL_LIMIT {
                    break;
                }
                if frame.ends_with(b"\n\n") {
                    return Ok(frame);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("invalid credential frame"))
}

struct CredentialBroker {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    delivered: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CredentialBroker {
    fn attach(command: &mut Command, token: &str) -> Result<Self> {
        let executable = std::env::current_exe().map_err(|_| Error::GitCredentialHelperStart)?;
        Self::attach_executable(command, token, &executable)
    }

    fn attach_executable(command: &mut Command, token: &str, executable: &Path) -> Result<Self> {
        use std::io::Write;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let executable = executable.to_str().ok_or(Error::GitCredentialHelperStart)?;
        if executable.chars().any(char::is_control) {
            return Err(Error::GitCredentialHelperStart);
        }
        // Git shell helpers use its POSIX shell, including Git for Windows. Quote
        // the executable as a single word; never interpolate the token in shell text.
        let executable = if cfg!(windows) {
            executable.replace('\\', "/")
        } else {
            executable.to_owned()
        };
        // Keep a shell around the re-exec so loader/startup failures produce a
        // stable marker rather than looking like an HTTP credential rejection.
        // Git appends get/store/erase; forward it without evaluating shell data.
        let helper = format!(
            "!f() {{ '{}' {HELPER_FLAG} \"$@\" || {{ printf '%s\\n' '{HELPER_START_FAILURE}' >&2; return 1; }}; }}; f",
            executable.replace('\'', "'\\''")
        );
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| credential_error())?;
        listener
            .set_nonblocking(true)
            .map_err(|_| credential_error())?;
        let endpoint = listener.local_addr().map_err(|_| credential_error())?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| credential_error())?;
        let capability: String = random.iter().map(|b| format!("{b:02x}")).collect();
        command
            .env(ENDPOINT_ENV, endpoint.to_string())
            .env(CAPABILITY_ENV, &capability)
            .args([
                "-c",
                "credential.helper=",
                "-c",
                &format!("credential.helper={helper}"),
                "-c",
                "credential.username=x-access-token",
            ]);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let delivered = Arc::new(AtomicBool::new(false));
        let delivery = delivered.clone();
        let token = token.to_owned();
        let thread = std::thread::Builder::new()
            .name("mdwell-git-credential".into())
            .spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                            if let Ok(frame) = credential_frame(&mut stream) {
                                if let Some(index) = frame.iter().position(|b| *b == b'\n') {
                                    let (key, request) = (&frame[..index], &frame[index + 1..]);
                                    if key == capability.as_bytes()
                                        && github_credential_request(request)
                                        && !stopped.load(Ordering::Acquire)
                                        && stream
                                            .write_all(
                                                format!(
                                                    "username=x-access-token\npassword={token}\n\n"
                                                )
                                                .as_bytes(),
                                            )
                                            .is_ok()
                                    {
                                        delivery.store(true, Ordering::Release);
                                    }
                                }
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| credential_error())?;
        Ok(Self {
            stop,
            delivered,
            thread: Some(thread),
        })
    }

    fn finish(self) -> bool {
        let delivered = self.delivered.clone();
        // Join the broker before reading delivery state: a completed Git child
        // must not race a successful response write that is still being recorded.
        drop(self);
        delivered.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl Drop for CredentialBroker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn validate_url(input: &str) -> Result<String> {
    // Never echo rejected input: it may contain a pasted access token.
    let invalid = || {
        Error::InvalidGithubUrl(
            "expected a GitHub HTTPS or git@github.com SSH repository URL".into(),
        )
    };
    // Match the original text, not a URL parser's normalized host, port or path.
    let (prefix, tail) = if let Some(tail) = input.strip_prefix("https://github.com/") {
        ("https://github.com/", tail)
    } else if let Some(tail) = input.strip_prefix("git@github.com:") {
        ("git@github.com:", tail)
    } else if let Some(tail) = input.strip_prefix("ssh://git@github.com/") {
        ("git@github.com:", tail)
    } else {
        return Err(invalid());
    };
    let tail = tail.strip_suffix('/').unwrap_or(tail);
    let parts: Vec<_> = tail.split('/').collect();
    if parts.len() != 2 {
        return Err(invalid());
    }
    let owner = parts[0];
    let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if owner.is_empty()
        || owner.len() > 39
        || owner.starts_with('-')
        || owner.ends_with('-')
        || owner.contains("--")
        || !owner
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || repo.is_empty()
        || repo.len() > 100
        || matches!(repo, "." | "..")
        || !repo
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    {
        return Err(invalid());
    }
    Ok(format!("{prefix}{owner}/{repo}.git"))
}

pub(crate) fn run(root: &Path, args: &[&str]) -> Result<String> {
    run_with_protocol(root, args, false)
}

// Keep the returned directory alive until the Git child has exited. A real empty
// hooks directory works on both Unix and Windows without null-device semantics.
fn isolate_config(command: &mut Command) -> Result<tempfile::TempDir> {
    let settings = tempfile::Builder::new().prefix("mdwell-git-").tempdir()?;
    let hooks = settings.path().join("hooks");
    let global_config = settings.path().join("global.config");
    std::fs::create_dir(&hooks)?;
    std::fs::write(&global_config, b"")?;
    let mut hooks_config = std::ffi::OsString::from("core.hooksPath=");
    hooks_config.push(&hooks);
    // Do not inherit injected Git config, alternate worktrees, or credential commands.
    clear_git_env(command);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &global_config)
        .env("GIT_TERMINAL_PROMPT", "0")
        // Empty askpass prevents fallback to configured or SSH askpass programs;
        // terminal prompts are separately disabled, with stdin closed by the caller.
        .env("GIT_ASKPASS", "")
        .arg("-c")
        .arg(hooks_config);
    Ok(settings)
}

fn run_with_protocol(root: &Path, args: &[&str], allow_file: bool) -> Result<String> {
    let trusted = trusted_config()?;
    run_with_settings(root, args, allow_file, &trusted)
}

pub(crate) fn run_with_token(
    root: &Path,
    args: &[&str],
    url: &str,
    token: Option<&str>,
) -> Result<String> {
    let token = https_token(url, token)?;
    match token {
        Some(token) => {
            let trusted = trusted_config_for(true)?;
            run_with_oauth_settings(root, args, false, &trusted, Some(token))
        }
        None => run(root, args),
    }
}

fn https_token<'a>(url: &str, token: Option<&'a str>) -> Result<Option<&'a str>> {
    let url = validate_url(url)?;
    if !url.starts_with("https://github.com/") {
        return Ok(None);
    }
    if let Some(token) = token {
        if token.is_empty() || token.len() > 8192 || !token.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(Error::Git("invalid OAuth credential".into()));
        }
    }
    Ok(token)
}

fn run_with_settings(
    root: &Path,
    args: &[&str],
    allow_file: bool,
    trusted: &[(String, String)],
) -> Result<String> {
    run_with_oauth_settings(root, args, allow_file, trusted, None)
}

fn run_with_oauth_settings(
    root: &Path,
    args: &[&str],
    allow_file: bool,
    trusted: &[(String, String)],
    token: Option<&str>,
) -> Result<String> {
    let (mut command, _settings) = authenticated_command(root, allow_file, trusted)?;
    let broker = token
        .filter(|_| matches!(args.first().copied(), Some("clone" | "fetch")))
        .map(|token| CredentialBroker::attach(&mut command, token))
        .transpose()?;
    command.args(args);
    let operation = operation(args);
    let (success, output, stderr) = command_output(command, Duration::from_secs(120), operation)?;
    let delivered = broker.map(CredentialBroker::finish);
    if !success {
        let error = classify_error(&String::from_utf8_lossy(&stderr), operation);
        return Err(
            if matches!(error, Error::GitAuthentication) && delivered == Some(false) {
                Error::GitCredentialIpc
            } else {
                error
            },
        );
    }
    String::from_utf8(output)
        .map_err(|_| Error::Git(format!("git {operation} returned non-UTF-8 output")))
}

fn authenticated_command(
    root: &Path,
    allow_file: bool,
    trusted: &[(String, String)],
) -> Result<(Command, tempfile::TempDir)> {
    check_local_config(root)?;
    let mut command = Command::new("git");
    command.current_dir(root);
    let settings = isolate_config(&mut command)?;
    command.env("GIT_CEILING_DIRECTORIES", root);
    // Environment config keeps trusted helper definitions out of process arguments
    // and off disk. Local executable settings were rejected before reaching here.
    command.env("GIT_CONFIG_COUNT", trusted.len().to_string());
    for (index, (key, value)) in trusted.iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{index}"), key);
        command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    command
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes",
        )
        .env("GIT_SSH_VARIANT", "ssh")
        .env("SSH_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "-c",
            "credential.interactive=false",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.sslVerify=true",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.ssh.allow=always",
            "-c",
            "core.attributesFile=",
            "-c",
            "init.templateDir=",
            "-c",
            if allow_file {
                "protocol.file.allow=always"
            } else {
                "protocol.file.allow=never"
            },
        ]);
    Ok((command, settings))
}

// Read configuration only; this never invokes helpers or other configured commands.
// Explicit scopes and an outside-repository cwd prevent local includes from being
// mistaken for trusted settings. Git itself resolves trusted includes and XDG paths.
fn trusted_config() -> Result<Vec<(String, String)>> {
    trusted_config_for(false)
}

fn trusted_config_for(oauth: bool) -> Result<Vec<(String, String)>> {
    let outside = tempfile::Builder::new().prefix("mdwell-auth-").tempdir()?;
    let mut settings = Vec::new();
    for scope in ["--system", "--global"] {
        let mut command = Command::new("git");
        clear_git_env(&mut command);
        // Respect explicit user-selected trusted config files, not injected -c config.
        for key in [
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let no_system = std::env::var("GIT_CONFIG_NOSYSTEM")
            .map(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "" | "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(false);
        if scope == "--system" && no_system {
            continue;
        }
        command.env("GIT_CEILING_DIRECTORIES", outside.path());
        command.current_dir(outside.path()).args([
            "config",
            scope,
            "--includes",
            "--null",
            "--list",
        ]);
        let (success, output, error) = config_output(command)?;
        if !success {
            // Git reports absent default config files as an error. Other failures
            // (including broken includes) must not silently discard authentication.
            let error = String::from_utf8_lossy(&error);
            let explicit = if scope == "--system" {
                "GIT_CONFIG_SYSTEM"
            } else {
                "GIT_CONFIG_GLOBAL"
            };
            if output.is_empty()
                && std::env::var_os(explicit).is_none()
                && error.contains("unable to read config file")
                && error.contains("No such file or directory")
            {
                continue;
            }
            return Err(Error::Git(
                "could not read trusted Git configuration".into(),
            ));
        }
        for (key, value) in parse_config(&output)? {
            if trusted_key(&key) && !(oauth && key.to_ascii_lowercase().starts_with("credential."))
            {
                if key.eq_ignore_ascii_case("credential.credentialstore") {
                    validate_credential_store(&value)?;
                }
                settings.push((key, value));
            }
        }
    }
    Ok(settings)
}

fn clear_git_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_")
            || key.to_string_lossy().starts_with("MDWELL_GIT_CREDENTIAL_")
        {
            command.env_remove(key);
        }
    }
    command.env("LC_ALL", "C");
}

fn trusted_key(key: &str) -> bool {
    // No URL rewrites, SSH commands, filters, includes, headers, cookies, or
    // credential values. Helper definitions are trusted executable user policy.
    let lower = key.to_ascii_lowercase();
    let field = lower.rsplit('.').next().unwrap_or("");
    lower == "credential.credentialstore"
        || (lower.starts_with("credential.")
            && matches!(field, "helper" | "username" | "usehttppath"))
        || (lower.starts_with("http.")
            && matches!(
                field,
                "proxy"
                    | "proxyauthmethod"
                    | "noproxy"
                    | "sslcainfo"
                    | "sslcapath"
                    | "sslbackend"
                    | "schannelusesslcainfo"
                    | "sslversion"
            ))
}

fn validate_credential_store(value: &str) -> Result<()> {
    // Preserve GCM's user-selected OS/encrypted stores and memory-only cache,
    // never its plaintext backend. Fail closed rather than silently falling back.
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "secretservice" | "gpg" | "wincred" | "dpapi" | "keychain" | "cache"
    ) {
        Ok(())
    } else {
        Err(Error::Git("unsupported GCM credential store; select secretservice, gpg, wincred, dpapi, keychain, or cache in trusted Git configuration (plaintext is not supported)".into()))
    }
}

fn parse_config(bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::Git("Git configuration is not UTF-8".into()))?;
    text.split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
            Ok((key.to_owned(), value.to_owned()))
        })
        .collect()
}

// Do not modify user/repository config, even temporarily. Reject local settings
// outside a small data-only allowlist: in particular includes, credential helpers,
// HTTP policy, URL rewrites, filters, extensions and transport commands cannot run.
fn check_local_config(root: &Path) -> Result<()> {
    let git_dir = root.join(".git");
    if !git_dir.exists() {
        return Ok(());
    }
    if !git_dir.is_dir() || git_dir.symlink_metadata()?.file_type().is_symlink() {
        return Err(Error::Git(
            "linked Git directories are not supported".into(),
        ));
    }
    let mut command = Command::new("git");
    let _settings = isolate_config(&mut command)?;
    command
        .current_dir(root)
        .args(["config", "--local", "--no-includes", "--null", "--list"]);
    let (success, output, _) = config_output(command)?;
    if !success {
        return Err(Error::Git(
            "could not inspect repository Git configuration".into(),
        ));
    }
    for (key, value) in parse_config(&output)? {
        let key = key.to_ascii_lowercase();
        let allowed = matches!(
            key.as_str(),
            "core.repositoryformatversion"
                | "core.filemode"
                | "core.bare"
                | "core.logallrefupdates"
                | "core.ignorecase"
                | "core.precomposeunicode"
                | "core.hookspath"
                | "core.fsmonitor"
                | "user.name"
                | "user.email"
        ) || matches!(key.as_str(), "remote.origin.url" | "remote.origin.fetch")
            // Git persists this data-only setting for our own --no-tags clone.
            || (key == "remote.origin.tagopt" && value == "--no-tags")
            || (key.starts_with("branch.")
                && (key.ends_with(".remote") || key.ends_with(".merge")));
        if !allowed {
            return Err(Error::Git(
                "unsafe repository-local Git configuration; remove custom settings before retrying"
                    .into(),
            ));
        }
    }
    Ok(())
}

// Config can contain credentials even in settings we discard. Capture in memory,
// never a temporary file, and bound both runtime and retained output.
fn config_output(command: Command) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    command_output(command, Duration::from_secs(10), "config")
}

fn command_output(
    mut command: Command,
    timeout: Duration,
    operation: &str,
) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    use std::io::Read;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| Error::Git(format!("could not start git {operation}")))?;
    fn reader(
        mut stream: impl Read + Send + 'static,
    ) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
        std::thread::spawn(move || {
            let mut result = Vec::new();
            let mut chunk = [0; 8192];
            loop {
                let n = stream.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                if result.len() + n <= 1024 * 1024 {
                    result.extend_from_slice(&chunk[..n]);
                } else {
                    return Err(std::io::Error::other("Git output exceeds limit"));
                }
            }
            Ok(result)
        })
    }
    let stdout = reader(child.stdout.take().expect("piped stdout"));
    let stderr = reader(child.stderr.take().expect("piped stderr"));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| Error::Git(format!("could not wait for git {operation}")))?
        {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Git(format!(
                "git {operation} timed out after {} seconds",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // A helper may leave a descendant holding an output pipe after Git exits.
    // Do not let joining readers bypass the command's deadline.
    while !stdout.is_finished() || !stderr.is_finished() {
        if start.elapsed() > timeout {
            return Err(Error::Git(format!(
                "git {operation} output timed out after {} seconds",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let read = |thread: std::thread::JoinHandle<std::io::Result<Vec<u8>>>| {
        thread
            .join()
            .ok()
            .and_then(std::result::Result::ok)
            .ok_or_else(|| {
                Error::Git(format!(
                    "could not read git {operation} output (limit: 1 MiB per stream)"
                ))
            })
    };
    Ok((status.success(), read(stdout)?, read(stderr)?))
}

fn operation(args: &[&str]) -> &'static str {
    match args.first().copied() {
        Some("clone") => "clone",
        Some("fetch") => "fetch",
        Some("merge") => "merge",
        Some("status") => "status",
        Some("symbolic-ref") => "symbolic-ref",
        Some("check-ref-format") => "check-ref-format",
        _ => "command",
    }
}

fn classify_error(stderr: &str, operation: &str) -> Error {
    let text = stderr.to_ascii_lowercase();
    if text
        .lines()
        .any(|line| line.eq_ignore_ascii_case(HELPER_IPC_FAILURE))
    {
        Error::GitCredentialIpc
    } else if text
        .lines()
        .any(|line| line.eq_ignore_ascii_case(HELPER_START_FAILURE))
    {
        Error::GitCredentialHelperStart
    } else if [
        "host key verification failed",
        "remote host identification has changed",
        "no matching host key",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Error::GitSshHostVerification
    } else if [
        "repository not found",
        "access denied",
        "403",
        "404",
        "permission to ",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Error::GitPermissionDenied
    } else if [
        "authentication failed",
        "could not read username",
        "could not read password",
        "unable to get password from user",
        "could not read from user",
        "terminal prompts disabled",
        "permission denied (publickey",
        "no supported authentication methods",
        "invalid username or token",
        "401",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Error::GitAuthentication
    } else if [
        "could not resolve",
        "failed to connect",
        "connection refused",
        "connection timed out",
        "network is unreachable",
        "could not read from remote repository",
        "ssl certificate problem",
        "server certificate verification failed",
        "ssl connect error",
        "tls connect error",
        "gnutls_handshake",
        "connection reset",
        "operation timed out",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Error::GitNetwork
    } else {
        Error::Git(format!(
            "git {operation} failed; diagnostic output withheld because it may contain credentials"
        ))
    }
}

pub(crate) fn sync(root: &Path, url: &str) -> Result<()> {
    sync_with_protocol(root, url, false)
}

pub(crate) fn sync_with_token(root: &Path, url: &str, token: Option<&str>) -> Result<()> {
    match https_token(url, token)? {
        Some(token) => sync_with_oauth(root, url, false, Some(token)),
        None => sync(root, url),
    }
}

fn sync_with_protocol(root: &Path, url: &str, allow_file: bool) -> Result<()> {
    sync_with_oauth(root, url, allow_file, None)
}

fn sync_with_oauth(root: &Path, url: &str, allow_file: bool, token: Option<&str>) -> Result<()> {
    let run = |root: &Path, args: &[&str], allow_file: bool| {
        if let Some(token) = token {
            run_with_oauth_settings(
                root,
                args,
                allow_file,
                &trusted_config_for(true)?,
                Some(token),
            )
        } else {
            run_with_protocol(root, args, allow_file)
        }
    };
    if !run(
        root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        allow_file,
    )?
    .trim()
    .is_empty()
    {
        return Err(Error::DirtyCheckout);
    }
    let branch = run(root, &["symbolic-ref", "--short", "HEAD"], allow_file)?;
    let branch = branch.trim();
    let reference = format!("refs/heads/{branch}");
    run(root, &["check-ref-format", &reference], allow_file)?;
    // Fetch the persisted, validated URL rather than trusting a changed origin remote.
    run(
        root,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--",
            url,
            &reference,
        ],
        allow_file,
    )?;
    // Recheck in case files were edited while fetch was running.
    if !run(
        root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        allow_file,
    )?
    .trim()
    .is_empty()
    {
        return Err(Error::DirtyCheckout);
    }
    run(
        root,
        &[
            "merge",
            "--ff-only",
            "--no-overwrite-ignore",
            "--quiet",
            "FETCH_HEAD",
        ],
        allow_file,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture_git(root: &Path, args: &[&str]) {
        let mut command = Command::new("git");
        let _settings = isolate_config(&mut command).unwrap();
        assert!(command
            .current_dir(root)
            .stdin(Stdio::null())
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }

    #[cfg(unix)]
    #[test]
    fn real_git_helper_start_failure_is_not_authentication_rejection() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        // An existing executable whose runtime interpreter is unavailable models a
        // desktop binary that cannot be re-executed in Git's runtime environment.
        let host = temp.path().join("host");
        fs::write(&host, "#!/mdwell-fixture-missing-interpreter\n").unwrap();
        fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
        let (mut command, _settings) = authenticated_command(temp.path(), false, &[]).unwrap();
        let _broker =
            CredentialBroker::attach_executable(&mut command, "fixture-secret", &host).unwrap();
        command.args(["credential", "fill"]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            b"protocol=https\nhost=github.com\nwwwauth[]=Basic realm=GitHub\n\n",
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert_eq!(
            classify_error(&String::from_utf8_lossy(&output.stderr), "clone")
                .info()
                .code,
            crate::ErrorCode::GitCredentialHelperStart,
            "helper runtime failure must not be reported as rejected credentials"
        );
    }

    #[test]
    fn oauth_brokers_are_scoped_authenticated_bounded_and_ephemeral() {
        use std::io::{Read, Write};
        let temp = tempfile::tempdir().unwrap();
        let (mut command, settings) = authenticated_command(temp.path(), false, &[]).unwrap();
        let token = "unit-fixture-token";
        let broker = CredentialBroker::attach(&mut command, token).unwrap();
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        let endpoint = env[std::ffi::OsStr::new(ENDPOINT_ENV)]
            .unwrap()
            .to_str()
            .unwrap();
        let capability = env[std::ffi::OsStr::new(CAPABILITY_ENV)]
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(capability.len(), 64);
        assert!(!command
            .get_args()
            .any(|arg| arg.to_string_lossy().contains(token)));
        assert!(!command
            .get_envs()
            .any(|(_, value)| value.is_some_and(|v| v.to_string_lossy().contains(token))));
        assert_eq!(
            fs::read(settings.path().join("global.config")).unwrap(),
            b""
        );
        let request = |key: &str, input: &str| {
            let mut stream = std::net::TcpStream::connect(endpoint).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .write_all(format!("{key}\n{input}\n\n").as_bytes())
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        };
        assert!(request(&"00".repeat(32), "protocol=https\nhost=github.com").is_empty());
        for input in [
            "protocol=http\nhost=github.com",
            "protocol=https\nhost=evil.invalid",
            "protocol=https\nhost=github.com:443",
            "protocol=https\nhost=github.com\npassword=injected",
        ] {
            assert!(request(capability, input).is_empty());
        }
        assert!(!broker.delivered.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            request(capability, "protocol=https\nhost=github.com"),
            format!("username=x-access-token\npassword={token}\n\n")
        );
        assert!(broker.delivered.load(std::sync::atomic::Ordering::Acquire));
        let (mut other, _settings) = authenticated_command(temp.path(), false, &[]).unwrap();
        let other_broker = CredentialBroker::attach(&mut other, "different-token").unwrap();
        let other_env: std::collections::HashMap<_, _> = other.get_envs().collect();
        assert_ne!(
            env[std::ffi::OsStr::new(ENDPOINT_ENV)],
            other_env[std::ffi::OsStr::new(ENDPOINT_ENV)]
        );
        assert_ne!(
            env[std::ffi::OsStr::new(CAPABILITY_ENV)],
            other_env[std::ffi::OsStr::new(CAPABILITY_ENV)]
        );
        let endpoint: std::net::SocketAddr = endpoint.parse().unwrap();
        let mut stalled = std::net::TcpStream::connect(endpoint).unwrap();
        stalled.write_all(b"partial").unwrap();
        let start = Instant::now();
        drop(broker);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(
            std::net::TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).is_err()
        );
        drop(other_broker);
    }

    #[test]
    fn oauth_is_only_selected_for_valid_github_https_and_safe_tokens() {
        assert_eq!(
            https_token("https://github.com/owner/repo", Some("safe-token")).unwrap(),
            Some("safe-token")
        );
        for url in [
            "git@github.com:owner/repo",
            "ssh://git@github.com/owner/repo",
        ] {
            assert_eq!(https_token(url, Some("ignored\ninvalid")).unwrap(), None);
        }
        assert_eq!(
            https_token("https://github.com/owner/repo", None).unwrap(),
            None
        );
        assert!(https_token("https://evil.invalid/owner/repo", Some("safe-token")).is_err());
        for token in [
            "",
            "line\nsecret",
            "line\rsecret",
            "nul\0secret",
            "white space",
        ] {
            assert!(https_token("https://github.com/owner/repo", Some(token)).is_err());
        }
        assert!(https_token("https://github.com/owner/repo", Some(&"x".repeat(8193))).is_err());
    }

    #[test]
    fn isolated_config_uses_real_empty_paths_and_disables_prompts() {
        let mut command = Command::new("git");
        let settings = isolate_config(&mut command).unwrap();
        let hooks = settings.path().join("hooks");
        let global = settings.path().join("global.config");
        assert!(hooks.is_dir());
        assert_eq!(fs::read_dir(&hooks).unwrap().count(), 0);
        assert_eq!(fs::metadata(&global).unwrap().len(), 0);
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "GIT_ASKPASS" && value == Some(std::ffi::OsStr::new(""))));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "GIT_TERMINAL_PROMPT"
                && value == Some(std::ffi::OsStr::new("0"))));
        let output = command
            .current_dir(settings.path())
            .stdin(Stdio::null())
            .args(["config", "--get", "core.hooksPath"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            Path::new(String::from_utf8(output.stdout).unwrap().trim()),
            hooks
        );
        let settings_path = settings.path().to_owned();
        drop(settings);
        assert!(!settings_path.exists());
    }

    #[test]
    fn trusted_helpers_and_scoped_reset_use_real_git_credential_fill() {
        use std::io::Write;
        // A subprocess isolates HOME/config overrides from parallel Rust tests.
        if std::env::var_os("MDWELL_AUTH_TEST_CHILD").is_none() {
            let temp = tempfile::tempdir().unwrap();
            let system = temp.path().join("system");
            let global = temp.path().join("global");
            let include = temp.path().join("included");
            fs::write(&system, "[credential]\ncredentialStore = cache\n").unwrap();
            fixture_git(
                temp.path(),
                &[
                    "config",
                    "--file",
                    system.to_str().unwrap(),
                    "credential.helper",
                    "!f() { printf 'username=system\\npassword=dummy-system\\n'; }; f",
                ],
            );
            fs::write(&global, format!("[include]\npath = {}\n[http]\nextraHeader = Authorization: secret-not-to-copy\n[core]\nsshCommand = must-not-run\n[url \"https://evil.invalid/\"]\ninsteadOf = https://github.com/\n", include.display())).unwrap();
            fs::write(
                &include,
                "[credential]\ncredentialStore = secretservice\n[credential \"https://github.com\"]\nhelper =\nuseHttpPath = true\n",
            )
            .unwrap();
            fixture_git(
                temp.path(),
                &[
                    "config",
                    "--file",
                    include.to_str().unwrap(),
                    "--add",
                    "credential.https://github.com.helper",
                    "!f() { git config --get credential.credentialStore '^secretservice$' >/dev/null || return 1; printf 'username=fixture\\npassword=dummy-global\\n'; }; f",
                ],
            );
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "git::tests::trusted_helpers_and_scoped_reset_use_real_git_credential_fill",
                ])
                .env("MDWELL_AUTH_TEST_CHILD", "1")
                .env("GIT_CONFIG_SYSTEM", system)
                .env("GIT_CONFIG_GLOBAL", global)
                .env_remove("GIT_CONFIG_NOSYSTEM")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let trusted = trusted_config().unwrap();
        assert!(trusted_config_for(true)
            .unwrap()
            .iter()
            .all(|(key, _)| !key.to_ascii_lowercase().starts_with("credential.")));
        let stores: Vec<_> = trusted
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("credential.credentialstore"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(stores, ["cache", "secretservice"]);
        assert_eq!(
            trusted
                .iter()
                .filter(|(key, _)| key.ends_with("helper"))
                .count(),
            3
        );
        assert!(!trusted.iter().any(|(key, _)| key.starts_with("url.")
            || key == "core.sshcommand"
            || key == "http.extraheader"));
        let temp = tempfile::tempdir().unwrap();
        let (mut command, settings) = authenticated_command(temp.path(), false, &trusted).unwrap();
        command
            .args(["credential", "fill"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"protocol=https\nhost=github.com\npath=owner/repo.git\n\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains("username=fixture"));
        assert!(output.contains("password=dummy-global"));
        assert!(!output.contains("dummy-system"));
        assert_eq!(
            fs::read(settings.path().join("global.config")).unwrap(),
            b""
        );
    }

    #[test]
    fn credential_store_validation_is_narrow_and_sanitized() {
        for store in [
            "secretservice",
            "gpg",
            "wincred",
            "dpapi",
            "keychain",
            "cache",
            "SecretService",
        ] {
            assert!(validate_credential_store(store).is_ok());
        }
        for store in [
            "plaintext",
            "none",
            "",
            "unknown-sensitive-value",
            "gpg; arbitrary-command",
            "gpg\npassword=dummy-sensitive-value",
        ] {
            let error = validate_credential_store(store).unwrap_err();
            assert!(matches!(error, Error::Git(_)));
            assert!(!error.to_string().contains("sensitive-value"));
            assert!(!error.to_string().contains("arbitrary-command"));
        }
        assert!(!trusted_key(
            "credential.https://github.com.credentialStore"
        ));
        assert!(!trusted_key("credential.plaintextStorePath"));
    }

    #[test]
    fn dangerous_local_config_is_rejected_without_execution() {
        let temp = tempfile::tempdir().unwrap();
        fixture_git(temp.path(), &["init", "-b", "main"]);
        let config = temp.path().join(".git/config");
        let original = fs::read_to_string(&config).unwrap();
        let marker = temp.path().join("executed");
        for addition in [
            format!("[credential]\nhelper = !touch {}\n", marker.display()),
            format!(
                "[credential \"https://github.com\"]\nhelper = !touch {}\n",
                marker.display()
            ),
            format!("[core]\nsshCommand = touch {}\n", marker.display()),
            "[url \"ext::touch marker\"]\ninsteadOf = https://github.com/\n".into(),
            "[include]\npath = untrusted.config\n".into(),
            "[http]\nsslVerify = false\n".into(),
            "[credential]\ncredentialStore = secretservice\n".into(),
            "[filter \"bad\"]\nprocess = must-not-run\n".into(),
        ] {
            fs::write(&config, format!("{original}\n{addition}")).unwrap();
            assert!(matches!(
                run_with_settings(temp.path(), &["status", "--porcelain"], false, &[]),
                Err(Error::Git(_))
            ));
            assert!(!marker.exists());
        }
    }

    #[test]
    fn ssh_and_protocol_policy_cannot_be_weakened_by_trusted_config() {
        let temp = tempfile::tempdir().unwrap();
        let (command, _) = authenticated_command(temp.path(), false, &[]).unwrap();
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env[std::ffi::OsStr::new("GIT_SSH_COMMAND")],
            Some(std::ffi::OsStr::new(
                "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes"
            ))
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        for setting in [
            "protocol.allow=never",
            "protocol.file.allow=never",
            "protocol.ssh.allow=always",
            "core.fsmonitor=false",
            "submodule.recurse=false",
            "http.sslVerify=true",
        ] {
            assert!(args.iter().any(|arg| arg == setting));
        }
        for key in [
            "http.extraheader",
            "http.cookiefile",
            "http.sslverify",
            "core.sshcommand",
            "url.https://evil/.insteadof",
            "credential.password",
            "credential.https://github.com.password",
        ] {
            assert!(!trusted_key(key), "{key}");
        }
        for key in [
            "credential.helper",
            "credential.credentialStore",
            "credential.https://github.com.helper",
            "http.https://github.com.proxy",
            "http.sslcainfo",
        ] {
            assert!(trusted_key(key), "{key}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn ssh_transport_uses_batch_and_strict_host_checking_without_network() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join("ssh");
        let captured = temp.path().join("args");
        fs::write(&ssh, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$MDWELL_SSH_ARGS\"\nprintf 'Host key verification failed. dummy-sensitive-value' >&2\nexit 255\n").unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        let (mut command, _settings) = authenticated_command(temp.path(), false, &[]).unwrap();
        let mut paths = vec![temp.path().to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("MDWELL_SSH_ARGS", &captured)
            .args(["ls-remote", "--", "ssh://git@github.com/owner/repo.git"])
            .stdin(Stdio::null());
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let args = fs::read_to_string(captured).unwrap();
        for expected in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "git@github.com",
            "owner/repo.git",
        ] {
            assert!(args.contains(expected), "missing {expected}");
        }
        assert!(matches!(
            classify_error(&String::from_utf8_lossy(&output.stderr), "fetch"),
            Error::GitSshHostVerification
        ));
    }

    #[test]
    fn errors_are_classified_without_echoing_secrets() {
        use crate::ErrorCode;
        for (text, code) in [
            ("Authentication failed", ErrorCode::GitAuthentication),
            (HELPER_START_FAILURE, ErrorCode::GitCredentialHelperStart),
            (HELPER_IPC_FAILURE, ErrorCode::GitCredentialIpc),
            ("mdwell: credential helper protocol or IPC failed\nmdwell: credential helper execution failed\nfatal: could not read password", ErrorCode::GitCredentialIpc),
            (
                "fatal: unable to get password from user",
                ErrorCode::GitAuthentication,
            ),
            (
                "fatal: could not read from user",
                ErrorCode::GitAuthentication,
            ),
            (
                "Permission denied (publickey).",
                ErrorCode::GitAuthentication,
            ),
            ("Repository not found", ErrorCode::GitPermissionDenied),
            (
                "requested URL returned error: 403",
                ErrorCode::GitPermissionDenied,
            ),
            ("Could not resolve host", ErrorCode::GitNetwork),
            ("SSL certificate problem: unable to get local issuer certificate", ErrorCode::GitNetwork),
            ("server certificate verification failed", ErrorCode::GitNetwork),
            ("gnutls_handshake() failed", ErrorCode::GitNetwork),
            ("SSL connect error", ErrorCode::GitNetwork),
            ("TLS connect error", ErrorCode::GitNetwork),
            ("requested URL returned error: 401", ErrorCode::GitAuthentication),
            (
                "Host key verification failed",
                ErrorCode::GitSshHostVerification,
            ),
            (
                "REMOTE HOST IDENTIFICATION HAS CHANGED",
                ErrorCode::GitSshHostVerification,
            ),
            ("something unexpected", ErrorCode::Git),
        ] {
            let error = classify_error(
                &format!("{text}\npassword=secret-token https://secret@github.com/a/b"),
                "fetch",
            );
            assert_eq!(error.info().code, code);
            let serialized = serde_json::to_string(&error).unwrap();
            assert!(!serialized.contains("secret"));
            assert!(!serialized.contains("https://"));
        }
        let error = validate_url("https://token-secret@github.com/a/b").unwrap_err();
        assert!(!error.to_string().contains("token-secret"));
    }

    #[test]
    fn process_failures_and_timeouts_identify_operation_without_arguments() {
        let mut missing = Command::new("mdwell-test-missing-executable");
        missing.arg("dummy-sensitive-value");
        let error = command_output(missing, Duration::from_secs(1), "fetch").unwrap_err();
        assert_eq!(error.to_string(), "git failed: could not start git fetch");
        #[cfg(unix)]
        {
            let mut sleeping = Command::new("sh");
            sleeping.args(["-c", "exec sleep 1"]);
            let error = command_output(sleeping, Duration::from_millis(20), "clone").unwrap_err();
            assert!(error.to_string().contains("git clone timed out"));
            assert!(!error.to_string().contains("exec sleep"));
        }
    }

    #[test]
    fn strict_urls() {
        assert_eq!(
            validate_url("https://github.com/owner/repo").unwrap(),
            "https://github.com/owner/repo.git"
        );
        for url in [
            "git@github.com:owner/repo",
            "ssh://git@github.com/owner/repo.git",
            "git@github.com:owner/repo.git/",
        ] {
            assert_eq!(validate_url(url).unwrap(), "git@github.com:owner/repo.git");
        }
        for url in [
            "http://github.com/a/b",
            "https://user@github.com/a/b",
            "https://github.com:443/a/b",
            "https://github.com/a/b?x",
            "https://github.com/a/b#x",
            "https://github.com/a/../b/c",
            "https://github.com/a/%62",
            "https://evil.test/a/b",
            "git@evil.test:a/b",
            "ssh://git@github.com:22/a/b",
            "ssh://user@github.com/a/b",
            "ssh://git:secret@github.com/a/b",
            "git@github.com:a/b?token=secret",
            "https://github.com/a/b/tree/main",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn real_clone_no_tags_configuration_is_safe_for_sync() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fixture_git(&source, &["init", "-b", "main"]);
        fs::write(source.join("README.md"), "# Fixture\n").unwrap();
        fixture_git(&source, &["add", "."]);
        fixture_git(&source, &["commit", "-m", "fixture"]);
        let checkout = temp.path().join("checkout");
        let url = format!("file://{}", source.display());
        run_with_protocol(
            temp.path(),
            &[
                "clone",
                "--quiet",
                "--depth=1",
                "--single-branch",
                "--no-tags",
                "--",
                &url,
                checkout.to_str().unwrap(),
            ],
            true,
        )
        .unwrap();
        assert!(
            check_local_config(&checkout).is_ok(),
            "core must accept configuration produced by its own clone flags"
        );
        sync_with_protocol(&checkout, &url, true).unwrap();
        fixture_git(&checkout, &["config", "remote.origin.tagopt", "unexpected"]);
        assert!(check_local_config(&checkout).is_err());
    }

    #[test]
    fn local_fixture_fast_forward_dirty_and_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let upstream = temp.path().join("upstream");
        fs::create_dir(&upstream).unwrap();
        fixture_git(&upstream, &["init", "-b", "main"]);
        fs::write(upstream.join("doc.md"), "# One").unwrap();
        fixture_git(&upstream, &["add", "."]);
        fixture_git(&upstream, &["commit", "-m", "one"]);
        let checkout = temp.path().join("checkout");
        run_with_protocol(
            temp.path(),
            &[
                "clone",
                "--quiet",
                "--depth=1",
                &format!("file://{}", upstream.display()),
                checkout.to_str().unwrap(),
            ],
            true,
        )
        .unwrap();
        let marker = temp.path().join("hook-ran");
        let hook = checkout.join(".git/hooks/post-merge");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fixture_git(
            &checkout,
            &["config", "core.fsmonitor", hook.to_str().unwrap()],
        );
        fs::write(upstream.join("doc.md"), "# Two").unwrap();
        fixture_git(&upstream, &["add", "."]);
        fixture_git(&upstream, &["commit", "-m", "two"]);
        let remote = format!("file://{}", upstream.display());
        sync_with_protocol(&checkout, &remote, true).unwrap();
        assert_eq!(
            fs::read_to_string(checkout.join("doc.md")).unwrap(),
            "# Two"
        );
        assert!(!marker.exists());
        fs::write(checkout.join("untracked.md"), "dirty").unwrap();
        assert!(matches!(
            sync_with_protocol(&checkout, &remote, true),
            Err(Error::DirtyCheckout)
        ));
        fs::remove_file(checkout.join("untracked.md")).unwrap();
        fs::write(checkout.join("doc.md"), "unsaved edit").unwrap();
        assert!(matches!(
            sync_with_protocol(&checkout, &remote, true),
            Err(Error::DirtyCheckout)
        ));
        fs::write(checkout.join("doc.md"), "# Two").unwrap();
        fixture_git(&checkout, &["config", "--unset", "core.fsmonitor"]);
        fs::write(checkout.join("local.md"), "local").unwrap();
        fixture_git(&checkout, &["add", "."]);
        fixture_git(&checkout, &["commit", "-m", "diverge"]);
        fs::write(upstream.join("doc.md"), "# Three").unwrap();
        fixture_git(&upstream, &["add", "."]);
        fixture_git(&upstream, &["commit", "-m", "three"]);
        assert!(matches!(
            sync_with_protocol(&checkout, &remote, true),
            Err(Error::Git(_))
        ));
        assert_eq!(
            fs::read_to_string(checkout.join("doc.md")).unwrap(),
            "# Two"
        );
        let clean = temp.path().join("clean");
        run_with_protocol(
            temp.path(),
            &["clone", "--quiet", &remote, clean.to_str().unwrap()],
            true,
        )
        .unwrap();
        fs::create_dir_all(clean.join(".git/info")).unwrap();
        fs::write(clean.join(".git/info/exclude"), "collision.md\n").unwrap();
        fs::write(clean.join("collision.md"), "ignored local content").unwrap();
        fs::write(upstream.join("collision.md"), "remote content").unwrap();
        fixture_git(&upstream, &["add", "."]);
        fixture_git(&upstream, &["commit", "-m", "collision"]);
        assert!(matches!(
            sync_with_protocol(&clean, &remote, true),
            Err(Error::Git(_))
        ));
        assert_eq!(
            fs::read_to_string(clean.join("collision.md")).unwrap(),
            "ignored local content"
        );
    }
}
