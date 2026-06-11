pub const ENV_TEMPLATE: &str = include_str!("../../../../.env.template");

pub fn embedded_env_template() -> &'static str {
    ENV_TEMPLATE
}
