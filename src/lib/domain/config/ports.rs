use super::model::{AppConfig, LoadAppConfigError};

pub trait AppConfigLoader: Clone + Send + Sync + 'static {
    fn load(&self) -> Result<AppConfig, LoadAppConfigError>;
}
