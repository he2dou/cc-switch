#[cfg(not(target_os = "windows"))]
use once_cell::sync::Lazy;
#[cfg(not(target_os = "windows"))]
use regex::Regex;
use std::collections::HashMap;
#[cfg(not(target_os = "windows"))]
use std::path::Path;
#[cfg(any(test, not(target_os = "windows")))]
use std::path::PathBuf;

#[derive(serde::Serialize)]
pub struct ToolVersion {
    name: String,
    version: Option<String>,
    latest_version: Option<String>,
    error: Option<String>,
    env_type: String,
    wsl_distro: Option<String>,
}

#[cfg(not(target_os = "windows"))]
const VALID_TOOLS: [&str; 4] = ["claude", "codex", "gemini", "opencode"];

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslShellPreferenceInput {
    #[serde(default)]
    pub wsl_shell: Option<String>,
    #[serde(default)]
    pub wsl_shell_flag: Option<String>,
}

#[cfg(all(not(target_os = "windows"), target_os = "macos"))]
fn tool_env_type_and_wsl_distro(_tool: &str) -> (String, Option<String>) {
    ("macos".to_string(), None)
}

#[cfg(all(not(target_os = "windows"), target_os = "linux"))]
fn tool_env_type_and_wsl_distro(_tool: &str) -> (String, Option<String>) {
    ("linux".to_string(), None)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn tool_env_type_and_wsl_distro(_tool: &str) -> (String, Option<String>) {
    ("unknown".to_string(), None)
}

pub async fn get_tool_versions(
    tools: Option<Vec<String>>,
    wsl_shell_by_tool: Option<HashMap<String, WslShellPreferenceInput>>,
) -> Result<Vec<ToolVersion>, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = (tools, wsl_shell_by_tool);
        Ok(Vec::new())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let requested: Vec<&str> = if let Some(tools) = tools.as_ref() {
            let set: std::collections::HashSet<&str> = tools.iter().map(|s| s.as_str()).collect();
            VALID_TOOLS
                .iter()
                .copied()
                .filter(|t| set.contains(t))
                .collect()
        } else {
            VALID_TOOLS.to_vec()
        };
        let mut results = Vec::new();

        for tool in requested {
            let pref = wsl_shell_by_tool.as_ref().and_then(|m| m.get(tool));
            let tool_wsl_shell = pref.and_then(|p| p.wsl_shell.as_deref());
            let tool_wsl_shell_flag = pref.and_then(|p| p.wsl_shell_flag.as_deref());

            results.push(
                get_single_tool_version_impl(tool, tool_wsl_shell, tool_wsl_shell_flag).await,
            );
        }

        Ok(results)
    }
}

#[cfg(not(target_os = "windows"))]
async fn get_single_tool_version_impl(
    tool: &str,
    wsl_shell: Option<&str>,
    wsl_shell_flag: Option<&str>,
) -> ToolVersion {
    debug_assert!(
        VALID_TOOLS.contains(&tool),
        "unexpected tool name in get_single_tool_version_impl: {tool}"
    );

    let (env_type, wsl_distro) = tool_env_type_and_wsl_distro(tool);
    let client = crate::proxy::http_client::get();

    let (local_version, local_error) = if let Some(distro) = wsl_distro.as_deref() {
        try_get_version_wsl(tool, distro, wsl_shell, wsl_shell_flag)
    } else {
        let direct_result = try_get_version(tool);
        if direct_result.0.is_some() {
            direct_result
        } else {
            scan_cli_version(tool)
        }
    };

    let latest_version = match tool {
        "claude" => fetch_npm_latest_version(&client, "@anthropic-ai/claude-code").await,
        "codex" => fetch_npm_latest_version(&client, "@openai/codex").await,
        "gemini" => fetch_npm_latest_version(&client, "@google/gemini-cli").await,
        "opencode" => fetch_github_latest_version(&client, "anomalyco/opencode").await,
        _ => None,
    };

    ToolVersion {
        name: tool.to_string(),
        version: local_version,
        latest_version,
        error: local_error,
        env_type,
        wsl_distro,
    }
}

#[cfg(not(target_os = "windows"))]
async fn fetch_npm_latest_version(client: &reqwest::Client, package: &str) -> Option<String> {
    let url = format!("https://registry.npmjs.org/{package}");
    match client.get(&url).send().await {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json.get("dist-tags")
                    .and_then(|tags| tags.get("latest"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

#[cfg(not(target_os = "windows"))]
async fn fetch_github_latest_version(client: &reqwest::Client, repo: &str) -> Option<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    match client
        .get(&url)
        .header("User-Agent", "cc-switch")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json.get("tag_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.strip_prefix('v').unwrap_or(s).to_string())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

#[cfg(not(target_os = "windows"))]
static VERSION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\d+\.\d+\.\d+(-[\w.]+)?").expect("Invalid version regex"));

#[cfg(not(target_os = "windows"))]
fn extract_version(raw: &str) -> String {
    VERSION_RE
        .find(raw)
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| raw.to_string())
}

#[cfg(not(target_os = "windows"))]
fn try_get_version(tool: &str) -> (Option<String>, Option<String>) {
    use std::process::Command;

    #[cfg(target_os = "windows")]
    let output = {
        Command::new("cmd")
            .args(["/C", &format!("{tool} --version")])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    };

    #[cfg(not(target_os = "windows"))]
    let output = {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| is_valid_shell(s))
            .unwrap_or_else(|| "sh".to_string());
        let flag = default_flag_for_shell(&shell);
        Command::new(shell)
            .arg(flag)
            .arg(format!("{tool} --version"))
            .output()
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if out.status.success() {
                let raw = if stdout.is_empty() { &stderr } else { &stdout };
                if raw.is_empty() {
                    (None, Some("not installed or not executable".to_string()))
                } else {
                    (Some(extract_version(raw)), None)
                }
            } else {
                let err = if stderr.is_empty() { stdout } else { stderr };
                (
                    None,
                    Some(if err.is_empty() {
                        "not installed or not executable".to_string()
                    } else {
                        err
                    }),
                )
            }
        }
        Err(e) => (None, Some(e.to_string())),
    }
}

#[cfg(not(target_os = "windows"))]
fn is_valid_shell(shell: &str) -> bool {
    matches!(
        shell.rsplit('/').next().unwrap_or(shell),
        "sh" | "bash" | "zsh" | "fish" | "dash"
    )
}

#[cfg(not(target_os = "windows"))]
fn default_flag_for_shell(shell: &str) -> &'static str {
    match shell.rsplit('/').next().unwrap_or(shell) {
        "dash" | "sh" => "-c",
        "fish" => "-lc",
        _ => "-lic",
    }
}

#[cfg(not(target_os = "windows"))]
fn try_get_version_wsl(
    _tool: &str,
    _distro: &str,
    _force_shell: Option<&str>,
    _force_shell_flag: Option<&str>,
) -> (Option<String>, Option<String>) {
    (
        None,
        Some("WSL check not supported on this platform".to_string()),
    )
}

#[cfg(not(target_os = "windows"))]
fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }

    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[cfg(not(target_os = "windows"))]
fn push_env_single_dir(paths: &mut Vec<PathBuf>, value: Option<std::ffi::OsString>) {
    if let Some(raw) = value {
        push_unique_path(paths, PathBuf::from(raw));
    }
}

#[cfg(not(target_os = "windows"))]
fn extend_from_path_list(
    paths: &mut Vec<PathBuf>,
    value: Option<std::ffi::OsString>,
    suffix: Option<&str>,
) {
    if let Some(raw) = value {
        for p in std::env::split_paths(&raw) {
            let dir = match suffix {
                Some(s) => p.join(s),
                None => p,
            };
            push_unique_path(paths, dir);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn opencode_extra_search_paths(
    home: &Path,
    opencode_install_dir: Option<std::ffi::OsString>,
    xdg_bin_dir: Option<std::ffi::OsString>,
    gopath: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    push_env_single_dir(&mut paths, opencode_install_dir);
    push_env_single_dir(&mut paths, xdg_bin_dir);

    if !home.as_os_str().is_empty() {
        push_unique_path(&mut paths, home.join("bin"));
        push_unique_path(&mut paths, home.join(".opencode").join("bin"));
        push_unique_path(&mut paths, home.join(".bun").join("bin"));
        push_unique_path(&mut paths, home.join("go").join("bin"));
    }

    extend_from_path_list(&mut paths, gopath, Some("bin"));

    paths
}

#[cfg(not(target_os = "windows"))]
fn tool_executable_candidates(tool: &str, dir: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        vec![
            dir.join(format!("{tool}.cmd")),
            dir.join(format!("{tool}.exe")),
            dir.join(tool),
        ]
    }

    #[cfg(not(target_os = "windows"))]
    {
        vec![dir.join(tool)]
    }
}

#[cfg(not(target_os = "windows"))]
fn scan_cli_version(tool: &str) -> (Option<String>, Option<String>) {
    use std::process::Command;

    let home = dirs::home_dir().unwrap_or_default();
    let mut search_paths: Vec<PathBuf> = Vec::new();
    if !home.as_os_str().is_empty() {
        push_unique_path(&mut search_paths, home.join(".local/bin"));
        push_unique_path(&mut search_paths, home.join(".npm-global/bin"));
        push_unique_path(&mut search_paths, home.join("n/bin"));
        push_unique_path(&mut search_paths, home.join(".volta/bin"));
    }

    #[cfg(target_os = "macos")]
    {
        push_unique_path(&mut search_paths, PathBuf::from("/opt/homebrew/bin"));
        push_unique_path(&mut search_paths, PathBuf::from("/usr/local/bin"));
    }

    #[cfg(target_os = "linux")]
    {
        push_unique_path(&mut search_paths, PathBuf::from("/usr/local/bin"));
        push_unique_path(&mut search_paths, PathBuf::from("/usr/bin"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = dirs::data_dir() {
            push_unique_path(&mut search_paths, appdata.join("npm"));
        }
        push_unique_path(
            &mut search_paths,
            PathBuf::from("C:\\Program Files\\nodejs"),
        );
    }

    let fnm_base = home.join(".local/state/fnm_multishells");
    if fnm_base.exists() {
        if let Ok(entries) = std::fs::read_dir(&fnm_base) {
            for entry in entries.flatten() {
                let bin_path = entry.path().join("bin");
                if bin_path.exists() {
                    push_unique_path(&mut search_paths, bin_path);
                }
            }
        }
    }

    let nvm_base = home.join(".nvm/versions/node");
    if nvm_base.exists() {
        if let Ok(entries) = std::fs::read_dir(&nvm_base) {
            for entry in entries.flatten() {
                let bin_path = entry.path().join("bin");
                if bin_path.exists() {
                    push_unique_path(&mut search_paths, bin_path);
                }
            }
        }
    }

    if tool == "opencode" {
        let extra_paths = opencode_extra_search_paths(
            &home,
            std::env::var_os("OPENCODE_INSTALL_DIR"),
            std::env::var_os("XDG_BIN_DIR"),
            std::env::var_os("GOPATH"),
        );

        for path in extra_paths {
            push_unique_path(&mut search_paths, path);
        }
    }

    let current_path = std::env::var("PATH").unwrap_or_default();

    for path in &search_paths {
        #[cfg(target_os = "windows")]
        let new_path = format!("{};{}", path.display(), current_path);

        #[cfg(not(target_os = "windows"))]
        let new_path = format!("{}:{}", path.display(), current_path);

        for tool_path in tool_executable_candidates(tool, path) {
            if !tool_path.exists() {
                continue;
            }

            #[cfg(target_os = "windows")]
            let output = {
                Command::new("cmd")
                    .args(["/C", &format!("\"{}\" --version", tool_path.display())])
                    .env("PATH", &new_path)
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
            };

            #[cfg(not(target_os = "windows"))]
            let output = {
                Command::new(&tool_path)
                    .arg("--version")
                    .env("PATH", &new_path)
                    .output()
            };

            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if out.status.success() {
                    let raw = if stdout.is_empty() { &stderr } else { &stdout };
                    if !raw.is_empty() {
                        return (Some(extract_version(raw)), None);
                    }
                }
            }
        }
    }

    (None, Some("not installed or not executable".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_extract_version() {
        assert_eq!(extract_version("claude 1.0.20"), "1.0.20");
        assert_eq!(extract_version("v2.3.4-beta.1"), "2.3.4-beta.1");
        assert_eq!(extract_version("no version here"), "no version here");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_extra_search_paths_includes_install_and_fallback_dirs() {
        let home = PathBuf::from("/home/tester");
        let install_dir = Some(std::ffi::OsString::from("/custom/opencode/bin"));
        let xdg_bin_dir = Some(std::ffi::OsString::from("/xdg/bin"));
        let gopath =
            std::env::join_paths([PathBuf::from("/go/path1"), PathBuf::from("/go/path2")]).ok();

        let paths = opencode_extra_search_paths(&home, install_dir, xdg_bin_dir, gopath);

        assert_eq!(paths[0], PathBuf::from("/custom/opencode/bin"));
        assert_eq!(paths[1], PathBuf::from("/xdg/bin"));
        assert!(paths.contains(&PathBuf::from("/home/tester/bin")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.opencode/bin")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.bun/bin")));
        assert!(paths.contains(&PathBuf::from("/home/tester/go/bin")));
        assert!(paths.contains(&PathBuf::from("/go/path1/bin")));
        assert!(paths.contains(&PathBuf::from("/go/path2/bin")));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_extra_search_paths_deduplicates_repeated_entries() {
        let home = PathBuf::from("/home/tester");
        let same_dir = Some(std::ffi::OsString::from("/same/path"));

        let paths = opencode_extra_search_paths(&home, same_dir.clone(), same_dir, None);

        let count = paths
            .iter()
            .filter(|path| **path == PathBuf::from("/same/path"))
            .count();
        assert_eq!(count, 1);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_extra_search_paths_deduplicates_bun_default_dir() {
        let home = PathBuf::from("/home/tester");
        let paths = opencode_extra_search_paths(&home, None, None, None);

        let count = paths
            .iter()
            .filter(|path| **path == PathBuf::from("/home/tester/.bun/bin"))
            .count();
        assert_eq!(count, 1);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn tool_executable_candidates_non_windows_uses_plain_binary_name() {
        let dir = PathBuf::from("/usr/local/bin");
        let candidates = tool_executable_candidates("opencode", &dir);

        assert_eq!(candidates, vec![PathBuf::from("/usr/local/bin/opencode")]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn tool_executable_candidates_windows_includes_cmd_exe_and_plain_name() {
        let dir = PathBuf::from("C:\\tools");
        let candidates = tool_executable_candidates("opencode", &dir);

        assert_eq!(
            candidates,
            vec![
                PathBuf::from("C:\\tools\\opencode.cmd"),
                PathBuf::from("C:\\tools\\opencode.exe"),
                PathBuf::from("C:\\tools\\opencode"),
            ]
        );
    }
}
