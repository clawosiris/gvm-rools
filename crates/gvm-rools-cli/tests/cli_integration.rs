// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Output;

use anyhow::{Context, Result};
use gvm_mock_server::{MockGmpServer, ServerMode};
use gvm_protocol::Response;
use tempfile::{NamedTempFile, TempDir};

async fn start_mock_server() -> Result<Option<(MockGmpServer, TempDir, PathBuf)>> {
    start_mock_server_with_credentials("admin", "admin").await
}

async fn start_mock_server_with_credentials(
    username: &str,
    password: &str,
) -> Result<Option<(MockGmpServer, TempDir, PathBuf)>> {
    let socket_dir = tempfile::Builder::new().prefix("gvm-cli-").tempdir()?;
    let socket_path = socket_dir.path().join("mock.sock");
    let server = match MockGmpServer::builder()
        .mode(ServerMode::Stateful)
        .credentials(username, password)
        .unix_socket(&socket_path)
        .build()
        .await
    {
        Ok(server) => server,
        Err(error) if error.kind() == ErrorKind::PermissionDenied => return Ok(None),
        Err(error) => return Err(error).context("mock server should start"),
    };
    let path = server.socket_path().context("socket path")?.to_path_buf();
    Ok(Some((server, socket_dir, path)))
}

/// Run gvm-cli in a blocking thread so tokio can service the mock server concurrently.
async fn gvm_cli(socket_path: &Path, global_args: &[&str]) -> Result<Output> {
    let bin = assert_cmd::cargo::cargo_bin("gvm-cli");
    let mut args: Vec<String> = global_args.iter().map(|s| s.to_string()).collect();
    args.extend([
        "socket".to_string(),
        "--path".to_string(),
        socket_path.to_string_lossy().to_string(),
        "--timeout".to_string(),
        "10".to_string(),
    ]);
    tokio::task::spawn_blocking(move || {
        std::process::Command::new(bin)
            .args(&args)
            .output()
            .map_err(anyhow::Error::from)
    })
    .await?
}

/// Run gvm-cli with stdin piped, in a blocking thread.
async fn gvm_cli_stdin(
    socket_path: &Path,
    global_args: &[&str],
    stdin_data: &str,
) -> Result<Output> {
    use std::io::Write;
    let bin = assert_cmd::cargo::cargo_bin("gvm-cli");
    let mut args: Vec<String> = global_args.iter().map(|s| s.to_string()).collect();
    args.extend([
        "socket".to_string(),
        "--path".to_string(),
        socket_path.to_string_lossy().to_string(),
        "--timeout".to_string(),
        "10".to_string(),
    ]);
    let input = stdin_data.to_string();
    tokio::task::spawn_blocking(move || {
        let mut child = std::process::Command::new(bin)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        if let Some(ref mut w) = child.stdin {
            w.write_all(input.as_bytes())?;
        }
        drop(child.stdin.take());
        child.wait_with_output().map_err(anyhow::Error::from)
    })
    .await?
}

/// Run gvm-cli with a file argument, in a blocking thread.
async fn gvm_cli_file(
    socket_path: &Path,
    global_args: &[&str],
    file_path: &Path,
) -> Result<Output> {
    let bin = assert_cmd::cargo::cargo_bin("gvm-cli");
    let mut args: Vec<String> = global_args.iter().map(|s| s.to_string()).collect();
    args.push(file_path.to_string_lossy().to_string());
    args.extend([
        "socket".to_string(),
        "--path".to_string(),
        socket_path.to_string_lossy().to_string(),
        "--timeout".to_string(),
        "10".to_string(),
    ]);
    tokio::task::spawn_blocking(move || {
        std::process::Command::new(bin)
            .args(&args)
            .output()
            .map_err(anyhow::Error::from)
    })
    .await?
}

#[tokio::test]
async fn test_get_version() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(&socket_path, &["-X", "<get_version/>"]).await?;
    server.shutdown().await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_version_response"));
    assert!(stdout.contains("<version>"));
    Ok(())
}

#[tokio::test]
async fn test_get_version_from_file() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let file = NamedTempFile::new()?;
    std::fs::write(file.path(), "<get_version/>")?;

    let output = gvm_cli_file(&socket_path, &[], file.path()).await?;
    server.shutdown().await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_version_response"));
    Ok(())
}

#[tokio::test]
async fn test_get_version_from_stdin() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli_stdin(&socket_path, &[], "<get_version/>").await?;
    server.shutdown().await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_version_response"));
    Ok(())
}

#[tokio::test]
async fn test_authenticated_command() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(
        &socket_path,
        &[
            "--gmp-username",
            "admin",
            "--gmp-password",
            "admin",
            "-X",
            "<get_tasks/>",
        ],
    )
    .await?;
    server.shutdown().await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_tasks_response"));
    Ok(())
}

#[tokio::test]
async fn test_auth_failure() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(
        &socket_path,
        &[
            "--gmp-username",
            "admin",
            "--gmp-password",
            "wrong",
            "-X",
            "<get_tasks/>",
        ],
    )
    .await?;
    server.shutdown().await;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("authentication failed"), "stderr: {stderr}");
    Ok(())
}

#[tokio::test]
async fn test_authenticated_command_escapes_xml_credentials() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(
        &socket_path,
        &[
            "--gmp-username",
            "admin",
            "--gmp-password",
            "foo&bar<baz>",
            "-X",
            "<get_tasks/>",
        ],
    )
    .await?;
    let history = server.command_history();
    server.shutdown().await;

    assert_eq!(output.status.code(), Some(1));
    let auth = history
        .iter()
        .find(|record| record.command_name() == "authenticate")
        .context("missing authenticate command in mock server history")?;
    let raw_auth = String::from_utf8(auth.raw_xml().to_vec())?;
    assert!(
        raw_auth.contains("<password>foo&amp;bar&lt;baz&gt;</password>"),
        "raw authenticate XML: {raw_auth}"
    );
    Ok(())
}

#[tokio::test]
async fn test_missing_password_error() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(
        &socket_path,
        &["--gmp-username", "admin", "-X", "<get_version/>"],
    )
    .await?;
    server.shutdown().await;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.to_lowercase().contains("password"),
        "stderr: {stderr}"
    );
    Ok(())
}

#[tokio::test]
async fn test_non_2xx_raw_mode() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(&socket_path, &["--raw", "-X", "<get_tasks/>"]).await?;
    server.shutdown().await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    let response = Response::new(stdout.as_bytes().to_vec());
    assert!(stdout.contains("<get_tasks_response"), "stdout: {stdout}");
    assert!(!response.is_success(), "stdout: {stdout}");
    Ok(())
}

#[tokio::test]
async fn test_duration_flag() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(&socket_path, &["--duration", "-X", "<get_version/>"]).await?;
    server.shutdown().await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("Elapsed time:"), "stderr: {stderr}");
    Ok(())
}

#[tokio::test]
async fn test_empty_xml_error() -> Result<()> {
    let Some((_server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(&socket_path, &["-X", ""]).await?;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("no XML provided"), "stderr: {stderr}");
    Ok(())
}

#[tokio::test]
async fn test_pretty_output() -> Result<()> {
    let Some((server, _dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let output = gvm_cli(&socket_path, &["--pretty", "-X", "<get_version/>"]).await?;
    server.shutdown().await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains('\n'), "expected newlines in pretty output");
    Ok(())
}
