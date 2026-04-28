use crate::app_config::AppType;
use crate::services::ProviderService;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tauri::State;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub async fn open_provider_terminal(
    state: State<'_, crate::store::AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    cwd: Option<String>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    let launch_cwd = resolve_launch_cwd(cwd)?;

    let providers = ProviderService::list(state.inner(), app_type.clone())
        .map_err(|e| format!("failed to list providers: {e}"))?;

    let provider = providers
        .get(&providerId)
        .ok_or_else(|| format!("provider not found: {providerId}"))?;

    let config = &provider.settings_config;
    let env_vars = extract_env_vars_from_config(config, &app_type);

    launch_terminal_with_env(env_vars, &providerId, launch_cwd.as_deref())
        .map_err(|e| format!("failed to launch terminal: {e}"))?;

    Ok(true)
}

fn extract_env_vars_from_config(
    config: &serde_json::Value,
    app_type: &AppType,
) -> Vec<(String, String)> {
    let mut env_vars = Vec::new();

    let Some(obj) = config.as_object() else {
        return env_vars;
    };

    if let Some(env) = obj.get("env").and_then(|v| v.as_object()) {
        for (key, value) in env {
            if let Some(str_val) = value.as_str() {
                env_vars.push((key.clone(), str_val.to_string()));
            }
        }

        let base_url_key = match app_type {
            AppType::Claude => Some("ANTHROPIC_BASE_URL"),
            AppType::Gemini => Some("GOOGLE_GEMINI_BASE_URL"),
            _ => None,
        };

        if let Some(key) = base_url_key {
            if let Some(url_str) = env.get(key).and_then(|v| v.as_str()) {
                env_vars.push((key.to_string(), url_str.to_string()));
            }
        }
    }

    if *app_type == AppType::Codex {
        if let Some(auth) = obj.get("auth").and_then(|v| v.as_str()) {
            env_vars.push(("OPENAI_API_KEY".to_string(), auth.to_string()));
        }
    }

    if *app_type == AppType::Gemini {
        if let Some(api_key) = obj.get("api_key").and_then(|v| v.as_str()) {
            env_vars.push(("GEMINI_API_KEY".to_string(), api_key.to_string()));
        }
    }

    env_vars
}

fn resolve_launch_cwd(cwd: Option<String>) -> Result<Option<PathBuf>, String> {
    let Some(raw_path) = cwd.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };

    if raw_path.contains('\n') || raw_path.contains('\r') {
        return Err("cwd contains invalid newline characters".to_string());
    }

    let path = Path::new(&raw_path);
    if !path.exists() {
        return Err(format!("directory does not exist: {raw_path}"));
    }

    let resolved =
        std::fs::canonicalize(path).map_err(|e| format!("failed to resolve directory: {e}"))?;
    if !resolved.is_dir() {
        return Err(format!("path is not a directory: {}", resolved.display()));
    }

    #[cfg(target_os = "windows")]
    let resolved = {
        let s = resolved.to_string_lossy();
        if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{unc}"))
        } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            resolved
        }
    };

    Ok(Some(resolved))
}

fn launch_terminal_with_env(
    env_vars: Vec<(String, String)>,
    provider_id: &str,
    cwd: Option<&Path>,
) -> Result<(), String> {
    let temp_dir = std::env::temp_dir();
    let config_file = temp_dir.join(format!(
        "claude_{}_{}.json",
        provider_id,
        std::process::id()
    ));

    write_claude_config(&config_file, &env_vars)?;

    #[cfg(target_os = "macos")]
    {
        launch_macos_terminal(&config_file, cwd)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        launch_linux_terminal(&config_file, cwd)?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        launch_windows_terminal(&temp_dir, &config_file, cwd)?;
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err("unsupported operating system".to_string())
}

fn write_claude_config(
    config_file: &std::path::Path,
    env_vars: &[(String, String)],
) -> Result<(), String> {
    let mut config_obj = serde_json::Map::new();
    let mut env_obj = serde_json::Map::new();

    for (key, value) in env_vars {
        env_obj.insert(key.clone(), serde_json::Value::String(value.clone()));
    }

    config_obj.insert("env".to_string(), serde_json::Value::Object(env_obj));

    let config_json = serde_json::to_string_pretty(&config_obj)
        .map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(config_file, config_json)
        .map_err(|e| format!("failed to write config file: {e}"))
}

#[cfg(target_os = "macos")]
fn launch_macos_terminal(config_file: &std::path::Path, cwd: Option<&Path>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let preferred = crate::settings::get_preferred_terminal();
    let terminal = preferred.as_deref().unwrap_or("terminal");

    let temp_dir = std::env::temp_dir();
    let script_file = temp_dir.join(format!("cc_switch_launcher_{}.sh", std::process::id()));
    let config_path = config_file.to_string_lossy();
    let cd_command = build_shell_cd_command(cwd);

    let script_content = format!(
        r#"#!/bin/bash
trap 'rm -f "{config_path}" "{script_file}"' EXIT
{cd_command}
echo "Using provider-specific claude config:"
echo "{config_path}"
claude --settings "{config_path}"
exec bash --norc --noprofile
"#,
        config_path = config_path,
        script_file = script_file.display(),
        cd_command = cd_command,
    );

    std::fs::write(&script_file, &script_content)
        .map_err(|e| format!("failed to write launcher script: {e}"))?;
    std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("failed to set launcher permissions: {e}"))?;

    let result = match terminal {
        "iterm2" => launch_macos_iterm2(&script_file),
        "alacritty" => launch_macos_open_app("Alacritty", &script_file, true),
        "kitty" => launch_macos_open_app("kitty", &script_file, false),
        "ghostty" => launch_macos_open_app("Ghostty", &script_file, true),
        "wezterm" => launch_macos_open_app("WezTerm", &script_file, true),
        "kaku" => launch_macos_open_app("Kaku", &script_file, true),
        _ => launch_macos_terminal_app(&script_file),
    };

    if result.is_err() && terminal != "terminal" {
        log::warn!(
            "preferred terminal {} failed, falling back to Terminal.app: {:?}",
            terminal,
            result.as_ref().err()
        );
        return launch_macos_terminal_app(&script_file);
    }

    result
}

#[cfg(target_os = "macos")]
fn launch_macos_terminal_app(script_file: &std::path::Path) -> Result<(), String> {
    use std::process::Command;

    let applescript = format!(
        r#"tell application "Terminal"
    activate
    do script "bash '{}'"
end tell"#,
        script_file.display()
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&applescript)
        .output()
        .map_err(|e| format!("failed to execute osascript: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Terminal.app failed (exit code: {:?}): {}",
            output.status.code(),
            stderr
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_macos_iterm2(script_file: &std::path::Path) -> Result<(), String> {
    use std::process::Command;

    let applescript = format!(
        r#"tell application "iTerm"
    activate
    tell current window
        create tab with default profile
        tell current session
            write text "bash '{}'"
        end tell
    end tell
end tell"#,
        script_file.display()
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&applescript)
        .output()
        .map_err(|e| format!("failed to execute osascript: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "iTerm2 failed (exit code: {:?}): {}",
            output.status.code(),
            stderr
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_macos_open_app(
    app_name: &str,
    script_file: &std::path::Path,
    use_e_flag: bool,
) -> Result<(), String> {
    use std::process::Command;

    let mut cmd = Command::new("open");
    cmd.arg("-a").arg(app_name).arg("--args");

    if use_e_flag {
        cmd.arg("-e");
    }
    cmd.arg("bash").arg(script_file);

    let output = cmd
        .output()
        .map_err(|e| format!("failed to launch {app_name}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} failed (exit code: {:?}): {}",
            app_name,
            output.status.code(),
            stderr
        ));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn launch_linux_terminal(config_file: &std::path::Path, cwd: Option<&Path>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let preferred = crate::settings::get_preferred_terminal();
    let default_terminals = [
        ("gnome-terminal", vec!["--"]),
        ("konsole", vec!["-e"]),
        ("xfce4-terminal", vec!["-e"]),
        ("mate-terminal", vec!["--"]),
        ("lxterminal", vec!["-e"]),
        ("alacritty", vec!["-e"]),
        ("kitty", vec!["-e"]),
        ("ghostty", vec!["-e"]),
    ];

    let temp_dir = std::env::temp_dir();
    let script_file = temp_dir.join(format!("cc_switch_launcher_{}.sh", std::process::id()));
    let config_path = config_file.to_string_lossy();
    let cd_command = build_shell_cd_command(cwd);

    let script_content = format!(
        r#"#!/bin/bash
trap 'rm -f "{config_path}" "{script_file}"' EXIT
{cd_command}
echo "Using provider-specific claude config:"
echo "{config_path}"
claude --settings "{config_path}"
exec bash --norc --noprofile
"#,
        config_path = config_path,
        script_file = script_file.display(),
        cd_command = cd_command,
    );

    std::fs::write(&script_file, &script_content)
        .map_err(|e| format!("failed to write launcher script: {e}"))?;
    std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("failed to set launcher permissions: {e}"))?;

    let terminals_to_try: Vec<(&str, Vec<&str>)> = if let Some(ref pref) = preferred {
        let pref_args = default_terminals
            .iter()
            .find(|(name, _)| *name == pref.as_str())
            .map(|(_, args)| args.to_vec())
            .unwrap_or_else(|| vec!["-e"]);

        let mut list = vec![(pref.as_str(), pref_args)];
        for (name, args) in &default_terminals {
            if *name != pref.as_str() {
                list.push((*name, args.to_vec()));
            }
        }
        list
    } else {
        default_terminals
            .iter()
            .map(|(name, args)| (*name, args.to_vec()))
            .collect()
    };

    let mut last_error = String::from("no supported terminal found");

    for (terminal, args) in terminals_to_try {
        let terminal_exists = std::path::Path::new(&format!("/usr/bin/{}", terminal)).exists()
            || std::path::Path::new(&format!("/bin/{}", terminal)).exists()
            || std::path::Path::new(&format!("/usr/local/bin/{}", terminal)).exists()
            || which_command(terminal);

        if terminal_exists {
            let result = Command::new(terminal)
                .args(&args)
                .arg("bash")
                .arg(script_file.to_string_lossy().as_ref())
                .spawn();

            match result {
                Ok(_) => return Ok(()),
                Err(e) => last_error = format!("failed to execute {}: {}", terminal, e),
            }
        }
    }

    let _ = std::fs::remove_file(&script_file);
    let _ = std::fs::remove_file(config_file);
    Err(last_error)
}

#[cfg(target_os = "linux")]
fn which_command(cmd: &str) -> bool {
    use std::process::Command;
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn launch_windows_terminal(
    temp_dir: &std::path::Path,
    config_file: &std::path::Path,
    cwd: Option<&Path>,
) -> Result<(), String> {
    let preferred = crate::settings::get_preferred_terminal();
    let terminal = preferred.as_deref().unwrap_or("cmd");

    let bat_file = temp_dir.join(format!("cc_switch_claude_{}.bat", std::process::id()));
    let config_path_for_batch = escape_windows_batch_value(&config_file.to_string_lossy());
    let cwd_command = build_windows_cwd_command(cwd);

    let content = format!(
        "@echo off
{cwd_command}
echo Using provider-specific claude config:
echo {}
claude --settings \"{}\"
del \"{}\" >nul 2>&1
del \"%~f0\" >nul 2>&1
",
        config_path_for_batch,
        config_path_for_batch,
        config_path_for_batch,
        cwd_command = cwd_command,
    );

    std::fs::write(&bat_file, &content)
        .map_err(|e| format!("failed to write batch file: {e}"))?;

    let bat_path = bat_file.to_string_lossy();
    let ps_cmd = format!("& '{}'", bat_path);

    let result = match terminal {
        "powershell" => run_windows_start_command(
            &["powershell", "-NoExit", "-Command", &ps_cmd],
            "PowerShell",
        ),
        "wt" => run_windows_start_command(&["wt", "cmd", "/K", &bat_path], "Windows Terminal"),
        _ => run_windows_start_command(&["cmd", "/K", &bat_path], "cmd"),
    };

    if result.is_err() && terminal != "cmd" {
        log::warn!(
            "preferred terminal {} failed, falling back to cmd: {:?}",
            terminal,
            result.as_ref().err()
        );
        return run_windows_start_command(&["cmd", "/K", &bat_path], "cmd");
    }

    result
}

#[cfg(not(target_os = "windows"))]
fn build_shell_cd_command(cwd: Option<&Path>) -> String {
    cwd.map(|dir| {
        format!(
            "cd {} || exit 1\n",
            shell_single_quote(&dir.to_string_lossy())
        )
    })
    .unwrap_or_default()
}

#[cfg(not(target_os = "windows"))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn is_windows_unc_path(path: &str) -> bool {
    path.starts_with(r"\\")
}

fn build_windows_cwd_command_str(path: &str) -> String {
    let escaped = escape_windows_batch_value(path);

    if is_windows_unc_path(path) {
        format!("pushd \"{escaped}\" || exit /b 1\r\n")
    } else {
        format!("cd /d \"{escaped}\" || exit /b 1\r\n")
    }
}

#[cfg(target_os = "windows")]
fn build_windows_cwd_command(cwd: Option<&Path>) -> String {
    cwd.map(|dir| build_windows_cwd_command_str(&dir.to_string_lossy()))
        .unwrap_or_default()
}

fn escape_windows_batch_value(value: &str) -> String {
    value
        .replace('^', "^^")
        .replace('%', "%%")
        .replace('&', "^&")
        .replace('|', "^|")
        .replace('<', "^<")
        .replace('>', "^>")
        .replace('(', "^(")
        .replace(')', "^)")
}

#[cfg(target_os = "windows")]
fn run_windows_start_command(args: &[&str], terminal_name: &str) -> Result<(), String> {
    use std::process::Command;

    let mut full_args = vec!["/C", "start"];
    full_args.extend(args);

    let output = Command::new("cmd")
        .args(&full_args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("failed to start {}: {e}", terminal_name))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} failed (exit code: {:?}): {}",
            terminal_name,
            output.status.code(),
            stderr
        ));
    }

    Ok(())
}

pub(crate) fn launch_terminal_running(command_line: &str, label: &str) -> Result<(), String> {
    let temp_dir = std::env::temp_dir();
    let pid = std::process::id();

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let (script_file, script_content) = {
        let file = temp_dir.join(format!("cc_switch_{}_{}.sh", label, pid));
        let content = format!(
            r#"#!/bin/bash
trap 'rm -f "{script_path}"' EXIT
echo "[cc-switch] Starting: {cmd}"
echo ""
{cmd}
echo ""
echo "[cc-switch] Command exited. Press any key to close."
read -n 1 -s
"#,
            script_path = file.display(),
            cmd = command_line,
        );
        (file, content)
    };

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(&script_file, &script_content)
            .map_err(|e| format!("failed to write launcher script: {e}"))?;
        std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed to set launcher permissions: {e}"))?;

        let preferred = crate::settings::get_preferred_terminal();
        let terminal = preferred.as_deref().unwrap_or("terminal");

        let result = match terminal {
            "iterm2" => launch_macos_iterm2(&script_file),
            "alacritty" => launch_macos_open_app("Alacritty", &script_file, true),
            "kitty" => launch_macos_open_app("kitty", &script_file, false),
            "ghostty" => launch_macos_open_app("Ghostty", &script_file, true),
            "wezterm" => launch_macos_open_app("WezTerm", &script_file, true),
            "kaku" => launch_macos_open_app("Kaku", &script_file, true),
            _ => launch_macos_terminal_app(&script_file),
        };

        if result.is_err() && terminal != "terminal" {
            log::warn!(
                "preferred terminal {} failed, falling back to Terminal.app: {:?}",
                terminal,
                result.as_ref().err()
            );
            return launch_macos_terminal_app(&script_file);
        }
        result
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        std::fs::write(&script_file, &script_content)
            .map_err(|e| format!("failed to write launcher script: {e}"))?;
        std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed to set launcher permissions: {e}"))?;

        let preferred = crate::settings::get_preferred_terminal();
        let default_terminals = [
            ("gnome-terminal", vec!["--"]),
            ("konsole", vec!["-e"]),
            ("xfce4-terminal", vec!["-e"]),
            ("mate-terminal", vec!["--"]),
            ("lxterminal", vec!["-e"]),
            ("alacritty", vec!["-e"]),
            ("kitty", vec!["-e"]),
            ("ghostty", vec!["-e"]),
        ];

        let terminals_to_try: Vec<(&str, Vec<&str>)> = if let Some(ref pref) = preferred {
            let pref_args = default_terminals
                .iter()
                .find(|(name, _)| *name == pref.as_str())
                .map(|(_, args)| args.to_vec())
                .unwrap_or_else(|| vec!["-e"]);
            let mut list = vec![(pref.as_str(), pref_args)];
            for (name, args) in &default_terminals {
                if *name != pref.as_str() {
                    list.push((*name, args.to_vec()));
                }
            }
            list
        } else {
            default_terminals
                .iter()
                .map(|(name, args)| (*name, args.to_vec()))
                .collect()
        };

        let mut last_error = String::from("no supported terminal found");

        for (terminal, args) in terminals_to_try {
            let terminal_exists = which_command(terminal)
                || ["/usr/bin", "/bin", "/usr/local/bin"]
                    .iter()
                    .any(|dir| std::path::Path::new(&format!("{}/{}", dir, terminal)).exists());

            if terminal_exists {
                let spawn_result = Command::new(terminal)
                    .args(&args)
                    .arg("bash")
                    .arg(script_file.to_string_lossy().as_ref())
                    .spawn();
                match spawn_result {
                    Ok(_) => return Ok(()),
                    Err(e) => last_error = format!("failed to execute {}: {}", terminal, e),
                }
            }
        }

        let _ = std::fs::remove_file(&script_file);
        Err(last_error)
    }

    #[cfg(target_os = "windows")]
    {
        let preferred = crate::settings::get_preferred_terminal();
        let terminal = preferred.as_deref().unwrap_or("cmd");

        let bat_file = temp_dir.join(format!("cc_switch_{}_{}.bat", label, pid));
        let content = format!(
            "@echo off\r\necho [cc-switch] Starting: {cmd}\r\necho.\r\n{cmd}\r\necho.\r\necho [cc-switch] Command exited. Press any key to close.\r\npause >nul\r\ndel \"%~f0\" >nul 2>&1\r\n",
            cmd = command_line,
        );
        std::fs::write(&bat_file, &content)
            .map_err(|e| format!("failed to write batch file: {e}"))?;

        let bat_path = bat_file.to_string_lossy();
        let ps_cmd = format!("& '{}'", bat_path);

        let result = match terminal {
            "powershell" => run_windows_start_command(
                &["powershell", "-NoExit", "-Command", &ps_cmd],
                "PowerShell",
            ),
            "wt" => run_windows_start_command(&["wt", "cmd", "/K", &bat_path], "Windows Terminal"),
            _ => run_windows_start_command(&["cmd", "/K", &bat_path], "cmd"),
        };

        let final_result = if result.is_err() && terminal != "cmd" {
            log::warn!(
                "preferred terminal {} failed, falling back to cmd: {:?}",
                terminal,
                result.as_ref().err()
            );
            run_windows_start_command(&["cmd", "/K", &bat_path], "cmd")
        } else {
            result
        };

        if final_result.is_err() {
            let _ = std::fs::remove_file(&bat_file);
        }
        final_result
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (temp_dir, pid, command_line, label);
        Err("unsupported operating system".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_launch_cwd_accepts_existing_directory() {
        let resolved =
            resolve_launch_cwd(Some(std::env::temp_dir().to_string_lossy().into_owned()))
                .expect("temp dir should resolve")
                .expect("temp dir should be present");

        assert!(resolved.is_dir());
    }

    #[test]
    fn resolve_launch_cwd_rejects_missing_directory() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let missing = std::env::temp_dir().join(format!("cc-switch-missing-{unique}"));

        let error = resolve_launch_cwd(Some(missing.to_string_lossy().into_owned()))
            .expect_err("missing directory should fail");

        assert!(error.contains("directory does not exist"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_shell_cd_command_quotes_spaces_and_single_quotes() {
        let command = build_shell_cd_command(Some(Path::new("/tmp/project O'Brien")));

        assert_eq!(command, "cd '/tmp/project O'\"'\"'Brien' || exit 1\n");
    }

    #[test]
    fn build_windows_cwd_command_str_uses_cd_for_drive_paths() {
        let command = build_windows_cwd_command_str(r"C:\work\repo");

        assert_eq!(command, "cd /d \"C:\\work\\repo\" || exit /b 1\r\n");
    }

    #[test]
    fn build_windows_cwd_command_str_uses_pushd_for_unc_paths() {
        let command = build_windows_cwd_command_str(r"\\wsl$\Ubuntu\home\coder\repo");

        assert_eq!(
            command,
            "pushd \"\\\\wsl$\\Ubuntu\\home\\coder\\repo\" || exit /b 1\r\n"
        );
    }

    #[test]
    fn build_windows_cwd_command_str_escapes_batch_metacharacters() {
        let command = build_windows_cwd_command_str(r"\\server\share\100%&(test)");

        assert_eq!(
            command,
            "pushd \"\\\\server\\share\\100%%^&^(test^)\" || exit /b 1\r\n"
        );
    }
}
