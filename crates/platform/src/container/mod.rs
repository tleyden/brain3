pub mod docker;
pub mod macos_container;
mod process;
pub mod startup;

pub use docker::DockerContainerAdapter;
pub use macos_container::MacOsContainerAdapter;

pub(super) const MCP_NETWORK_NAME: &str = "brain3-mcp-net";
