mod logging;
mod release;
mod server;
mod setup_tui;
#[allow(dead_code)]
mod tui;

use std::io::{stderr, stdin, stdout, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use brain3_core::domain::model::{GatewayConfig, TunnelConfig};
use brain3_core::domain::setup::{
    DependencyAvailability, DependencyStatus, RuntimeLaunchPlan, SetupDefaults,
};
use brain3_core::ports::config::ConfigPort;
use brain3_core::ports::setup_system::SetupSystemPort;
use brain3_platform::config::env_file::EnvFileConfigAdapter;
use brain3_platform::runtime::{bootstrap_configured_runtime, named_tunnel_setup_config};
use brain3_platform::setup::app_home::Brain3AppHome;
use brain3_platform::setup::PlatformSetupSystem;

use crate::tui::GatewayTuiLaunch;

const DEFAULT_HOST: &str = "127.0.0.1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

#[derive(Parser)]
#[command(
    name = "brain3",
    about = "OAuth2 gateway for MCP servers",
    long_about = release::HELP_ABOUT,
    version = release::APP_VERSION,
    long_version = release::APP_VERSION
)]
struct Args {
    #[arg(long, default_value = DEFAULT_HOST)]
    host: String,

    #[arg(long)]
    env_file: Option<PathBuf>,

    #[arg(long, conflicts_with_all = ["cli", "cf_setup"])]
    tui: bool,

    #[arg(long, conflicts_with_all = ["tui", "cf_setup"])]
    cli: bool,

    #[arg(
        long = "cf-setup",
        conflicts_with_all = ["tui", "cli"],
        help = "Run the interactive setup wizard for Cloudflare named tunnel provisioning"
    )]
    cf_setup: bool,

    #[arg(
        long,
        help = "Override the Brain3 MCP container tag for this run or new setup, e.g. latest, v0.1.6, pr-123"
    )]
    container_tag: Option<String>,

    #[arg(long, value_enum, default_value = "info")]
    log_level: LogLevel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RuntimeOverrides {
    container_tag: Option<String>,
}

impl RuntimeOverrides {
    fn from_args(args: &Args) -> Self {
        Self {
            container_tag: args.container_tag.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EffectiveContainerImageSource {
    FreshInstallDefault,
    ConfiguredImage,
    LegacyLatestConfig,
    ExplicitTagOverride,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveContainerImage {
    image: String,
    source: EffectiveContainerImageSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchMode {
    Tui,
    Cli,
    Setup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvFileSource {
    Default,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedEnvFile {
    app_home: Brain3AppHome,
    env_file: PathBuf,
    source: EnvFileSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LaunchDispatch {
    TuiFirstRun,
    TuiConfigured { launch_plan: RuntimeLaunchPlan },
    Cli { launch_plan: RuntimeLaunchPlan },
    Setup { env_file: PathBuf },
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    tracing::info!("Received shutdown signal, draining connections...");
}

fn choose_launch_mode(args: &Args) -> LaunchMode {
    if args.cf_setup {
        LaunchMode::Setup
    } else if args.cli {
        LaunchMode::Cli
    } else {
        LaunchMode::Tui
    }
}

fn resolve_config_env_file(args: &Args) -> Result<ResolvedEnvFile> {
    let app_home =
        Brain3AppHome::resolve_from_env().context("failed to resolve Brain3 app home")?;
    let source = if args.env_file.is_some() {
        EnvFileSource::Custom
    } else {
        EnvFileSource::Default
    };
    let env_file = args
        .env_file
        .clone()
        .unwrap_or_else(|| app_home.env_file.clone());
    Ok(ResolvedEnvFile {
        app_home,
        env_file,
        source,
    })
}

fn setup_requires_named_tunnel() -> Result<()> {
    eprintln!(
        "\nBrain3 --cf-setup only applies to Cloudflare named tunnel provisioning.\n\
         \nRun this in an interactive terminal to use the normal setup/status flow:\n  brain3 --tui\n"
    );
    anyhow::bail!("--cf-setup requires B3_CF_TUNNEL_NAME and B3_CF_DOMAIN to be set");
}

fn runtime_launch_plan(resolved_env: &ResolvedEnvFile, log_file: PathBuf) -> RuntimeLaunchPlan {
    RuntimeLaunchPlan {
        paths: resolved_env.app_home.as_setup_paths(),
        env_file: resolved_env.env_file.clone(),
        log_file,
    }
}

fn plan_launch(
    mode: LaunchMode,
    resolved_env: &ResolvedEnvFile,
    env_exists: bool,
    log_file: PathBuf,
) -> Result<LaunchDispatch> {
    if !env_exists {
        return match resolved_env.source {
            EnvFileSource::Custom => missing_custom_env_file(&resolved_env.env_file),
            EnvFileSource::Default => match mode {
                LaunchMode::Tui => Ok(LaunchDispatch::TuiFirstRun),
                LaunchMode::Cli => {
                    cli_requires_interactive_setup(&resolved_env.app_home, &resolved_env.env_file)
                }
                LaunchMode::Setup => {
                    setup_requires_existing_config(&resolved_env.app_home, &resolved_env.env_file)
                }
            },
        };
    }

    let launch_plan = runtime_launch_plan(resolved_env, log_file);

    Ok(match mode {
        LaunchMode::Tui => LaunchDispatch::TuiConfigured { launch_plan },
        LaunchMode::Cli => LaunchDispatch::Cli { launch_plan },
        LaunchMode::Setup => LaunchDispatch::Setup {
            env_file: resolved_env.env_file.clone(),
        },
    })
}

fn resolve_effective_container_image(
    configured_image: Option<&str>,
    container_tag: Option<&str>,
) -> EffectiveContainerImage {
    if let Some(tag) = container_tag {
        return EffectiveContainerImage {
            image: release::container_image_for_tag(tag),
            source: EffectiveContainerImageSource::ExplicitTagOverride,
        };
    }

    if let Some(image) = configured_image {
        if release::is_official_latest_container_image(image) {
            return EffectiveContainerImage {
                image: release::default_container_image(),
                source: EffectiveContainerImageSource::LegacyLatestConfig,
            };
        }

        return EffectiveContainerImage {
            image: image.trim().to_string(),
            source: EffectiveContainerImageSource::ConfiguredImage,
        };
    }

    EffectiveContainerImage {
        image: release::default_container_image(),
        source: EffectiveContainerImageSource::FreshInstallDefault,
    }
}

fn setup_defaults(runtime_overrides: &RuntimeOverrides) -> SetupDefaults {
    let effective =
        resolve_effective_container_image(None, runtime_overrides.container_tag.as_deref());

    SetupDefaults {
        default_container_image: effective.image,
    }
}

fn apply_runtime_overrides(
    config: &mut GatewayConfig,
    runtime_overrides: &RuntimeOverrides,
) -> Result<()> {
    let Some(container) = config.container.as_mut() else {
        if runtime_overrides.container_tag.is_some() {
            anyhow::bail!("--container-tag requires container startup configuration");
        }
        return Ok(());
    };

    let effective = resolve_effective_container_image(
        Some(container.image.as_str()),
        runtime_overrides.container_tag.as_deref(),
    );
    container.image = effective.image.clone();

    match effective.source {
        EffectiveContainerImageSource::FreshInstallDefault => {
            tracing::info!(image = %container.image, "resolved MCP container image");
        }
        EffectiveContainerImageSource::ConfiguredImage => {
            tracing::info!(image = %container.image, source = ?effective.source, "resolved MCP container image");
        }
        EffectiveContainerImageSource::LegacyLatestConfig => {
            tracing::warn!(
                image = %container.image,
                source = ?effective.source,
                "remapping legacy official :latest MCP image to the release-matched default"
            );
        }
        EffectiveContainerImageSource::ExplicitTagOverride => {
            tracing::info!(
                image = %container.image,
                source = ?effective.source,
                tag = runtime_overrides.container_tag.as_deref().unwrap_or_default(),
                "resolved MCP container image"
            );
        }
    }

    Ok(())
}

fn missing_custom_env_file(env_file: &Path) -> Result<LaunchDispatch> {
    eprintln!(
        "\nCustom Brain3 config file not found.\n\
         \n  Env file: {}\n\
         \nCreate that file or point --env-file at an existing config.\n",
        env_file.display()
    );
    tracing::warn!(env_file = %env_file.display(), "custom env file missing");
    anyhow::bail!("custom env file not found: {}", env_file.display());
}

fn is_interactive_terminal() -> bool {
    stdin().is_terminal() && stdout().is_terminal() && stderr().is_terminal()
}

fn brain3_command(args: &Args, mode: LaunchMode) -> String {
    let mut parts = vec!["brain3".to_string()];

    match mode {
        LaunchMode::Tui => parts.push("--tui".into()),
        LaunchMode::Cli => parts.push("--cli".into()),
        LaunchMode::Setup => parts.push("--cf-setup".into()),
    }

    if args.host != DEFAULT_HOST {
        parts.push("--host".into());
        parts.push(shell_quote(&args.host));
    }

    if let Some(env_file) = &args.env_file {
        parts.push("--env-file".into());
        parts.push(shell_quote(&env_file.display().to_string()));
    }

    if let Some(container_tag) = &args.container_tag {
        parts.push("--container-tag".into());
        parts.push(shell_quote(container_tag));
    }

    parts.join(" ")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }

    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b':' | b'=')
    }) {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn noninteractive_first_run_requires_tui(
    args: &Args,
    resolved_env: &ResolvedEnvFile,
) -> Result<()> {
    let tui_command = brain3_command(args, LaunchMode::Tui);
    eprintln!(
        "\nBrain3 needs interactive first-run setup.\n\
         \n  App home: {}\n\
         \n  Expected config: {}\n\
         \nRun this in an interactive terminal:\n  {}\n",
        resolved_env.app_home.root_dir.display(),
        resolved_env.env_file.display(),
        tui_command
    );
    tracing::warn!(
        app_home = %resolved_env.app_home.root_dir.display(),
        env_file = %resolved_env.env_file.display(),
        command = %tui_command,
        "default TUI launch refused because the terminal is non-interactive during first-run"
    );
    anyhow::bail!("first-run setup requires an interactive terminal; run: {tui_command}");
}

fn noninteractive_configured_launch_guidance(
    args: &Args,
    resolved_env: &ResolvedEnvFile,
) -> Result<()> {
    let tui_command = brain3_command(args, LaunchMode::Tui);
    let cli_command = brain3_command(args, LaunchMode::Cli);
    eprintln!(
        "\nBrain3 default launch uses the interactive status dashboard.\n\
         \n  Env file: {}\n\
         \nRun this in an interactive terminal for the dashboard:\n  {}\n\
         \nOr use the foreground non-TUI startup path:\n  {}\n",
        resolved_env.env_file.display(),
        tui_command,
        cli_command
    );
    tracing::warn!(
        env_file = %resolved_env.env_file.display(),
        tui_command = %tui_command,
        cli_command = %cli_command,
        "default TUI launch refused because the terminal is non-interactive for a configured install"
    );
    anyhow::bail!(
        "default TUI launch requires an interactive terminal; run: {tui_command} or {cli_command}"
    );
}

fn named_tunnel_setup_requires_tui(args: &Args, resolved_env: &ResolvedEnvFile) -> Result<()> {
    let tui_command = brain3_command(args, LaunchMode::Tui);
    eprintln!(
        "\nBrain3 needs interactive Cloudflare named-tunnel setup before startup can continue.\n\
         \n  Env file: {}\n\
         \nRun this in an interactive terminal:\n  {}\n",
        resolved_env.env_file.display(),
        tui_command
    );
    tracing::warn!(
        env_file = %resolved_env.env_file.display(),
        command = %tui_command,
        "configured startup requires interactive named-tunnel setup"
    );
    anyhow::bail!("named-tunnel setup requires an interactive terminal; run: {tui_command}");
}

fn cli_requires_interactive_setup(
    app_home: &Brain3AppHome,
    env_file: &Path,
) -> Result<LaunchDispatch> {
    eprintln!(
        "\nBrain3 --cli only works after interactive setup is complete.\n\
         \n  App home: {}\n\
         \n  Expected config: {}\n\
         \nRerun without --cli to use the setup/status TUI.\n",
        app_home.root_dir.display(),
        env_file.display()
    );
    tracing::warn!(
        app_home = %app_home.root_dir.display(),
        env_file = %env_file.display(),
        "cli mode refused because interactive setup is incomplete"
    );
    anyhow::bail!("--cli requires completed interactive setup; rerun without --cli");
}

fn setup_requires_existing_config(
    app_home: &Brain3AppHome,
    env_file: &Path,
) -> Result<LaunchDispatch> {
    eprintln!(
        "\nBrain3 --cf-setup only provisions a Cloudflare named tunnel after Brain3 is configured.\n\
         \n  App home: {}\n\
         \n  Expected config: {}\n\
         \nRun this in an interactive terminal to create or manage configuration:\n  brain3 --tui\n",
        app_home.root_dir.display(),
        env_file.display()
    );
    tracing::warn!(
        app_home = %app_home.root_dir.display(),
        env_file = %env_file.display(),
        "setup mode refused because config is missing"
    );
    anyhow::bail!("--cf-setup requires existing configuration");
}

fn ensure_cli_ready(dependencies: &DependencyStatus) -> Result<()> {
    let runtime_ready = matches!(
        dependencies.preferred_container_runtime,
        DependencyAvailability::Installed
    );
    let tunnel_ready = matches!(dependencies.cloudflared, DependencyAvailability::Installed);

    if runtime_ready && tunnel_ready {
        return Ok(());
    }

    eprintln!(
        "\nBrain3 --cli only works after interactive setup is complete.\n\
         \nDependency doctor still reports setup work for the default runtime.\n\
         Rerun without --cli to use the setup/status TUI.\n"
    );
    tracing::warn!(dependencies = ?dependencies, "cli mode refused because dependency doctor is not green");
    anyhow::bail!(
        "--cli requires interactive setup to finish dependency installation; rerun without --cli"
    );
}

fn load_config(
    env_file: PathBuf,
    runtime_overrides: &RuntimeOverrides,
) -> Result<Arc<brain3_core::domain::model::GatewayConfig>> {
    let config_adapter = EnvFileConfigAdapter::new(Some(env_file));
    let mut config = config_adapter
        .load()
        .context("failed to load configuration")?;
    apply_runtime_overrides(&mut config, runtime_overrides)?;
    Ok(Arc::new(config))
}

async fn run_setup_mode(env_file: PathBuf) -> Result<()> {
    let config = load_config(env_file, &RuntimeOverrides::default())?;

    match &config.tunnel {
        Some(tc @ TunnelConfig::CloudflareNamed { .. }) => setup_tui::run(tc).await,
        _ => setup_requires_named_tunnel(),
    }
}

async fn run_cli_mode(
    host: &str,
    config: Arc<GatewayConfig>,
    launch_plan: RuntimeLaunchPlan,
    logging: &logging::GatewayLogging,
) -> Result<()> {
    let setup_system = PlatformSetupSystem::new();
    let dependencies = setup_system
        .collect_dependency_status()
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    ensure_cli_ready(&dependencies)?;

    logging.enable_terminal_mirror();

    let runtime = bootstrap_configured_runtime(Arc::clone(&config), launch_plan).await?;

    if let Some(public_url) = &runtime.public_url {
        tracing::info!(url = %public_url, "runtime public URL ready");
    }
    tracing::info!(
        container_status = ?runtime.container_status,
        tunnel_status = ?runtime.tunnel_status,
        log_file = %runtime.launch_plan.log_file.display(),
        "runtime bootstrap complete"
    );

    if !runtime.can_start_gateway() {
        let summary = runtime
            .primary_failure_summary()
            .unwrap_or("MCP container failed to start");
        anyhow::bail!(
            "Brain3 startup failed: {summary}. See logs: {}",
            runtime.launch_plan.log_file.display()
        );
    }

    if let Some(summary) = runtime.tunnel_status.failure_summary() {
        tracing::warn!(
            summary,
            "tunnel failed to start; continuing with local gateway only"
        );
    }

    server::run_gateway_server_until(
        host,
        config,
        runtime.upstream_secret.clone(),
        shutdown_signal(),
    )
    .await
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let runtime_overrides = RuntimeOverrides::from_args(&args);
    let logging = logging::init_logging(args.log_level.as_str()).await?;
    let resolved_env = resolve_config_env_file(&args)?;
    let interactive_terminal = is_interactive_terminal();
    let mode = choose_launch_mode(&args);
    let dispatch = plan_launch(
        mode,
        &resolved_env,
        resolved_env.env_file.exists(),
        logging.log_file.clone(),
    )?;

    match dispatch {
        LaunchDispatch::TuiFirstRun => {
            if !interactive_terminal {
                return noninteractive_first_run_requires_tui(&args, &resolved_env);
            }
            tui::run_gateway_tui(
                &args.host,
                logging.log_file.clone(),
                GatewayTuiLaunch::FirstRun,
                setup_defaults(&runtime_overrides),
                runtime_overrides.clone(),
            )
            .await
        }
        LaunchDispatch::TuiConfigured { launch_plan } => {
            let config = load_config(launch_plan.env_file.clone(), &runtime_overrides)?;
            if let Some(tunnel_config) = named_tunnel_setup_config(&config) {
                if !interactive_terminal {
                    return named_tunnel_setup_requires_tui(&args, &resolved_env);
                }
                return setup_tui::run(tunnel_config).await;
            }
            if !interactive_terminal {
                return noninteractive_configured_launch_guidance(&args, &resolved_env);
            }
            tui::run_gateway_tui(
                &args.host,
                logging.log_file.clone(),
                GatewayTuiLaunch::Configured { launch_plan },
                setup_defaults(&runtime_overrides),
                runtime_overrides.clone(),
            )
            .await
        }
        LaunchDispatch::Cli { launch_plan } => {
            let config = load_config(launch_plan.env_file.clone(), &runtime_overrides)?;
            if named_tunnel_setup_config(&config).is_some() {
                return named_tunnel_setup_requires_tui(&args, &resolved_env);
            }
            run_cli_mode(&args.host, config, launch_plan, &logging).await
        }
        LaunchDispatch::Setup { env_file } => run_setup_mode(env_file).await,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use brain3_core::domain::setup::{
        DependencyAvailability, DependencyStatus, InstallAction, PackageManager,
        SetupOperatingSystem,
    };

    use super::*;

    #[test]
    fn args_default_to_tui_mode() {
        let args = Args::try_parse_from(["brain3"]).expect("args should parse");
        assert_eq!(choose_launch_mode(&args), LaunchMode::Tui);
    }

    #[test]
    fn args_accept_explicit_tui_and_cli_modes() {
        let tui_args = Args::try_parse_from(["brain3", "--tui"]).expect("tui args should parse");
        assert_eq!(choose_launch_mode(&tui_args), LaunchMode::Tui);

        let cli_args = Args::try_parse_from(["brain3", "--cli"]).expect("cli args should parse");
        assert_eq!(choose_launch_mode(&cli_args), LaunchMode::Cli);
    }

    #[test]
    fn setup_conflicts_with_launch_modes() {
        assert!(Args::try_parse_from(["brain3", "--cf-setup", "--tui"]).is_err());
        assert!(Args::try_parse_from(["brain3", "--cf-setup", "--cli"]).is_err());
    }

    #[test]
    fn old_setup_flag_is_rejected() {
        assert!(Args::try_parse_from(["brain3", "--setup"]).is_err());
    }

    #[test]
    fn parses_container_tag_override() {
        let args = Args::try_parse_from(["brain3", "--container-tag", "pr-123"])
            .expect("container tag args should parse");

        assert_eq!(args.container_tag.as_deref(), Some("pr-123"));
    }

    #[test]
    fn legacy_official_latest_resolves_to_release_tag() {
        assert_eq!(
            resolve_effective_container_image(
                Some("ghcr.io/tleyden/brain3-mcp-vault-tools:latest"),
                None,
            )
            .image,
            release::default_container_image()
        );
    }

    #[test]
    fn explicit_container_tag_latest_wins_over_legacy_remap() {
        assert_eq!(
            resolve_effective_container_image(
                Some("ghcr.io/tleyden/brain3-mcp-vault-tools:latest"),
                Some("latest"),
            )
            .image,
            release::container_image_for_tag("latest")
        );
    }

    #[test]
    fn custom_configured_image_is_left_unchanged() {
        let custom = "ghcr.io/acme/custom-mcp:dev";
        assert_eq!(
            resolve_effective_container_image(Some(custom), None).image,
            custom
        );
    }

    #[test]
    fn launch_dispatch_uses_wizard_only_for_missing_default_env_in_tui_mode() {
        let app_home = Brain3AppHome::from_root(PathBuf::from("/tmp/brain3-home"));
        let default_env = ResolvedEnvFile {
            app_home: app_home.clone(),
            env_file: app_home.env_file.clone(),
            source: EnvFileSource::Default,
        };

        assert_eq!(
            plan_launch(
                LaunchMode::Tui,
                &default_env,
                false,
                PathBuf::from("/tmp/brain3.log"),
            )
            .expect("tui dispatch should succeed"),
            LaunchDispatch::TuiFirstRun
        );

        let err = plan_launch(
            LaunchMode::Cli,
            &default_env,
            false,
            PathBuf::from("/tmp/brain3.log"),
        )
        .expect_err("cli dispatch should refuse missing default env");
        assert!(
            err.to_string().contains("rerun without --cli"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn launch_dispatch_fails_for_missing_custom_env_file() {
        let resolved = ResolvedEnvFile {
            app_home: Brain3AppHome::from_root(PathBuf::from("/tmp/brain3-home")),
            env_file: PathBuf::from("/tmp/custom.env"),
            source: EnvFileSource::Custom,
        };

        let err = plan_launch(
            LaunchMode::Tui,
            &resolved,
            false,
            PathBuf::from("/tmp/brain3.log"),
        )
        .expect_err("missing custom env should fail");

        assert!(
            err.to_string().contains("/tmp/custom.env"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn cli_readiness_requires_installed_runtime_dependencies() {
        let ready = dependency_status(
            DependencyAvailability::Installed,
            DependencyAvailability::Installed,
        );
        assert!(ensure_cli_ready(&ready).is_ok());

        let err = ensure_cli_ready(&dependency_status(
            DependencyAvailability::InstallAvailable(InstallAction::InstallCloudflared),
            DependencyAvailability::Installed,
        ))
        .expect_err("installable dependency should require interactive setup");
        assert!(
            err.to_string().contains("rerun without --cli"),
            "unexpected error: {err:#}"
        );
    }

    fn dependency_status(
        cloudflared: DependencyAvailability,
        preferred_container_runtime: DependencyAvailability,
    ) -> DependencyStatus {
        DependencyStatus {
            operating_system: SetupOperatingSystem::MacOS,
            package_manager: Some(PackageManager::Homebrew),
            cloudflared,
            preferred_container_runtime,
            docker_installed: true,
            macos_container_installed: Some(true),
            homebrew_installed: Some(true),
        }
    }
}
