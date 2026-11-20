use std::fs;
use std::process::Stdio;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_VAR: &str = "TELEGRAM_WEBHOOK_API_READ_TOKEN";
const TOKEN: &str = "test-token-with-unique-value";

fn kwp(dir: &TempDir, yaml: &str) -> Command {
    fs::write(dir.path().join("config.yml"), yaml).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_kwp"));
    command
        .current_dir(dir.path())
        .env_clear()
        .env("DATABASE_URL", "sqlite::memory:")
        .env("BIND_ADDRESS", "127.0.0.1:0")
        .env("IGNORED_HEADERS", "host")
        .env("UI_ACCESS_TOKEN", "test-ui-token")
        .kill_on_drop(true);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

async fn startup_error(command: &mut Command) -> String {
    let output = timeout(STARTUP_TIMEOUT, command.output())
        .await
        .expect("kwp did not exit after a configuration error")
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "kwp exited successfully: {stdout}{stderr}"
    );
    format!("{stdout}{stderr}")
}

#[tokio::test]
async fn starts_with_references_from_the_environment_and_dotenv() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".env"), "TELEGRAM_METRICS=false\n").unwrap();
    let mut child = kwp(
        &dir,
        "\
channels:
  - name: telegram
    api-read-token: TELEGRAM_WEBHOOK_API_READ_TOKEN
    monitoring-metrics: '${TELEGRAM_METRICS}'
",
    )
    .env(TOKEN_VAR, TOKEN)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();

    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let listening = timeout(STARTUP_TIMEOUT, async {
        while let Some(line) = lines.next_line().await.unwrap() {
            assert!(!line.contains(TOKEN), "token leaked in startup logs");
            if line.contains("Listening on '127.0.0.1:0'") {
                return true;
            }
        }
        false
    })
    .await
    .expect("kwp did not start listening");
    if listening {
        child.start_kill().unwrap();
    }
    let output = timeout(STARTUP_TIMEOUT, child.wait_with_output())
        .await
        .expect("kwp did not exit")
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(TOKEN), "token leaked in startup output");
    if !listening {
        panic!("kwp exited during startup: {stderr}");
    }
}

#[tokio::test]
async fn missing_references_stop_startup_with_the_yaml_path() {
    let dir = TempDir::new().unwrap();
    let yaml = "\
channels:
  - name: telegram
    api-read-token: TELEGRAM_WEBHOOK_API_READ_TOKEN
  - name: internal
    api-read-token: literal-token
    allowed-ips: ['${AllowedIp}']
";

    for (provided, value, expected) in [
        (
            "AllowedIp",
            "192.0.2.1",
            "environment variable TELEGRAM_WEBHOOK_API_READ_TOKEN for channels[0].api-read-token is not set",
        ),
        (
            TOKEN_VAR,
            TOKEN,
            "environment variable AllowedIp for channels[1].allowed-ips[0] is not set",
        ),
    ] {
        let error = startup_error(kwp(&dir, yaml).env(provided, value)).await;
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains(TOKEN), "token leaked in startup output");
    }
}

#[tokio::test]
async fn required_fields_are_still_checked_after_substitution() {
    let dir = TempDir::new().unwrap();
    let yaml = "channels: [{name: '${ChannelName}'}]";
    let error = startup_error(kwp(&dir, yaml).env("ChannelName", "telegram")).await;

    assert!(
        error.contains("missing configuration field") && error.contains("api-read-token"),
        "{error}"
    );
}

#[tokio::test]
async fn invalid_resolved_values_report_the_field_without_the_value() {
    let dir = TempDir::new().unwrap();

    for field in ["monitoring-metrics", "max-body-size", "secret-type"] {
        let yaml = format!(
            "channels:\n  - name: telegram\n    api-read-token: literal-token\n    {field}: '${{InvalidValue}}'\n"
        );
        let error = startup_error(kwp(&dir, &yaml).env("InvalidValue", TOKEN)).await;
        assert!(error.contains("failed to deserialize config"), "{error}");
        assert!(
            error.contains("channels[0]") && error.contains(field),
            "{error}"
        );
        assert!(
            !error.contains(TOKEN),
            "resolved value leaked in startup output"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_environment_value_stops_startup() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = TempDir::new().unwrap();
    let yaml = "channels: [{name: telegram, api-read-token: TELEGRAM_WEBHOOK_API_READ_TOKEN}]";
    let error = startup_error(kwp(&dir, yaml).env(TOKEN_VAR, OsString::from_vec(vec![0xff]))).await;

    assert!(error.contains(
        "environment variable TELEGRAM_WEBHOOK_API_READ_TOKEN for channels[0].api-read-token is not valid UTF-8"
    ), "{error}");
}
