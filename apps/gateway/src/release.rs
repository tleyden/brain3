pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_VERSION_DISPLAY: &str = concat!("Brain3 v", env!("CARGO_PKG_VERSION"));
pub const HELP_ABOUT: &str = concat!(
    "Brain3 v",
    env!("CARGO_PKG_VERSION"),
    "\nOAuth2 gateway for MCP servers"
);

pub const MCP_IMAGE_REPO: &str = "ghcr.io/tleyden/brain3-mcp-vault-tools";

pub fn default_container_image() -> String {
    format!("{MCP_IMAGE_REPO}:v{APP_VERSION}")
}

pub fn official_latest_container_image() -> String {
    format!("{MCP_IMAGE_REPO}:latest")
}

pub fn is_official_latest_container_image(image: &str) -> bool {
    image.trim() == official_latest_container_image()
}

pub fn container_image_for_tag(tag: &str) -> String {
    format!("{MCP_IMAGE_REPO}:{}", tag.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_container_image_uses_versioned_tag() {
        assert_eq!(
            default_container_image(),
            format!("{MCP_IMAGE_REPO}:v{APP_VERSION}")
        );
    }

    #[test]
    fn container_image_for_tag_uses_official_repo() {
        assert_eq!(
            container_image_for_tag("pr-123"),
            format!("{MCP_IMAGE_REPO}:pr-123")
        );
    }

    #[test]
    fn detects_official_latest_image() {
        assert!(is_official_latest_container_image(
            &official_latest_container_image()
        ));
        assert!(!is_official_latest_container_image(
            "ghcr.io/tleyden/brain3-mcp-vault-tools:v0.1.5"
        ));
    }
}
