#![allow(non_snake_case)]

mod terminal;
mod tool_versions;

use crate::init_status::{InitErrorPayload, SkillsMigrationPayload};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

pub(crate) use terminal::launch_terminal_running;
pub use tool_versions::{ToolVersion, WslShellPreferenceInput};

/// 鎵撳紑澶栭儴閾炬帴
#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> Result<bool, String> {
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else {
        format!("https://{url}")
    };

    app.opener()
        .open_url(&url, None::<String>)
        .map_err(|e| format!("鎵撳紑閾炬帴澶辫触: {e}"))?;

    Ok(true)
}

#[tauri::command]
pub async fn copy_text_to_clipboard(text: String) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("璁块棶绯荤粺鍓创鏉垮け璐? {e}"))?;
        clipboard
            .set_text(text)
            .map_err(|e| format!("鍐欏叆绯荤粺鍓创鏉垮け璐? {e}"))?;
        Ok(true)
    })
    .await
    .map_err(|e| format!("鍓创鏉夸换鍔℃墽琛屽け璐? {e}"))?
}

/// 妫€鏌ユ洿鏂?
#[tauri::command]
pub async fn check_for_updates(handle: AppHandle) -> Result<bool, String> {
    handle
        .opener()
        .open_url(
            "https://github.com/farion1231/cc-switch/releases/latest",
            None::<String>,
        )
        .map_err(|e| format!("鎵撳紑鏇存柊椤甸潰澶辫触: {e}"))?;

    Ok(true)
}

/// 鍒ゆ柇鏄惁涓轰究鎼虹増锛堢豢鑹茬増锛夎繍琛?
#[tauri::command]
pub async fn is_portable_mode() -> Result<bool, String> {
    let exe_path = std::env::current_exe().map_err(|e| format!("鑾峰彇鍙墽琛岃矾寰勫け璐? {e}"))?;
    if let Some(dir) = exe_path.parent() {
        Ok(dir.join("portable.ini").is_file())
    } else {
        Ok(false)
    }
}

/// 鑾峰彇搴旂敤鍚姩闃舵鐨勫垵濮嬪寲閿欒锛堣嫢鏈夛級銆?
#[tauri::command]
pub async fn get_init_error() -> Result<Option<InitErrorPayload>, String> {
    Ok(crate::init_status::get_init_error())
}

/// 鑾峰彇 JSON鈫扴QLite 杩佺Щ缁撴灉锛堣嫢鏈夛級銆?
#[tauri::command]
pub async fn get_migration_result() -> Result<bool, String> {
    Ok(crate::init_status::take_migration_success())
}

/// 鑾峰彇 Skills 鑷姩瀵煎叆锛圫SOT锛夎縼绉荤粨鏋滐紙鑻ユ湁锛夈€?
#[tauri::command]
pub async fn get_skills_migration_result() -> Result<Option<SkillsMigrationPayload>, String> {
    Ok(crate::init_status::take_skills_migration_result())
}

#[tauri::command]
pub async fn get_tool_versions(
    tools: Option<Vec<String>>,
    wsl_shell_by_tool: Option<std::collections::HashMap<String, WslShellPreferenceInput>>,
) -> Result<Vec<ToolVersion>, String> {
    tool_versions::get_tool_versions(tools, wsl_shell_by_tool).await
}

#[allow(non_snake_case)]
#[tauri::command]
pub async fn open_provider_terminal(
    state: tauri::State<'_, crate::store::AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    cwd: Option<String>,
) -> Result<bool, String> {
    terminal::open_provider_terminal(state, app, providerId, cwd).await
}

/// 璁剧疆绐楀彛涓婚锛圵indows/macOS 鏍囬鏍忛鑹诧級
/// theme: "dark" | "light" | "system"
#[tauri::command]
pub async fn set_window_theme(window: tauri::Window, theme: String) -> Result<(), String> {
    use tauri::Theme;

    let tauri_theme = match theme.as_str() {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    };

    window.set_theme(tauri_theme).map_err(|e| e.to_string())
}
