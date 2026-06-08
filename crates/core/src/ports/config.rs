use crate::domain::errors::ConfigError;
use crate::domain::model::GatewayConfig;

pub trait ConfigPort {
    fn load(&self) -> Result<GatewayConfig, ConfigError>;
}
