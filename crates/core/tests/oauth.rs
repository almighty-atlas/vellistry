use mdwell_core::handle_git_credential_request;
#[cfg(unix)]
use mdwell_core::{Core, Error, ErrorCode};
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const TOKEN: &str = "fixture-oauth-secret-never-persist";

fn main() {
    // This is the exact host executable contract, before any normal startup.
    match handle_git_credential_request() {
        Ok(true) => return,
        Ok(false) => {}
        Err(_) => std::process::exit(1),
    }
    #[cfg(unix)]
    {
        if std::env::args().nth(1).as_deref() == Some("--http-fixture") {
            http_fixture();
            return;
        }
        if std::env::current_exe().unwrap().file_name().unwrap() == "git" {
            std::process::exit(fake_git());
        }
        if std::env::args().nth(1).as_deref() == Some("--fixture-child") {
            core_fixture();
            return;
        }
    }
    helper_protocol();
    #[cfg(unix)]
    {
        real_git_helper_protocol();
        offline_core_fixture();
    }
    println!("OAuth offline fixtures passed");
}

// Launched by scripts/test-git-http.py with real Git and an isolated trusted
// proxy/CA configuration. The fake credential never comes from the environment.
#[cfg(unix)]
fn http_fixture() {
    let root = std::path::PathBuf::from(std::env::args_os().nth(2).expect("fixture root"));
    let mode = std::env::args().nth(3).expect("fixture mode");
    let data = root.join(format!("data-{mode}"));
    let core = Core::open(&data).unwrap();
    let result = core.add_github_with_token("https://github.com/owner/repo", Some(TOKEN));
    if mode != "success" {
        let expected = match mode.as_str() {
            "permission" => ErrorCode::GitPermissionDenied,
            "authentication" => ErrorCode::GitAuthentication,
            _ => panic!("unknown HTTP fixture mode"),
        };
        assert_eq!(result.unwrap_err().info().code, expected);
        assert_no_token(&data);
        return;
    }
    let workspace = result.unwrap();
    assert!(core
        .read_document(workspace.id, "README.md")
        .unwrap()
        .html
        .contains("quokka"));
    assert_eq!(core.search("quokka", Some(workspace.id)).unwrap().len(), 1);
    drop(core);

    // Advance the served repository so the reopened core must fetch and merge
    // new content, not merely repeat a no-op advertisement request.
    let source = root.join("source");
    std::fs::write(
        source.join("README.md"),
        "# HTTPS fixture\n\nUpdated platypus.\n",
    )
    .unwrap();
    real_git(&source, &["add", "README.md"]);
    real_git(&source, &["commit", "--quiet", "-m", "advance fixture"]);
    real_git(
        &source,
        &[
            "push",
            "--quiet",
            "--",
            root.join("repositories/owner/repo.git").to_str().unwrap(),
            "main",
        ],
    );

    let core = Core::open(&data).unwrap();
    assert_eq!(core.list_workspaces().unwrap().len(), 1);
    core.sync_workspace_with_token(workspace.id, Some(TOKEN))
        .unwrap();
    assert!(core
        .read_document(workspace.id, "README.md")
        .unwrap()
        .html
        .contains("platypus"));
    assert_eq!(
        core.search("platypus", Some(workspace.id)).unwrap().len(),
        1
    );
    assert!(core
        .search("quokka", Some(workspace.id))
        .unwrap()
        .is_empty());
    assert_no_token(&data);
    println!("Real Git HTTPS clone/reopen/fetch/search passed");
}

fn helper(
    request: &[u8],
    operation: &str,
    endpoint: &str,
    capability: &str,
) -> std::process::Output {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--mdwell-git-credential", operation])
        .env("MDWELL_GIT_CREDENTIAL_ENDPOINT", endpoint)
        .env("MDWELL_GIT_CREDENTIAL_CAPABILITY", capability)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(request).unwrap();
    child.wait_with_output().unwrap()
}

fn helper_protocol() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let capability = "ab".repeat(32);
    for request in [
        b"protocol=http\nhost=github.com\n\n".as_slice(),
        b"protocol=https\nhost=evil.invalid\n\n",
        b"protocol=https\nhost=github.com:443\n\n",
        b"protocol=https\nhost=github.com\nhost=evil.invalid\n\n",
        b"protocol=https\nhost=github.com\nusername=other\n\n",
        b"capability[]=authtype\nprotocol=https\nhost=evil.invalid\n\n",
        b"capability[]=state\nprotocol=https\nhost=github.com:443\n\n",
    ] {
        let output = helper(request, "get", &endpoint, &capability);
        assert!(output.status.success());
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
    }
    for operation in ["store", "erase"] {
        let output = helper(b"", operation, "invalid", "invalid");
        assert!(output.status.success());
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
    }
    let output = helper(b"", "unexpected", &endpoint, &capability);
    assert_helper_ipc_failure(&output);
    let output = helper(
        b"protocol=https\nhost=github.com\n\n",
        "get",
        "192.0.2.1:1",
        &capability,
    );
    assert_helper_ipc_failure(&output);
    let output = helper(&vec![b'x'; 16 * 1024 + 1], "get", &endpoint, &capability);
    assert_helper_ipc_failure(&output);
    let unavailable = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable_endpoint = unavailable.local_addr().unwrap().to_string();
    drop(unavailable);
    let output = helper(
        b"protocol=https\nhost=github.com\n",
        "get",
        &unavailable_endpoint,
        &capability,
    );
    assert_helper_ipc_failure(&output);
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    listener.set_nonblocking(false).unwrap();
    let expected = capability.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\n\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        assert_eq!(
            request,
            format!("{expected}\ncapability[]=authtype\ncapability[]=state\nprotocol=https\nhost=github.com\npath=owner/repo.git\n\n")
                .as_bytes()
        );
        stream
            .write_all(format!("username=x-access-token\npassword={TOKEN}\n\n").as_bytes())
            .unwrap();
    });
    let output = helper(
        b"capability[]=authtype\ncapability[]=state\nprotocol=https\nhost=github.com\npath=owner/repo.git\n",
        "get",
        &endpoint,
        &capability,
    );
    server.join().unwrap();
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        format!("username=x-access-token\npassword={TOKEN}\n\n").as_bytes()
    );
    assert!(output.stderr.is_empty());
}

fn assert_helper_ipc_failure(output: &std::process::Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"mdwell: credential helper protocol or IPC failed\n"
    );
}

// Verify real Git's EOF-terminated helper input and HTTP challenge on the IPC wire.
#[cfg(unix)]
fn real_git_helper_protocol() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let capability = "ab".repeat(32);
    let expected = capability.clone();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "helper did not connect"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\n\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() <= 16 * 1024);
        }
        assert_eq!(request, format!("{expected}\nprotocol=https\nhost=github.com\nusername=x-access-token\nwwwauth[]=Basic realm=GitHub\n\n").as_bytes());
        stream
            .write_all(format!("username=x-access-token\npassword={TOKEN}\n\n").as_bytes())
            .unwrap();
    });
    let executable = std::env::current_exe().unwrap();
    let helper = format!(
        "credential.helper=!exec '{}' --mdwell-git-credential",
        executable.to_str().unwrap().replace('\'', "'\\''")
    );
    let mut child = Command::new("git")
        .args([
            "-c",
            "credential.helper=",
            "-c",
            &helper,
            "-c",
            "credential.username=x-access-token",
            "credential",
            "fill",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("MDWELL_GIT_CREDENTIAL_ENDPOINT", endpoint)
        .env("MDWELL_GIT_CREDENTIAL_CAPABILITY", capability)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=github.com\nwwwauth[]=Basic realm=GitHub\n\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    server.join().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("password={TOKEN}\n")));
}

#[cfg(unix)]
fn real_git(root: &Path, args: &[&str]) {
    assert!(
        Command::new(std::env::var_os("MDWELL_FIXTURE_REAL_GIT").unwrap())
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid"
            ])
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[cfg(unix)]
fn offline_core_fixture() {
    let temp = tempfile::tempdir().unwrap();
    // Exercise POSIX-shell quoting, including apostrophes in the host executable.
    let host_dir = temp.path().join("host's directory");
    let bin = temp.path().join("bin");
    std::fs::create_dir(&host_dir).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let executable = std::env::current_exe().unwrap();
    let host = host_dir.join("oauth-host");
    std::fs::copy(&executable, &host).unwrap();
    std::fs::copy(&executable, bin.join("git")).unwrap();
    let git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(git.status.success());
    let git = String::from_utf8(git.stdout).unwrap();
    let config = temp.path().join("global.config");
    let marker = temp.path().join("manual-helper-ran");
    let helper = format!(
        "!printf invoked >> '{}'; printf 'username=manual\\npassword=manual\\n'",
        marker.display()
    );
    assert!(Command::new(git.trim())
        .args([
            "config",
            "--file",
            config.to_str().unwrap(),
            "credential.https://github.com.helper",
            &helper
        ])
        .output()
        .unwrap()
        .status
        .success());
    let empty_system = temp.path().join("system.config");
    std::fs::write(&empty_system, b"").unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = Command::new(host)
        .arg("--fixture-child")
        .env("PATH", path)
        .env("GIT_CONFIG_SYSTEM", empty_system)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("MDWELL_FIXTURE_REAL_GIT", git.trim())
        .env("MDWELL_FIXTURE_ROOT", temp.path())
        .env("MDWELL_FIXTURE_MARKER", marker)
        // Inherited diagnostic settings must be removed by the core runner.
        .env("GIT_TRACE", temp.path().join("trace"))
        .env("GIT_TRACE_CURL", temp.path().join("curl-trace"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        std::fs::read_to_string(temp.path().join("fixture-diagnostic")).unwrap_or_default()
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
    assert!(!temp.path().join("trace").exists());
    assert!(!temp.path().join("curl-trace").exists());
}

#[cfg(unix)]
fn core_fixture() {
    let root = std::path::PathBuf::from(std::env::var_os("MDWELL_FIXTURE_ROOT").unwrap());
    let source = root.join("source");
    std::fs::create_dir(&source).unwrap();
    // Fixture preparation is outside the runner and must not create trace files.
    std::env::remove_var("GIT_TRACE");
    std::env::remove_var("GIT_TRACE_CURL");
    real_git(&source, &["init", "--initial-branch=main"]);
    std::fs::write(source.join("README.md"), "# Fixture\n").unwrap();
    real_git(&source, &["add", "."]);
    real_git(&source, &["commit", "-m", "fixture"]);
    std::env::set_var("GIT_TRACE", root.join("trace"));
    std::env::set_var("GIT_TRACE_CURL", root.join("curl-trace"));
    let core = Core::open(root.join("data")).unwrap();
    let workspace = core
        .add_github_with_token("https://github.com/owner/repo", Some(TOKEN))
        .unwrap();
    assert_eq!(
        workspace.url.as_deref(),
        Some("https://github.com/owner/repo.git")
    );
    core.sync_workspace_with_token(workspace.id, Some(TOKEN))
        .unwrap();
    let marker = std::path::PathBuf::from(std::env::var_os("MDWELL_FIXTURE_MARKER").unwrap());
    assert!(!marker.exists());
    std::env::set_var("MDWELL_FIXTURE_REJECT", "1");
    assert!(matches!(
        core.add_github_with_token("https://github.com/owner/rejected", Some(TOKEN)),
        Err(Error::GitAuthentication)
    ));
    assert!(matches!(
        core.sync_workspace_with_token(workspace.id, Some(TOKEN)),
        Err(Error::GitAuthentication)
    ));
    assert!(!marker.exists());
    std::env::remove_var("MDWELL_FIXTURE_REJECT");
    for (mode, code) in [
        ("not-called", ErrorCode::GitCredentialIpc),
        ("unsupported", ErrorCode::GitCredentialIpc),
        ("permission", ErrorCode::GitPermissionDenied),
        ("network", ErrorCode::GitNetwork),
        ("helper-start", ErrorCode::GitCredentialHelperStart),
    ] {
        std::env::set_var("MDWELL_FIXTURE_UNDELIVERED", mode);
        let error = core
            .add_github_with_token("https://github.com/owner/undelivered", Some(TOKEN))
            .unwrap_err();
        assert_eq!(error.info().code, code, "clone: {mode}");
        let error = core
            .sync_workspace_with_token(workspace.id, Some(TOKEN))
            .unwrap_err();
        assert_eq!(error.info().code, code, "fetch: {mode}");
        assert!(!marker.exists());
    }
    std::env::remove_var("MDWELL_FIXTURE_UNDELIVERED");
    for token in ["", "bad\npassword=injected", "bad\rtoken", "bad\0token"] {
        let error = core
            .add_github_with_token("https://github.com/owner/invalid", Some(token))
            .unwrap_err();
        assert!(!error.to_string().contains(token) || token.is_empty());
    }
    std::env::set_var("MDWELL_FIXTURE_MANUAL", "1");
    let manual = core.add_github("https://github.com/owner/manual").unwrap();
    core.sync_workspace(manual.id).unwrap();
    assert!(marker.exists());
    let before = std::fs::read(&marker).unwrap();
    let ssh = core
        .add_github_with_token("git@github.com:owner/ssh", Some("ignored\ninvalid"))
        .unwrap();
    core.sync_workspace_with_token(ssh.id, Some(TOKEN)).unwrap();
    assert_eq!(std::fs::read(&marker).unwrap(), before);
    let local = core.add_local(&source).unwrap();
    core.sync_workspace_with_token(local.id, Some("ignored\ninvalid"))
        .unwrap();

    assert_no_token(&root.join("data"));
}

#[cfg(unix)]
fn assert_no_token(path: &Path) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_token(&path);
        } else {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(TOKEN.len())
                    .any(|chunk| chunk == TOKEN.as_bytes()),
                "credential persisted in {}",
                path.display()
            );
        }
    }
}

// An offline Git shim preserves all core -c flags and runs real credential fill,
// approve/reject and local clone/fetch. No HTTP requests or token files are used.
#[cfg(unix)]
fn fake_git() -> i32 {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert!(!args.iter().any(|arg| arg.contains(TOKEN)));
    assert!(!std::env::vars().any(|(_, value)| value.contains(TOKEN)));
    let mut index = 0;
    while args.get(index).is_some_and(|arg| arg == "-c") {
        index += 2;
    }
    let operation = args.get(index).map(String::as_str).unwrap_or("");
    let real = std::env::var_os("MDWELL_FIXTURE_REAL_GIT").unwrap();
    if !matches!(operation, "clone" | "fetch") {
        return Command::new(real)
            .args(&args)
            .status()
            .unwrap()
            .code()
            .unwrap_or(1);
    }
    let credential = |operation: &str, input: &[u8]| {
        let mut child = Command::new(&real)
            .args(&args[..index])
            .args(["credential", operation])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };
    if let Ok(mode) = std::env::var("MDWELL_FIXTURE_UNDELIVERED") {
        if mode == "unsupported" {
            let output = credential(
                "fill",
                b"protocol=https\nhost=github.com\nusername=unsupported\n\n",
            );
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
        eprintln!(
            "{}",
            match mode.as_str() {
                "permission" => "fatal: requested URL returned error: 403",
                "network" => "fatal: SSL certificate problem: untrusted issuer",
                "helper-start" => "mdwell: credential helper execution failed",
                _ => "fatal: requested URL returned error: 401; Authentication failed",
            }
        );
        return 1;
    }
    let url = &args[args.len() - 2];
    if url.starts_with("git@github.com:") {
        assert!(std::env::var_os("MDWELL_GIT_CREDENTIAL_ENDPOINT").is_none());
        assert!(std::env::var_os("MDWELL_GIT_CREDENTIAL_CAPABILITY").is_none());
        assert!(!args
            .iter()
            .any(|arg| arg.contains("--mdwell-git-credential")));
        assert_eq!(
            std::env::var("GIT_SSH_COMMAND").unwrap(),
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes"
        );
    } else {
        let output = credential(
            "fill",
            b"protocol=https\nhost=github.com\npath=owner/repo.git\nwwwauth[]=Basic realm=GitHub\n\n",
        );
        if !output.status.success() {
            let root = std::path::PathBuf::from(std::env::var_os("MDWELL_FIXTURE_ROOT").unwrap());
            std::fs::write(
                root.join("fixture-diagnostic"),
                String::from_utf8_lossy(&output.stderr).replace(TOKEN, "<redacted>"),
            )
            .unwrap();
            return 1;
        }
        let expected = if std::env::var_os("MDWELL_FIXTURE_MANUAL").is_some() {
            "manual"
        } else {
            TOKEN
        };
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("password={expected}\n")));
        if std::env::var_os("MDWELL_FIXTURE_REJECT").is_some() {
            assert!(credential("reject", &output.stdout).status.success());
            eprintln!("fatal: requested URL returned error: 401; Authentication failed");
            return 1;
        }
        assert!(credential("approve", &output.stdout).status.success());
    }
    let source =
        std::path::PathBuf::from(std::env::var_os("MDWELL_FIXTURE_ROOT").unwrap()).join("source");
    let mut command = Command::new(&real);
    command
        .args(&args[..index])
        .args(["-c", "protocol.file.allow=always"]);
    if operation == "clone" {
        command.args([
            "clone",
            "--quiet",
            "--",
            source.to_str().unwrap(),
            args.last().unwrap(),
        ]);
    } else {
        command.args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--",
            source.to_str().unwrap(),
            args.last().unwrap(),
        ]);
    }
    if !command.status().unwrap().success() {
        return 1;
    }
    if operation == "clone" {
        let url = &args[args.len() - 2];
        assert!(Command::new(real)
            .args(&args[..index])
            .arg("-C")
            .arg(args.last().unwrap())
            .args(["config", "remote.origin.url", url])
            .status()
            .unwrap()
            .success());
    }
    0
}
