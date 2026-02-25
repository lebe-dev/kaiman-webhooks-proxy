use std::net::IpAddr;

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::domain::crypto;

#[derive(Clone, Debug, Deserialize, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SecretType {
    #[default]
    Plain,
    HmacSha256,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WebhookForwardConfig {
    pub url: String,
    pub interval_seconds: u64,
    #[serde(default = "default_expected_status")]
    pub expected_status: u16,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    pub sign_header: Option<String>,
    pub sign_secret: Option<String>,
    pub sign_template: Option<String>,
}

fn default_expected_status() -> u16 {
    200
}

fn default_timeout_seconds() -> u64 {
    15
}

impl PartialEq for WebhookForwardConfig {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
            && self.interval_seconds == other.interval_seconds
            && self.expected_status == other.expected_status
            && self.timeout_seconds == other.timeout_seconds
            && self.sign_header == other.sign_header
            && self.sign_secret == other.sign_secret
            && self.sign_template == other.sign_template
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WebhookChannelConfig {
    pub name: String,
    pub api_read_token: String,
    pub webhook_secret: Option<String>,
    pub secret_header: Option<String>,
    #[serde(default)]
    pub secret_type: SecretType,
    pub secret_extract_template: Option<String>,
    pub forward: Option<WebhookForwardConfig>,
    #[serde(default)]
    pub max_body_size: Option<usize>,
    #[serde(default)]
    pub allowed_ips: Option<Vec<String>>,
}

impl WebhookChannelConfig {
    /// Returns `true` if the given IP is allowed to send to this channel.
    /// If `allowed_ips` is `None`, all IPs are allowed.
    pub fn is_ip_allowed(&self, ip: &IpAddr) -> bool {
        let Some(entries) = &self.allowed_ips else {
            return true;
        };
        entries.iter().any(|entry| {
            if let Ok(net) = entry.parse::<IpNet>() {
                net.contains(ip)
            } else if let Ok(allowed) = entry.parse::<IpAddr>() {
                allowed == *ip
            } else {
                false
            }
        })
    }
}

impl PartialEq for WebhookChannelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.api_read_token == other.api_read_token
            && self.webhook_secret == other.webhook_secret
            && self.secret_header == other.secret_header
            && self.secret_type == other.secret_type
            && self.secret_extract_template == other.secret_extract_template
            && self.forward == other.forward
            && self.max_body_size == other.max_body_size
            && self.allowed_ips == other.allowed_ips
    }
}

const MIN_BODY_LIMIT: usize = 64;
const MAX_BODY_LIMIT: usize = 104_857_600; // 100 MB

#[derive(PartialEq, Clone, Debug)]
pub struct AppConfig {
    pub bind: String,
    pub log_level: String,
    pub log_target: String,
    pub data_path: String,
    pub db_cnn: String,
    pub channels: Vec<WebhookChannelConfig>,
    pub default_body_limit: usize,
    pub ignored_headers: Vec<String>,
}

impl AppConfig {
    /// Constant-time token lookup — for GET (client reads webhooks).
    pub fn find_channel_by_token(&self, bearer: &str) -> Option<&WebhookChannelConfig> {
        self.channels
            .iter()
            .find(|c| c.api_read_token.as_bytes().ct_eq(bearer.as_bytes()).into())
    }

    /// Plain name lookup — for POST (incoming webhook routing).
    pub fn find_channel_by_name(&self, name: &str) -> Option<&WebhookChannelConfig> {
        self.channels.iter().find(|c| c.name == name)
    }

    /// Returns the maximum body limit across all channels and the global default.
    /// Used to set Axum's DefaultBodyLimit layer.
    pub fn max_body_limit(&self) -> usize {
        self.channels
            .iter()
            .filter_map(|c| c.max_body_size)
            .max()
            .unwrap_or(self.default_body_limit)
            .max(self.default_body_limit)
    }

    /// Validates allowed_ips entries at startup. Returns Err on invalid entry.
    pub fn validate_allowed_ips(&self) -> Result<(), String> {
        for ch in &self.channels {
            let Some(entries) = &ch.allowed_ips else {
                continue;
            };
            for entry in entries {
                if entry.parse::<IpNet>().is_err() && entry.parse::<IpAddr>().is_err() {
                    return Err(format!(
                        "channel '{}': invalid allowed-ips entry '{}'",
                        ch.name, entry
                    ));
                }
            }
        }
        Ok(())
    }

    /// Validates HMAC/template configuration at startup. Returns Err on misconfiguration.
    pub fn validate_templates(&self) -> Result<(), String> {
        for ch in &self.channels {
            if ch.secret_type == SecretType::HmacSha256 {
                if ch.webhook_secret.is_none() {
                    return Err(format!(
                        "channel '{}': hmac-sha256 requires webhook-secret",
                        ch.name
                    ));
                }
                if ch.secret_header.is_none() {
                    return Err(format!(
                        "channel '{}': hmac-sha256 requires secret-header",
                        ch.name
                    ));
                }
            }
            if let Some(tmpl) = &ch.secret_extract_template {
                crypto::validate_template(tmpl).map_err(|e| {
                    format!(
                        "channel '{}': invalid secret-extract-template: {}",
                        ch.name, e
                    )
                })?;
            }
            if let Some(fwd) = &ch.forward {
                match (&fwd.sign_header, &fwd.sign_secret) {
                    (Some(_), None) => {
                        return Err(format!(
                            "channel '{}': sign-header requires sign-secret",
                            ch.name
                        ));
                    }
                    (None, Some(_)) => {
                        return Err(format!(
                            "channel '{}': sign-secret requires sign-header",
                            ch.name
                        ));
                    }
                    _ => {}
                }
                if let Some(tmpl) = &fwd.sign_template {
                    crypto::validate_template(tmpl).map_err(|e| {
                        format!("channel '{}': invalid sign-template: {}", ch.name, e)
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Validates body limit values at startup. Returns Err on invalid config.
    pub fn validate_body_limits(&self) -> Result<(), String> {
        if !(MIN_BODY_LIMIT..=MAX_BODY_LIMIT).contains(&self.default_body_limit) {
            return Err(format!(
                "default_body_limit {} is out of range [{}, {}]",
                self.default_body_limit, MIN_BODY_LIMIT, MAX_BODY_LIMIT
            ));
        }
        for ch in &self.channels {
            if let Some(limit) = ch.max_body_size
                && !(MIN_BODY_LIMIT..=MAX_BODY_LIMIT).contains(&limit)
            {
                return Err(format!(
                    "channel '{}': max-body-size {} is out of range [{}, {}]",
                    ch.name, limit, MIN_BODY_LIMIT, MAX_BODY_LIMIT
                ));
            }
        }
        Ok(())
    }
}

#[derive(PartialEq, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigDto {
    pub bind: String,
    pub log_level: String,
    pub log_target: String,
    pub data_path: String,
}

impl From<AppConfig> for AppConfigDto {
    fn from(config: AppConfig) -> Self {
        AppConfigDto {
            bind: config.bind,
            log_level: config.log_level,
            log_target: config.log_target,
            data_path: config.data_path,
        }
    }
}

#[derive(Debug, Error)]
pub enum LoadAppConfigError {
    #[error(transparent)]
    Unknown(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app_config(
        default_body_limit: usize,
        channels: Vec<WebhookChannelConfig>,
    ) -> AppConfig {
        AppConfig {
            bind: "0.0.0.0:8080".to_string(),
            log_level: "info".to_string(),
            log_target: "stdout".to_string(),
            data_path: "./data".to_string(),
            db_cnn: "sqlite:test.db".to_string(),
            channels,
            default_body_limit,
            ignored_headers: vec![],
        }
    }

    fn make_channel(name: &str, max_body_size: Option<usize>) -> WebhookChannelConfig {
        WebhookChannelConfig {
            name: name.to_string(),
            api_read_token: "token".to_string(),
            webhook_secret: None,
            secret_header: None,
            secret_type: SecretType::Plain,
            secret_extract_template: None,
            forward: None,
            max_body_size,
            allowed_ips: None,
        }
    }

    #[test]
    fn test_channel_config_max_body_size_present() {
        let yaml = r#"
name: test
api-read-token: tok
max-body-size: 1048576
"#;
        let cfg: WebhookChannelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.max_body_size, Some(1_048_576));
    }

    #[test]
    fn test_channel_config_max_body_size_absent() {
        let yaml = r#"
name: test
api-read-token: tok
"#;
        let cfg: WebhookChannelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.max_body_size, None);
    }

    #[test]
    fn test_max_body_limit_uses_largest() {
        let channels = vec![
            make_channel("a", Some(1_048_576)), // 1 MB
            make_channel("b", None),
        ];
        let config = make_app_config(262_144, channels); // global = 256 KB
        assert_eq!(config.max_body_limit(), 1_048_576);
    }

    #[test]
    fn test_max_body_limit_no_overrides() {
        let channels = vec![make_channel("a", None)];
        let config = make_app_config(262_144, channels);
        assert_eq!(config.max_body_limit(), 262_144);
    }

    #[test]
    fn test_validate_body_limits_zero_rejected() {
        let channels = vec![make_channel("a", Some(0))];
        let config = make_app_config(262_144, channels);
        assert!(config.validate_body_limits().is_err());
    }

    #[test]
    fn test_validate_body_limits_exceeds_max_rejected() {
        let channels = vec![make_channel("a", Some(200_000_000))]; // 200 MB
        let config = make_app_config(262_144, channels);
        assert!(config.validate_body_limits().is_err());
    }

    #[test]
    fn test_validate_body_limits_valid() {
        let channels = vec![make_channel("a", Some(524_288))];
        let config = make_app_config(262_144, channels);
        assert!(config.validate_body_limits().is_ok());
    }

    #[test]
    fn test_forward_config_deserialization_full() {
        let yaml = r#"
url: https://example.com/hook
interval-seconds: 30
expected-status: 201
timeout-seconds: 10
"#;
        let cfg: WebhookForwardConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.url, "https://example.com/hook");
        assert_eq!(cfg.interval_seconds, 30);
        assert_eq!(cfg.expected_status, 201);
        assert_eq!(cfg.timeout_seconds, 10);
    }

    #[test]
    fn test_forward_config_defaults() {
        let yaml = r#"
url: https://example.com/hook
interval-seconds: 60
"#;
        let cfg: WebhookForwardConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.expected_status, 200);
        assert_eq!(cfg.timeout_seconds, 15);
    }

    #[test]
    fn test_channel_config_with_forward() {
        let yaml = r#"
channels:
  - name: telegram
    api-read-token: abc123
    webhook-secret: mysecret
    secret-header: X-Telegram-Bot-Api-Secret-Token
    forward:
      url: https://my-app.local/telegram-hook
      interval-seconds: 30
  - name: open
    api-read-token: def456
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            channels: Vec<WebhookChannelConfig>,
        }
        let w: Wrapper = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(w.channels.len(), 2);
        assert!(w.channels[0].forward.is_some());
        assert!(w.channels[1].forward.is_none());
        let fwd = w.channels[0].forward.as_ref().unwrap();
        assert_eq!(fwd.url, "https://my-app.local/telegram-hook");
        assert_eq!(fwd.interval_seconds, 30);
        assert_eq!(fwd.expected_status, 200);
    }

    #[test]
    fn test_allowed_ips_deserialization_present() {
        let yaml = r#"
name: test
api-read-token: tok
allowed-ips:
  - "192.168.1.1"
  - "10.0.0.0/8"
"#;
        let cfg: WebhookChannelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cfg.allowed_ips,
            Some(vec!["192.168.1.1".to_string(), "10.0.0.0/8".to_string()])
        );
    }

    #[test]
    fn test_allowed_ips_deserialization_absent() {
        let yaml = r#"
name: test
api-read-token: tok
"#;
        let cfg: WebhookChannelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.allowed_ips, None);
    }

    #[test]
    fn test_is_ip_allowed_none_allows_all() {
        let ch = make_channel("test", None);
        assert!(ch.is_ip_allowed(&"1.2.3.4".parse().unwrap()));
        assert!(ch.is_ip_allowed(&"::1".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_allowed_single_ip_match() {
        let mut ch = make_channel("test", None);
        ch.allowed_ips = Some(vec!["192.168.1.10".to_string()]);
        assert!(ch.is_ip_allowed(&"192.168.1.10".parse().unwrap()));
        assert!(!ch.is_ip_allowed(&"192.168.1.11".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_allowed_cidr_match() {
        let mut ch = make_channel("test", None);
        ch.allowed_ips = Some(vec!["10.0.0.0/8".to_string()]);
        assert!(ch.is_ip_allowed(&"10.1.2.3".parse().unwrap()));
        assert!(!ch.is_ip_allowed(&"11.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_allowed_empty_list_blocks_all() {
        let mut ch = make_channel("test", None);
        ch.allowed_ips = Some(vec![]);
        assert!(!ch.is_ip_allowed(&"1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_allowed_ipv6() {
        let mut ch = make_channel("test", None);
        ch.allowed_ips = Some(vec!["::1".to_string()]);
        assert!(ch.is_ip_allowed(&"::1".parse().unwrap()));
        assert!(!ch.is_ip_allowed(&"::2".parse().unwrap()));
    }

    #[test]
    fn test_validate_allowed_ips_valid() {
        let mut ch = make_channel("a", None);
        ch.allowed_ips = Some(vec!["1.2.3.4".to_string(), "10.0.0.0/8".to_string()]);
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_allowed_ips().is_ok());
    }

    #[test]
    fn test_validate_allowed_ips_invalid_entry() {
        let mut ch = make_channel("a", None);
        ch.allowed_ips = Some(vec!["not-an-ip".to_string()]);
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_allowed_ips().is_err());
    }

    #[test]
    fn test_validate_allowed_ips_none_is_ok() {
        let config = make_app_config(262_144, vec![make_channel("a", None)]);
        assert!(config.validate_allowed_ips().is_ok());
    }

    #[test]
    fn test_secret_type_defaults_to_plain() {
        let yaml = r#"
name: test
api-read-token: tok
"#;
        let cfg: WebhookChannelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.secret_type, SecretType::Plain);
    }

    #[test]
    fn test_secret_type_hmac_sha256_parses() {
        let yaml = r#"
name: test
api-read-token: tok
webhook-secret: sec
secret-header: X-Hub-Signature-256
secret-type: hmac-sha256
"#;
        let cfg: WebhookChannelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.secret_type, SecretType::HmacSha256);
    }

    #[test]
    fn test_forward_config_with_signing_fields() {
        let yaml = r#"
url: https://example.com/hook
interval-seconds: 30
sign-header: X-Sig
sign-secret: mysecret
sign-template: "sha256={{ signature }}"
"#;
        let cfg: WebhookForwardConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.sign_header, Some("X-Sig".to_string()));
        assert_eq!(cfg.sign_secret, Some("mysecret".to_string()));
        assert_eq!(
            cfg.sign_template,
            Some("sha256={{ signature }}".to_string())
        );
    }

    #[test]
    fn test_validate_templates_hmac_missing_secret() {
        let mut ch = make_channel("a", None);
        ch.secret_type = SecretType::HmacSha256;
        ch.secret_header = Some("X-Sig".to_string());
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_templates().is_err());
    }

    #[test]
    fn test_validate_templates_hmac_missing_header() {
        let mut ch = make_channel("a", None);
        ch.secret_type = SecretType::HmacSha256;
        ch.webhook_secret = Some("sec".to_string());
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_templates().is_err());
    }

    #[test]
    fn test_validate_templates_hmac_valid() {
        let mut ch = make_channel("a", None);
        ch.secret_type = SecretType::HmacSha256;
        ch.webhook_secret = Some("sec".to_string());
        ch.secret_header = Some("X-Sig".to_string());
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_templates().is_ok());
    }

    #[test]
    fn test_validate_templates_invalid_extract_template() {
        let mut ch = make_channel("a", None);
        ch.secret_extract_template = Some("{{ unclosed".to_string());
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_templates().is_err());
    }

    #[test]
    fn test_validate_templates_sign_header_without_secret() {
        let mut ch = make_channel("a", None);
        ch.forward = Some(WebhookForwardConfig {
            url: "https://x.com".to_string(),
            interval_seconds: 10,
            expected_status: 200,
            timeout_seconds: 15,
            sign_header: Some("X-Sig".to_string()),
            sign_secret: None,
            sign_template: None,
        });
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_templates().is_err());
    }

    #[test]
    fn test_validate_templates_sign_secret_without_header() {
        let mut ch = make_channel("a", None);
        ch.forward = Some(WebhookForwardConfig {
            url: "https://x.com".to_string(),
            interval_seconds: 10,
            expected_status: 200,
            timeout_seconds: 15,
            sign_header: None,
            sign_secret: Some("sec".to_string()),
            sign_template: None,
        });
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_templates().is_err());
    }

    #[test]
    fn test_validate_templates_invalid_sign_template() {
        let mut ch = make_channel("a", None);
        ch.forward = Some(WebhookForwardConfig {
            url: "https://x.com".to_string(),
            interval_seconds: 10,
            expected_status: 200,
            timeout_seconds: 15,
            sign_header: Some("X-Sig".to_string()),
            sign_secret: Some("sec".to_string()),
            sign_template: Some("{{ unclosed".to_string()),
        });
        let config = make_app_config(262_144, vec![ch]);
        assert!(config.validate_templates().is_err());
    }

    #[test]
    fn test_validate_templates_all_ok() {
        let config = make_app_config(262_144, vec![make_channel("a", None)]);
        assert!(config.validate_templates().is_ok());
    }
}
