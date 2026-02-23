use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ForwardConfig {
    pub url: String,
    pub interval_seconds: u64,
    #[serde(default = "default_expected_status")]
    pub expected_status: u16,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_expected_status() -> u16 {
    200
}

fn default_timeout_seconds() -> u64 {
    15
}

impl PartialEq for ForwardConfig {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
            && self.interval_seconds == other.interval_seconds
            && self.expected_status == other.expected_status
            && self.timeout_seconds == other.timeout_seconds
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WebhookChannelConfig {
    pub name: String,
    pub api_read_token: String,
    pub webhook_secret: Option<String>,
    pub secret_header: Option<String>,
    pub forward: Option<ForwardConfig>,
}

impl PartialEq for WebhookChannelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.api_read_token == other.api_read_token
            && self.webhook_secret == other.webhook_secret
            && self.secret_header == other.secret_header
            && self.forward == other.forward
    }
}

#[derive(PartialEq, Clone, Debug)]
pub struct AppConfig {
    pub bind: String,
    pub log_level: String,
    pub log_target: String,
    pub data_path: String,
    pub db_cnn: String,
    pub channels: Vec<WebhookChannelConfig>,
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

    #[test]
    fn test_forward_config_deserialization_full() {
        let yaml = r#"
url: https://example.com/hook
interval-seconds: 30
expected-status: 201
timeout-seconds: 10
"#;
        let cfg: ForwardConfig = serde_yaml::from_str(yaml).unwrap();
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
        let cfg: ForwardConfig = serde_yaml::from_str(yaml).unwrap();
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
}
