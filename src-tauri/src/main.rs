#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod github;

use github::{AccountStatus, Github, Repository};
use mdwell_core::{
    Asset, Core, Document, DocumentSummary, ErrorCode, ErrorInfo, RefreshReport, ResolvedLink,
    SearchHit, Workspace,
};
use std::sync::Arc;
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;
use zeroize::Zeroizing;

type Shared = Arc<Core>;

async fn task<T: Send + 'static>(
    core: Shared,
    operation: impl FnOnce(&Core) -> mdwell_core::Result<T> + Send + 'static,
) -> Result<T, ErrorInfo> {
    tauri::async_runtime::spawn_blocking(move || operation(&core).map_err(|e| e.info()))
        .await
        .map_err(|e| ErrorInfo {
            code: ErrorCode::Io,
            message: format!("Background task failed: {e}"),
            expected_revision: None,
            actual_revision: None,
        })?
}

#[tauri::command]
async fn list_workspaces(core: State<'_, Shared>) -> Result<Vec<Workspace>, ErrorInfo> {
    task(core.inner().clone(), Core::list_workspaces).await
}
#[tauri::command]
async fn add_local(core: State<'_, Shared>, path: String) -> Result<Workspace, ErrorInfo> {
    task(core.inner().clone(), move |c| c.add_local(path)).await
}
#[tauri::command]
async fn add_github(
    core: State<'_, Shared>,
    github: State<'_, Arc<Github>>,
    url: String,
) -> Result<Workspace, ErrorInfo> {
    let account = github.inner().clone();
    task(core.inner().clone(), move |c| {
        let token = if url.starts_with("https://github.com/") {
            account
                .access_token()
                .map_err(mdwell_core::Error::Git)?
                .map(Zeroizing::new)
        } else {
            None
        };
        c.add_github_with_token(&url, token.as_deref().map(String::as_str))
    })
    .await
}
#[tauri::command]
async fn sync_workspace(
    core: State<'_, Shared>,
    workspace_id: i64,
    github: State<'_, Arc<Github>>,
) -> Result<RefreshReport, ErrorInfo> {
    let account = github.inner().clone();
    task(core.inner().clone(), move |c| {
        let https = c.list_workspaces()?.iter().any(|w| {
            w.id == workspace_id
                && w.url
                    .as_deref()
                    .is_some_and(|url| url.starts_with("https://github.com/"))
        });
        let token = if https {
            account
                .access_token()
                .map_err(mdwell_core::Error::Git)?
                .map(Zeroizing::new)
        } else {
            None
        };
        c.sync_workspace_with_token(workspace_id, token.as_deref().map(String::as_str))
    })
    .await
}
#[tauri::command]
async fn refresh_workspace(
    core: State<'_, Shared>,
    workspace_id: i64,
) -> Result<RefreshReport, ErrorInfo> {
    task(core.inner().clone(), move |c| {
        c.refresh_workspace(workspace_id)
    })
    .await
}
#[tauri::command]
async fn list_documents(
    core: State<'_, Shared>,
    workspace_id: i64,
) -> Result<Vec<DocumentSummary>, ErrorInfo> {
    task(core.inner().clone(), move |c| {
        c.list_documents(workspace_id)
    })
    .await
}
#[tauri::command]
async fn read_document(
    core: State<'_, Shared>,
    workspace_id: i64,
    path: String,
) -> Result<Document, ErrorInfo> {
    task(core.inner().clone(), move |c| {
        c.read_document(workspace_id, &path)
    })
    .await
}
#[tauri::command]
async fn save_document(
    core: State<'_, Shared>,
    workspace_id: i64,
    path: String,
    content: String,
    expected_revision: String,
) -> Result<Document, ErrorInfo> {
    task(core.inner().clone(), move |c| {
        c.save_document(workspace_id, &path, &content, &expected_revision)
    })
    .await
}
#[tauri::command]
async fn search(
    core: State<'_, Shared>,
    query: String,
    workspace_id: Option<i64>,
) -> Result<Vec<SearchHit>, ErrorInfo> {
    task(core.inner().clone(), move |c| {
        c.search(&query, workspace_id)
    })
    .await
}
#[tauri::command]
async fn resolve_link(
    core: State<'_, Shared>,
    workspace_id: i64,
    path: String,
    href: String,
) -> Result<ResolvedLink, ErrorInfo> {
    task(core.inner().clone(), move |c| {
        c.resolve_link(workspace_id, &path, &href)
    })
    .await
}
#[tauri::command]
async fn read_asset(
    core: State<'_, Shared>,
    workspace_id: i64,
    path: String,
) -> Result<Asset, ErrorInfo> {
    task(core.inner().clone(), move |c| {
        c.read_asset(workspace_id, &path)
    })
    .await
}

async fn account_task<T: Send + 'static>(
    github: Arc<Github>,
    operation: impl FnOnce(&Github) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || operation(&github))
        .await
        .map_err(|_| "GitHub background operation failed.".to_string())?
}

#[tauri::command]
async fn github_status(github: State<'_, Arc<Github>>) -> Result<AccountStatus, String> {
    account_task(github.inner().clone(), Github::status).await
}
#[tauri::command]
async fn github_connect(
    app: tauri::AppHandle,
    github: State<'_, Arc<Github>>,
) -> Result<AccountStatus, String> {
    account_task(github.inner().clone(), move |account| {
        account.start(|url| {
            app.opener()
                .open_url(url, None::<&str>)
                .map_err(|_| "Cannot open system browser.".to_string())
        })
    })
    .await
}
#[tauri::command]
async fn github_cancel(github: State<'_, Arc<Github>>) -> Result<(), String> {
    account_task(github.inner().clone(), Github::cancel).await
}
#[tauri::command]
async fn github_disconnect(github: State<'_, Arc<Github>>) -> Result<(), String> {
    account_task(github.inner().clone(), Github::disconnect).await
}
#[tauri::command]
async fn github_repositories(github: State<'_, Arc<Github>>) -> Result<Vec<Repository>, String> {
    account_task(github.inner().clone(), Github::repositories).await
}
#[tauri::command]
async fn github_manage(
    app: tauri::AppHandle,
    github: State<'_, Arc<Github>>,
) -> Result<(), String> {
    let url = github.management_url()?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "Cannot open system browser.".to_string())
}

fn main() {
    match mdwell_core::handle_git_credential_request() {
        Ok(true) => std::process::exit(0),
        Ok(false) => {}
        Err(_) => std::process::exit(1),
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let core = Arc::new(Core::open(app.path().app_data_dir()?)?);
            app.manage(core);
            app.manage(Arc::new(Github::new()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            github_status,
            github_connect,
            github_cancel,
            github_disconnect,
            github_repositories,
            github_manage,
            list_workspaces,
            add_local,
            add_github,
            sync_workspace,
            refresh_workspace,
            list_documents,
            read_document,
            save_document,
            search,
            resolve_link,
            read_asset
        ])
        .run(tauri::generate_context!())
        .expect("Failed to run Mdwell desktop");
}
