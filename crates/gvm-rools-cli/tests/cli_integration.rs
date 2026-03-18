// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result};
use assert_cmd::Command;
use gvm_mock_server::{MockGmpServer, ServerMode};
use tempfile::{NamedTempFile, TempDir};

async fn start_mock_server() -> Result<Option<(MockGmpServer, TempDir, PathBuf)>> {
    let socket_dir = tempfile::Builder::new()
        .prefix("gvm-cli-test-")
        .tempdir_in(".")?;
    let socket_path = socket_dir.path().join("mock.sock");
    let server = match MockGmpServer::builder()
        .mode(ServerMode::Stateful)
        .credentials("admin", "admin")
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

fn base_command(socket_path: &PathBuf) -> Result<Command> {
    let mut command = Command::cargo_bin("gvm-cli")?;
    command.arg("socket").arg("--path").arg(socket_path);
    Ok(command)
}

#[tokio::test]
async fn test_get_version() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args(["-X", "<get_version/>"])
        .output()?;

    server.shutdown().await;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_version_response"));
    assert!(stdout.contains("<version>"));
    Ok(())
}

#[tokio::test]
async fn test_get_version_from_file() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };
    let file = NamedTempFile::new()?;
    std::fs::write(file.path(), "<get_version/>")?;

    let output = base_command(&socket_path)?.arg(file.path()).output()?;

    server.shutdown().await;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_version_response"));
    assert!(stdout.contains("<version>"));
    Ok(())
}

#[tokio::test]
async fn test_get_version_from_stdin() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .write_stdin("<get_version/>")
        .output()?;

    server.shutdown().await;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_version_response"));
    assert!(stdout.contains("<version>"));
    Ok(())
}

#[tokio::test]
async fn test_authenticated_command() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args([
            "--gmp-username",
            "admin",
            "--gmp-password",
            "admin",
            "-X",
            "<get_tasks/>",
        ])
        .output()?;

    server.shutdown().await;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_tasks_response"));
    Ok(())
}

#[tokio::test]
async fn test_auth_failure() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args([
            "--gmp-username",
            "admin",
            "--gmp-password",
            "wrong",
            "-X",
            "<get_tasks/>",
        ])
        .output()?;

    server.shutdown().await;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("authentication failed"));
    Ok(())
}

#[tokio::test]
async fn test_missing_password_error() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args(["--gmp-username", "admin", "-X", "<get_version/>"])
        .output()?;

    server.shutdown().await;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("password is required"));
    Ok(())
}

#[tokio::test]
async fn test_non_2xx_default_mode() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args(["-X", "<get_tasks/>"])
        .output()?;

    server.shutdown().await;

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_tasks_response"));
    Ok(())
}

#[tokio::test]
async fn test_non_2xx_raw_mode() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args(["--raw", "-X", "<get_tasks/>"])
        .output()?;

    server.shutdown().await;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("<get_tasks_response"));
    Ok(())
}

#[tokio::test]
async fn test_duration_flag() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args(["--duration", "-X", "<get_version/>"])
        .output()?;

    server.shutdown().await;

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("Elapsed time:"));
    Ok(())
}

#[tokio::test]
async fn test_empty_xml_error() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?.args(["-X", ""]).output()?;

    server.shutdown().await;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("no XML provided"));
    Ok(())
}

#[tokio::test]
async fn test_custom_timeout() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args(["--timeout", "5", "-X", "<get_version/>"])
        .output()?;

    server.shutdown().await;

    assert!(output.status.success());
    Ok(())
}

#[tokio::test]
async fn test_pretty_output() -> Result<()> {
    let Some((server, _socket_dir, socket_path)) = start_mock_server().await? else {
        return Ok(());
    };

    let output = base_command(&socket_path)?
        .args(["--pretty", "-X", "<get_version/>"])
        .output()?;

    server.shutdown().await;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("\n  <version>"));
    Ok(())
}
