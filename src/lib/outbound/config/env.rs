use std::collections::HashSet;
use std::env;
use std::fmt;
use std::path::Path;

use config::{ConfigError, Value, ValueKind};
use serde::Deserialize;

use crate::domain::config::model::{
    AppConfig, ForwardBackoffDefaults, LoadAppConfigError, WebhookChannelConfig,
};
use crate::domain::config::ports::AppConfigLoader;

const DEFAULT_BODY_LIMIT_BYTES: usize = 262_144; // 256 KB

const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_LOG_TARGET: &str = "stdout";

/// Reads the logging settings on their own, before the rest of the configuration.
///
/// The logger has to be installed before [`AppConfigLoader::load`] runs: `load`
/// warns about unusable values, and `log` macros are no-ops until a logger exists,
/// so those warnings would otherwise be dropped. `load` reads the same two
/// variables through here, so the two paths cannot disagree.
pub fn log_settings_from_env() -> (String, String) {
    (
        env::var("LOG_LEVEL").unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string()),
        env::var("LOG_TARGET").unwrap_or_else(|_| DEFAULT_LOG_TARGET.to_string()),
    )
}

#[derive(Deserialize)]
struct WebhookChannelsConfig {
    channels: Vec<WebhookChannelConfig>,
}

#[derive(Clone)]
pub struct EnvConfigLoader;

impl AppConfigLoader for EnvConfigLoader {
    fn load(&self) -> Result<AppConfig, LoadAppConfigError> {
        let bind = env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

        let (log_level, log_target) = log_settings_from_env();

        let data_path = env::var("DATA_PATH").unwrap_or_else(|_| "./data".to_string());

        let db_cnn = env::var("DATABASE_URL").map_err(|_| {
            LoadAppConfigError::Unknown(anyhow::anyhow!("DATABASE_URL is required"))
        })?;

        let config_path = env::var("CONFIG_FILE").unwrap_or_else(|_| "config.yml".to_string());

        let app_config = load_channels_from_file(&config_path)?;

        let default_body_limit: usize = match env::var("DEFAULT_BODY_LIMIT") {
            Ok(val) => match val.parse() {
                Ok(v) => v,
                Err(_) => {
                    log::warn!(
                        "invalid DEFAULT_BODY_LIMIT value '{}', using default {}",
                        val,
                        DEFAULT_BODY_LIMIT_BYTES
                    );
                    DEFAULT_BODY_LIMIT_BYTES
                }
            },
            Err(_) => DEFAULT_BODY_LIMIT_BYTES,
        };

        let ignored_headers_str = env::var("IGNORED_HEADERS").map_err(|_| {
            LoadAppConfigError::Unknown(anyhow::anyhow!("IGNORED_HEADERS is required"))
        })?;

        let mut ignored_headers_set: HashSet<String> = HashSet::new();
        for header in ignored_headers_str.split(',') {
            let trimmed = header.trim().to_lowercase();
            if !trimmed.is_empty() {
                ignored_headers_set.insert(trimmed);
            }
        }
        let mut ignored_headers: Vec<String> = ignored_headers_set.into_iter().collect();
        ignored_headers.sort();

        let metrics_enabled =
            matches!(env::var("METRICS_ENABLED").as_deref(), Ok("1") | Ok("true"));

        let backoff_defaults = ForwardBackoffDefaults::default();
        let forward_backoff = ForwardBackoffDefaults {
            multiplier: parsed_or_default(
                "FORWARD_BACKOFF_MULTIPLIER",
                backoff_defaults.multiplier,
            ),
            max_seconds: parsed_or_default(
                "FORWARD_BACKOFF_MAX_SECONDS",
                backoff_defaults.max_seconds,
            ),
            jitter: parsed_or_default("FORWARD_BACKOFF_JITTER", backoff_defaults.jitter),
        };

        let trusted_proxies: Vec<String> = match env::var("TRUSTED_PROXIES") {
            Ok(val) => val
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            Err(_) => vec![],
        };

        let ui_enabled = !matches!(env::var("UI_ENABLED").as_deref(), Ok("0") | Ok("false"));
        let api_enabled = !matches!(env::var("API_ENABLED").as_deref(), Ok("0") | Ok("false"));

        let ui_access_token = match env::var("UI_ACCESS_TOKEN") {
            Ok(token) => token,
            Err(_) => {
                let generated = uuid::Uuid::new_v4().to_string();
                log::warn!(
                    "UI_ACCESS_TOKEN is not set — generated a temporary token for this session"
                );
                // Deliberately printed rather than logged: the token is a credential,
                // and `warn!` records become Sentry breadcrumbs that would carry it
                // off the host with the next reported error.
                println!("UI_ACCESS_TOKEN={generated}");
                generated
            }
        };

        Ok(AppConfig {
            bind,
            log_level,
            log_target,
            data_path,
            db_cnn,
            channels: app_config.channels,
            default_body_limit,
            ignored_headers,
            metrics_enabled,
            trusted_proxies,
            ui_access_token: Some(ui_access_token),
            ui_enabled,
            api_enabled,
            forward_backoff,
        })
    }
}

/// Reads a numeric variable, falling back to `default` when it is unset or
/// unparsable. Values that parse but make no sense are rejected later by
/// `AppConfig::validate_forward_backoff`, which fails startup instead of
/// silently substituting a default.
fn parsed_or_default<T: std::str::FromStr + fmt::Display>(name: &str, default: T) -> T {
    let Ok(raw) = env::var(name) else {
        return default;
    };

    match raw.parse() {
        Ok(value) => value,
        Err(_) => {
            log::warn!("invalid {name} value '{raw}', using default {default}");
            default
        }
    }
}

fn load_channels_from_file<P: AsRef<Path>>(
    path: P,
) -> Result<WebhookChannelsConfig, LoadAppConfigError> {
    let path = path.as_ref();

    let content = std::fs::read_to_string(path).map_err(|e| {
        LoadAppConfigError::Unknown(anyhow::anyhow!(
            "failed to read config file '{}': {}",
            path.display(),
            e
        ))
    })?;

    let mut config = config::Config::builder()
        .add_source(config::File::from_str(&content, config::FileFormat::Yaml))
        .build()
        .map_err(|e| {
            LoadAppConfigError::Unknown(anyhow::anyhow!("failed to parse config file: {}", e))
        })?;

    resolve_environment(&mut config.cache, "", &|name| env::var(name))?;

    config.try_deserialize().map_err(|error| {
        let message = match error {
            ConfigError::Type { expected, key, .. } => {
                format!(
                    "expected {expected} at {}",
                    key.as_deref().unwrap_or("root")
                )
            }
            ConfigError::At { key: Some(key), .. } => format!("invalid value at {key}"),
            error @ ConfigError::NotFound(_) => error.to_string(),
            _ => "invalid configuration value".to_string(),
        };
        LoadAppConfigError::Unknown(anyhow::anyhow!("failed to deserialize config: {message}"))
    })
}

fn environment_reference(value: &str) -> Option<&str> {
    let name = if let Some(name) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        name
    } else if value.contains('_') && !value.bytes().any(|byte| byte.is_ascii_lowercase()) {
        value
    } else {
        return None;
    };

    let mut bytes = name.bytes();
    (bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    .then_some(name)
}

fn resolve_environment(
    value: &mut Value,
    path: &str,
    lookup: &impl Fn(&str) -> Result<String, env::VarError>,
) -> Result<(), LoadAppConfigError> {
    match &mut value.kind {
        ValueKind::Table(fields) => {
            for (key, value) in fields {
                let field_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                resolve_environment(value, &field_path, lookup)?;
            }
        }
        ValueKind::Array(items) => {
            for (index, value) in items.iter_mut().enumerate() {
                resolve_environment(value, &format!("{path}[{index}]"), lookup)?;
            }
        }
        ValueKind::String(string) => {
            if let Some(literal) = string
                .strip_prefix("$$")
                .filter(|s| environment_reference(s).is_some())
            {
                *string = literal.to_string();
            } else if let Some(name) = environment_reference(string) {
                *string = lookup(name).map_err(|error| {
                    let reason = match error {
                        env::VarError::NotPresent => "is not set",
                        env::VarError::NotUnicode(_) => "is not valid UTF-8",
                    };
                    LoadAppConfigError::Unknown(anyhow::anyhow!(
                        "environment variable {name} for {path} {reason}"
                    ))
                })?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    fn test_config_path() -> String {
        "test-data/config.yml".to_string()
    }

    #[test]
    fn test_environment_reference_requires_a_complete_identifier() {
        for (value, name) in [
            ("TELEGRAM_TOKEN", "TELEGRAM_TOKEN"),
            ("_TOKEN_2", "_TOKEN_2"),
            ("${TOKEN}", "TOKEN"),
            ("${telegramToken2}", "telegramToken2"),
        ] {
            assert_eq!(environment_reference(value), Some(name), "{value:?}");
        }

        for value in [
            "",
            "TOKEN",
            "telegram_token",
            "2_TOKEN",
            "TOKEN_NAME ",
            "${}",
            "${2TOKEN}",
            "${TOKEN-NAME}",
            "${TÖKEN}",
            "${TOKEN",
            "Bearer ${TOKEN}",
        ] {
            assert_eq!(environment_reference(value), None, "{value:?}");
        }
    }

    #[test]
    fn test_resolve_environment_in_nested_values() {
        let yaml = r#"
channels:
  - name: telegram
    api-read-token: TELEGRAM_WEBHOOK_API_READ_TOKEN
    webhook-secret: '${secret}'
    note: '$$LITERAL_NOTE'
    secret-header: '$$${literal_header}'
    secret-type: '${SECRET_TYPE}'
    secret-sign-template: '${empty}'
    max-body-size: '${BODY_LIMIT}'
    monitoring-metrics: '${METRICS}'
    allowed-ips: ['${allowedIp}']
    forward:
      url: https://example.com/hook
      interval-seconds: '${INTERVAL}'
      sign-header: null
      backoff:
        multiplier: '${MULTIPLIER}'
        max-seconds: 900
"#;
        let mut config = config::Config::builder()
            .add_source(config::File::from_str(yaml, config::FileFormat::Yaml))
            .build()
            .unwrap();
        let vars = std::collections::HashMap::from([
            ("TELEGRAM_WEBHOOK_API_READ_TOKEN", "000123"),
            ("secret", "line: one # \"quoted\"\n${UNRESOLVED}\n"),
            ("SECRET_TYPE", "hmac-sha256"),
            ("empty", ""),
            ("BODY_LIMIT", "1024"),
            ("METRICS", "false"),
            ("allowedIp", "192.0.2.0/24"),
            ("INTERVAL", "30"),
            ("MULTIPLIER", "1.5"),
        ]);
        resolve_environment(&mut config.cache, "", &|name| {
            vars.get(name)
                .map(|value| value.to_string())
                .ok_or(env::VarError::NotPresent)
        })
        .unwrap();

        let config: WebhookChannelsConfig = config.try_deserialize().unwrap();
        let channel = &config.channels[0];
        assert_eq!(channel.name, "telegram");
        assert_eq!(channel.api_read_token, "000123");
        assert_eq!(channel.webhook_secret.as_deref(), Some(vars["secret"]));
        assert_eq!(channel.note.as_deref(), Some("LITERAL_NOTE"));
        assert_eq!(channel.secret_header.as_deref(), Some("${literal_header}"));
        assert_eq!(
            channel.secret_type,
            crate::domain::config::model::SecretType::HmacSha256
        );
        assert_eq!(channel.secret_sign_template.as_deref(), Some(""));
        assert_eq!(channel.max_body_size, Some(1024));
        assert!(!channel.monitoring_metrics);
        assert_eq!(channel.allowed_ips.as_deref().unwrap(), ["192.0.2.0/24"]);
        let forward = channel.forward.as_ref().unwrap();
        assert_eq!(forward.interval_seconds, 30);
        assert_eq!(forward.expected_status, 200);
        assert_eq!(forward.timeout_seconds, 15);
        assert_eq!(forward.sign_header, None);
        let backoff = forward.backoff.as_ref().unwrap();
        assert_eq!(backoff.multiplier, Some(1.5));
        assert_eq!(backoff.max_seconds, Some(900));
    }

    #[test]
    fn test_missing_environment_variable_reports_field_path() {
        let yaml = "channels: [{allowed-ips: ['${KWP_MISSING_IP}']}]";
        let mut config = config::Config::builder()
            .add_source(config::File::from_str(yaml, config::FileFormat::Yaml))
            .build()
            .unwrap();

        let error = resolve_environment(&mut config.cache, "", &|_| Err(env::VarError::NotPresent))
            .unwrap_err();
        assert!(error.to_string().contains(
            "environment variable KWP_MISSING_IP for channels[0].allowed-ips[0] is not set"
        ));
    }

    #[test]
    #[serial]
    fn test_env_loader_minimal_success() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "host,content-length");

            // Clean up optional vars to test defaults
            env::remove_var("BIND_ADDRESS");
            env::remove_var("LOG_LEVEL");
            env::remove_var("LOG_TARGET");
            env::remove_var("DATA_PATH");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();

        assert_eq!(config.db_cnn, "sqlite:test.db");
        assert_eq!(config.bind, "0.0.0.0:8080"); // default
        assert!(!config.channels.is_empty());
        assert_eq!(config.channels[0].name, "test");

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_env_loader_full_success() {
        unsafe {
            env::set_var("BIND_ADDRESS", "127.0.0.1:9000");
            env::set_var("LOG_LEVEL", "debug");
            env::set_var("LOG_TARGET", "file");
            env::set_var("DATA_PATH", "/tmp/data");
            env::set_var("DATABASE_URL", "sqlite:full.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "host,content-length");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();

        assert_eq!(config.bind, "127.0.0.1:9000");
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.log_target, "file");
        assert_eq!(config.data_path, "/tmp/data");
        assert_eq!(config.db_cnn, "sqlite:full.db");
        assert_eq!(config.channels[0].name, "test");

        unsafe {
            env::remove_var("BIND_ADDRESS");
            env::remove_var("LOG_LEVEL");
            env::remove_var("LOG_TARGET");
            env::remove_var("DATA_PATH");
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_env_loader_missing_required() {
        unsafe {
            env::remove_var("DATABASE_URL");
            env::set_var("CONFIG_FILE", "/nonexistent/config.yml");
        }

        let loader = EnvConfigLoader;
        // Missing DATABASE_URL is checked first
        assert!(loader.load().is_err());

        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
        }
        // Now config file doesn't exist
        assert!(loader.load().is_err());

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
        }
    }

    #[test]
    #[serial]
    fn test_env_loader_missing_config_file() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", "/nonexistent/config.yml");
        }

        let loader = EnvConfigLoader;
        assert!(loader.load().is_err());

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
        }
    }

    #[test]
    #[serial]
    fn test_env_loader_invalid_yaml() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yml");

        fs::write(&config_path, "invalid yaml: [").unwrap();

        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", config_path.to_str().unwrap());
        }

        let loader = EnvConfigLoader;
        assert!(loader.load().is_err());

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
        }
    }

    #[test]
    #[serial]
    fn test_default_body_limit_no_env() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "host,content-length");
            env::remove_var("DEFAULT_BODY_LIMIT");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();
        assert_eq!(config.default_body_limit, 262_144);

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_default_body_limit_valid_env() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("DEFAULT_BODY_LIMIT", "524288");
            env::set_var("IGNORED_HEADERS", "host,content-length");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();
        assert_eq!(config.default_body_limit, 524_288);

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("DEFAULT_BODY_LIMIT");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_default_body_limit_invalid_env_falls_back() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("DEFAULT_BODY_LIMIT", "not-a-number");
            env::set_var("IGNORED_HEADERS", "host,content-length");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();
        assert_eq!(config.default_body_limit, 262_144);

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("DEFAULT_BODY_LIMIT");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_ignored_headers_required() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::remove_var("IGNORED_HEADERS");
        }

        let loader = EnvConfigLoader;
        // Should fail because IGNORED_HEADERS is required
        assert!(loader.load().is_err());

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
        }
    }

    #[test]
    #[serial]
    fn test_ignored_headers_parsing() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "x-forwarded-for, x-real-ip, host");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();
        assert!(
            config
                .ignored_headers
                .contains(&"x-forwarded-for".to_string())
        );
        assert!(config.ignored_headers.contains(&"x-real-ip".to_string()));
        assert!(config.ignored_headers.contains(&"host".to_string()));

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_ignored_headers_case_insensitive() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "X-Forwarded-For, X-REAL-IP");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();
        // Should be lowercased
        assert!(
            config
                .ignored_headers
                .contains(&"x-forwarded-for".to_string())
        );
        assert!(config.ignored_headers.contains(&"x-real-ip".to_string()));

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_ignored_headers_deduplicates() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            // Provide a header that's already in defaults
            env::set_var("IGNORED_HEADERS", "host, x-custom, host");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();
        let host_count = config
            .ignored_headers
            .iter()
            .filter(|h| *h == "host")
            .count();
        assert_eq!(host_count, 1, "host should appear only once");

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_forward_backoff_defaults_when_unset() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "host");
            env::remove_var("FORWARD_BACKOFF_MULTIPLIER");
            env::remove_var("FORWARD_BACKOFF_MAX_SECONDS");
            env::remove_var("FORWARD_BACKOFF_JITTER");
        }

        let config = EnvConfigLoader.load().unwrap();

        assert_eq!(config.forward_backoff, ForwardBackoffDefaults::default());

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }

    #[test]
    #[serial]
    fn test_forward_backoff_read_from_env() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "host");
            env::set_var("FORWARD_BACKOFF_MULTIPLIER", "1.5");
            env::set_var("FORWARD_BACKOFF_MAX_SECONDS", "900");
            env::set_var("FORWARD_BACKOFF_JITTER", "0.05");
        }

        let config = EnvConfigLoader.load().unwrap();

        assert_eq!(config.forward_backoff.multiplier, 1.5);
        assert_eq!(config.forward_backoff.max_seconds, 900);
        assert_eq!(config.forward_backoff.jitter, 0.05);

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
            env::remove_var("FORWARD_BACKOFF_MULTIPLIER");
            env::remove_var("FORWARD_BACKOFF_MAX_SECONDS");
            env::remove_var("FORWARD_BACKOFF_JITTER");
        }
    }

    /// Garbage in the environment must not stop the service — a warning plus the
    /// default is enough, whereas a value that parses but is nonsensical is
    /// rejected later by `validate_forward_backoff`.
    #[test]
    #[serial]
    fn test_unparsable_forward_backoff_falls_back_to_default() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "host");
            env::set_var("FORWARD_BACKOFF_MULTIPLIER", "two");
            env::set_var("FORWARD_BACKOFF_MAX_SECONDS", "an hour");
        }

        let config = EnvConfigLoader.load().unwrap();
        let defaults = ForwardBackoffDefaults::default();

        assert_eq!(config.forward_backoff.multiplier, defaults.multiplier);
        assert_eq!(config.forward_backoff.max_seconds, defaults.max_seconds);

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
            env::remove_var("FORWARD_BACKOFF_MULTIPLIER");
            env::remove_var("FORWARD_BACKOFF_MAX_SECONDS");
        }
    }

    #[test]
    #[serial]
    fn test_ignored_headers_sorted() {
        unsafe {
            env::set_var("DATABASE_URL", "sqlite:test.db");
            env::set_var("CONFIG_FILE", test_config_path());
            env::set_var("IGNORED_HEADERS", "z-header, a-header, m-header");
        }

        let loader = EnvConfigLoader;
        let config = loader.load().unwrap();
        let mut sorted = config.ignored_headers.clone();
        sorted.sort();
        assert_eq!(config.ignored_headers, sorted);

        unsafe {
            env::remove_var("DATABASE_URL");
            env::remove_var("CONFIG_FILE");
            env::remove_var("IGNORED_HEADERS");
        }
    }
}
