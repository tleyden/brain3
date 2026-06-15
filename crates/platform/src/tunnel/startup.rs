use std::path::PathBuf;

use brain3_core::domain::errors::TunnelError;
use brain3_core::domain::model::TunnelConfig;
use brain3_core::ports::tunnel::{TunnelInfo, TunnelPort};

use super::{CloudflareNamedTunnelAdapter, CloudflareQuickTunnelAdapter};

pub async fn start_tunnel(
    config: &TunnelConfig,
    pid_file: PathBuf,
) -> Result<(Box<dyn TunnelPort>, TunnelInfo), TunnelError> {
    let adapter: Box<dyn TunnelPort> = match config {
        TunnelConfig::CloudflareQuick { local_port } => {
            Box::new(CloudflareQuickTunnelAdapter::new(*local_port, pid_file))
        }
        TunnelConfig::CloudflareNamed {
            tunnel_name,
            domain,
            config_file,
            ..
        } => Box::new(CloudflareNamedTunnelAdapter::new(
            tunnel_name,
            domain,
            config_file.clone(),
            pid_file,
        )),
    };
    let info = adapter.start().await?;
    Ok((adapter, info))
}
