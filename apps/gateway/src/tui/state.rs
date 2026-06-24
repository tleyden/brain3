use std::path::PathBuf;

use brain3_core::domain::setup::{
    AccessModeDraft, ConnectionCard, DependencyAvailability, FinalizeSetupRequest, InstallAction,
    SetupDraftConfig, SetupPreparation, SetupStep, SetupSummary,
};
use tokio::sync::oneshot;

use brain3_platform::runtime::RuntimeBootstrap;

use crate::server::{ConfiguredGatewaySession, GatewayServerHandle, GatewayServerStatus};

use super::runtime_logs::RuntimeLogs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthField {
    Username,
    ClientId,
    Password,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessModeField {
    LocalOnly,
    RemoteOnly,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortsField {
    GatewayPort,
    ContainerHostPort,
    ContainerMcpPort,
    AccessTokenLifetimeSecs,
    RefreshTokenLifetimeSecs,
    PkceRequired,
    EnforceHostnameCheck,
    ContainerNetworkIsolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyDoctorFocus {
    InstallAction,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeView {
    Status,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryField {
    VaultPath,
    Username,
    ClientId,
    PasswordMode,
    PasswordValue,
    GatewayPort,
    ContainerHostPort,
    ContainerMcpPort,
    AccessTokenLifetimeSecs,
    RefreshTokenLifetimeSecs,
    PkceRequired,
    HostnameCheck,
    ContainerNetworkIsolation,
}

pub struct FirstRunTuiState {
    pub host: String,
    pub log_file: PathBuf,
    pub step: SetupStep,
    pub runtime_view: RuntimeView,
    pub runtime_logs: RuntimeLogs,
    pub preparation: SetupPreparation,
    pub draft: SetupDraftConfig,
    pub generate_password: bool,
    pub summary: Option<SetupSummary>,
    pub connection_card: Option<ConnectionCard>,
    pub runtime: Option<RuntimeBootstrap>,
    pub server: Option<GatewayServerHandle>,
    pub error_message: Option<String>,
    pub info_message: Option<String>,
    pub vault_path_input: String,
    pub username_input: String,
    pub client_id_input: String,
    pub password_input: String,
    pub auth_focus: AuthField,
    pub access_mode_focus: AccessModeField,
    pub access_mode_locked: bool,
    pub ports_focus: PortsField,
    pub gateway_port_input: String,
    pub container_host_port_input: String,
    pub container_mcp_port_input: String,
    pub access_token_lifetime_secs_input: String,
    pub refresh_token_lifetime_secs_input: String,
    pub dependency_focus: DependencyDoctorFocus,
    pub dependency_action_index: usize,
    pub summary_focus: SummaryField,
    pub startup_rx: Option<oneshot::Receiver<anyhow::Result<ConfiguredGatewaySession>>>,
    pub probe_rx: Option<oneshot::Receiver<Result<(), String>>>,
    pub tick_count: u64,
}

impl FirstRunTuiState {
    pub fn new(host: String, log_file: PathBuf, preparation: SetupPreparation) -> Self {
        let draft = preparation.draft.clone();
        let dependency_focus = if dependency_actions_for(&preparation.dependencies).is_empty() {
            DependencyDoctorFocus::Continue
        } else {
            DependencyDoctorFocus::InstallAction
        };

        let vault_path_input = draft.vault_path.display().to_string();
        let username_input = draft.username.clone();
        let client_id_input = draft.client_id.clone();
        let gateway_port_input = draft.gateway_port.to_string();
        let container_host_port_input = draft.container_host_port.to_string();
        let container_mcp_port_input = draft.container_mcp_port.to_string();
        let access_token_lifetime_secs_input = draft.access_token_lifetime_secs.to_string();
        let refresh_token_lifetime_secs_input = draft.refresh_token_lifetime_secs.to_string();

        Self {
            host,
            runtime_logs: RuntimeLogs::new(log_file.clone()),
            log_file,
            step: SetupStep::Welcome,
            runtime_view: RuntimeView::Status,
            vault_path_input,
            username_input,
            client_id_input,
            password_input: String::new(),
            draft,
            preparation,
            generate_password: true,
            summary: None,
            connection_card: None,
            runtime: None,
            server: None,
            error_message: None,
            info_message: None,
            auth_focus: AuthField::Username,
            access_mode_focus: AccessModeField::Both,
            access_mode_locked: false,
            ports_focus: PortsField::GatewayPort,
            gateway_port_input,
            container_host_port_input,
            container_mcp_port_input,
            access_token_lifetime_secs_input,
            refresh_token_lifetime_secs_input,
            dependency_focus,
            dependency_action_index: 0,
            summary_focus: SummaryField::VaultPath,
            startup_rx: None,
            probe_rx: None,
            tick_count: 0,
        }
    }

    pub fn new_starting(
        host: String,
        log_file: PathBuf,
        preparation: SetupPreparation,
        startup_rx: oneshot::Receiver<anyhow::Result<ConfiguredGatewaySession>>,
    ) -> Self {
        let mut state = Self::new(host, log_file, preparation);
        state.step = SetupStep::RuntimeStatus;
        state.startup_rx = Some(startup_rx);
        state.info_message = Some("Starting Brain3...".into());
        state
    }

    pub fn clear_messages(&mut self) {
        self.error_message = None;
        self.info_message = None;
    }

    pub fn apply_inputs_to_draft(&mut self) -> FinalizeSetupRequest {
        self.draft.vault_path = PathBuf::from(self.vault_path_input.trim());
        self.draft.username = self.username_input.trim().to_string();
        self.draft.client_id = self.client_id_input.trim().to_string();
        self.draft.password = if self.generate_password {
            String::new()
        } else {
            self.password_input.clone()
        };

        if let Ok(port) = self.gateway_port_input.trim().parse::<u16>() {
            self.draft.gateway_port = port;
        }
        if let Ok(port) = self.container_host_port_input.trim().parse::<u16>() {
            self.draft.container_host_port = port;
        }
        if let Ok(port) = self.container_mcp_port_input.trim().parse::<u16>() {
            self.draft.container_mcp_port = port;
        }
        if let Ok(seconds) = self.access_token_lifetime_secs_input.trim().parse::<u64>() {
            self.draft.access_token_lifetime_secs = seconds;
        }
        if let Ok(seconds) = self.refresh_token_lifetime_secs_input.trim().parse::<u64>() {
            self.draft.refresh_token_lifetime_secs = seconds;
        }

        FinalizeSetupRequest {
            draft: self.draft.clone(),
            generate_password: self.generate_password,
        }
    }

    pub fn dependency_actions(&self) -> Vec<InstallAction> {
        dependency_actions_for(&self.preparation.dependencies)
    }

    pub fn set_dependencies(&mut self, dependencies: brain3_core::domain::setup::DependencyStatus) {
        self.preparation.dependencies = dependencies;
        self.sync_dependency_focus();
    }

    pub fn next_auth_focus(&mut self) {
        self.auth_focus = match self.auth_focus {
            AuthField::Username => AuthField::ClientId,
            AuthField::ClientId => {
                if self.generate_password {
                    AuthField::Username
                } else {
                    AuthField::Password
                }
            }
            AuthField::Password => AuthField::Username,
        };
    }

    pub fn previous_auth_focus(&mut self) {
        self.auth_focus = match self.auth_focus {
            AuthField::Username => {
                if self.generate_password {
                    AuthField::ClientId
                } else {
                    AuthField::Password
                }
            }
            AuthField::ClientId => AuthField::Username,
            AuthField::Password => AuthField::ClientId,
        };
    }

    pub fn next_access_mode_focus(&mut self) {
        self.access_mode_focus = match self.access_mode_focus {
            AccessModeField::LocalOnly => AccessModeField::RemoteOnly,
            AccessModeField::RemoteOnly => AccessModeField::Both,
            AccessModeField::Both => AccessModeField::LocalOnly,
        };
        self.draft.access_mode = access_mode_for_focus(self.access_mode_focus);
    }

    pub fn previous_access_mode_focus(&mut self) {
        self.access_mode_focus = match self.access_mode_focus {
            AccessModeField::LocalOnly => AccessModeField::Both,
            AccessModeField::RemoteOnly => AccessModeField::LocalOnly,
            AccessModeField::Both => AccessModeField::RemoteOnly,
        };
        self.draft.access_mode = access_mode_for_focus(self.access_mode_focus);
    }

    pub fn confirm_access_mode(&mut self) {
        self.access_mode_locked = true;
    }

    pub fn next_ports_focus(&mut self, access_mode: &AccessModeDraft) {
        let order = ports_focus_order(access_mode);
        let current_index = order
            .iter()
            .position(|field| *field == self.ports_focus)
            .unwrap_or(0);
        self.ports_focus = order[(current_index + 1) % order.len()];
    }

    pub fn previous_ports_focus(&mut self, access_mode: &AccessModeDraft) {
        let order = ports_focus_order(access_mode);
        let current_index = order
            .iter()
            .position(|field| *field == self.ports_focus)
            .unwrap_or(0);
        self.ports_focus = if current_index == 0 {
            *order.last().expect("ports order should not be empty")
        } else {
            order[current_index - 1]
        };
    }

    pub fn toggle_ports_boolean(&mut self) {
        match self.ports_focus {
            PortsField::PkceRequired => {
                self.draft.pkce_required = !self.draft.pkce_required;
            }
            PortsField::EnforceHostnameCheck => {
                self.draft.enforce_hostname_check = !self.draft.enforce_hostname_check;
            }
            PortsField::ContainerNetworkIsolation => {
                self.draft.container_network_isolated = !self.draft.container_network_isolated;
            }
            _ => {}
        }
    }

    pub fn ports_focus_is_text_field(&self) -> bool {
        matches!(
            self.ports_focus,
            PortsField::GatewayPort
                | PortsField::ContainerHostPort
                | PortsField::ContainerMcpPort
                | PortsField::AccessTokenLifetimeSecs
                | PortsField::RefreshTokenLifetimeSecs
        )
    }

    pub fn next_summary_focus(&mut self) {
        self.summary_focus = match self.summary_focus {
            SummaryField::VaultPath => SummaryField::Username,
            SummaryField::Username => SummaryField::ClientId,
            SummaryField::ClientId => SummaryField::PasswordMode,
            SummaryField::PasswordMode => {
                if self.generate_password {
                    SummaryField::GatewayPort
                } else {
                    SummaryField::PasswordValue
                }
            }
            SummaryField::PasswordValue => SummaryField::GatewayPort,
            SummaryField::GatewayPort => SummaryField::ContainerHostPort,
            SummaryField::ContainerHostPort => SummaryField::ContainerMcpPort,
            SummaryField::ContainerMcpPort => SummaryField::AccessTokenLifetimeSecs,
            SummaryField::AccessTokenLifetimeSecs => SummaryField::RefreshTokenLifetimeSecs,
            SummaryField::RefreshTokenLifetimeSecs => SummaryField::PkceRequired,
            SummaryField::PkceRequired => SummaryField::HostnameCheck,
            SummaryField::HostnameCheck => SummaryField::ContainerNetworkIsolation,
            SummaryField::ContainerNetworkIsolation => SummaryField::VaultPath,
        };
    }

    pub fn previous_summary_focus(&mut self) {
        self.summary_focus = match self.summary_focus {
            SummaryField::VaultPath => SummaryField::ContainerNetworkIsolation,
            SummaryField::Username => SummaryField::VaultPath,
            SummaryField::ClientId => SummaryField::Username,
            SummaryField::PasswordMode => SummaryField::ClientId,
            SummaryField::PasswordValue => SummaryField::PasswordMode,
            SummaryField::GatewayPort => {
                if self.generate_password {
                    SummaryField::PasswordMode
                } else {
                    SummaryField::PasswordValue
                }
            }
            SummaryField::ContainerHostPort => SummaryField::GatewayPort,
            SummaryField::ContainerMcpPort => SummaryField::ContainerHostPort,
            SummaryField::AccessTokenLifetimeSecs => SummaryField::ContainerMcpPort,
            SummaryField::RefreshTokenLifetimeSecs => SummaryField::AccessTokenLifetimeSecs,
            SummaryField::PkceRequired => SummaryField::RefreshTokenLifetimeSecs,
            SummaryField::HostnameCheck => SummaryField::PkceRequired,
            SummaryField::ContainerNetworkIsolation => SummaryField::HostnameCheck,
        };
    }

    pub fn summary_focus_is_text_field(&self) -> bool {
        matches!(
            self.summary_focus,
            SummaryField::VaultPath
                | SummaryField::Username
                | SummaryField::ClientId
                | SummaryField::PasswordValue
                | SummaryField::GatewayPort
                | SummaryField::ContainerHostPort
                | SummaryField::ContainerMcpPort
                | SummaryField::AccessTokenLifetimeSecs
                | SummaryField::RefreshTokenLifetimeSecs
        )
    }

    pub fn summary_focus_is_digits_only(&self) -> bool {
        matches!(
            self.summary_focus,
            SummaryField::GatewayPort
                | SummaryField::ContainerHostPort
                | SummaryField::ContainerMcpPort
                | SummaryField::AccessTokenLifetimeSecs
                | SummaryField::RefreshTokenLifetimeSecs
        )
    }

    pub fn summary_char_push(&mut self, ch: char) {
        match self.summary_focus {
            SummaryField::VaultPath => self.vault_path_input.push(ch),
            SummaryField::Username => self.username_input.push(ch),
            SummaryField::ClientId => self.client_id_input.push(ch),
            SummaryField::PasswordValue => self.password_input.push(ch),
            SummaryField::GatewayPort => self.gateway_port_input.push(ch),
            SummaryField::ContainerHostPort => self.container_host_port_input.push(ch),
            SummaryField::ContainerMcpPort => self.container_mcp_port_input.push(ch),
            SummaryField::AccessTokenLifetimeSecs => self.access_token_lifetime_secs_input.push(ch),
            SummaryField::RefreshTokenLifetimeSecs => {
                self.refresh_token_lifetime_secs_input.push(ch)
            }
            _ => {}
        }
    }

    pub fn summary_char_pop(&mut self) {
        match self.summary_focus {
            SummaryField::VaultPath => {
                self.vault_path_input.pop();
            }
            SummaryField::Username => {
                self.username_input.pop();
            }
            SummaryField::ClientId => {
                self.client_id_input.pop();
            }
            SummaryField::PasswordValue => {
                self.password_input.pop();
            }
            SummaryField::GatewayPort => {
                self.gateway_port_input.pop();
            }
            SummaryField::ContainerHostPort => {
                self.container_host_port_input.pop();
            }
            SummaryField::ContainerMcpPort => {
                self.container_mcp_port_input.pop();
            }
            SummaryField::AccessTokenLifetimeSecs => {
                self.access_token_lifetime_secs_input.pop();
            }
            SummaryField::RefreshTokenLifetimeSecs => {
                self.refresh_token_lifetime_secs_input.pop();
            }
            _ => {}
        }
    }

    pub fn toggle_summary_field(&mut self) {
        match self.summary_focus {
            SummaryField::PasswordMode => {
                self.generate_password = !self.generate_password;
                if self.generate_password && self.summary_focus == SummaryField::PasswordValue {
                    self.summary_focus = SummaryField::GatewayPort;
                }
            }
            SummaryField::PkceRequired => {
                self.draft.pkce_required = !self.draft.pkce_required;
            }
            SummaryField::HostnameCheck => {
                self.draft.enforce_hostname_check = !self.draft.enforce_hostname_check;
            }
            SummaryField::ContainerNetworkIsolation => {
                self.draft.container_network_isolated = !self.draft.container_network_isolated;
            }
            _ => {}
        }
    }

    pub fn toggle_dependency_focus(&mut self) {
        if self.dependency_actions().is_empty() {
            self.dependency_focus = DependencyDoctorFocus::Continue;
            return;
        }

        self.dependency_focus = match self.dependency_focus {
            DependencyDoctorFocus::InstallAction => DependencyDoctorFocus::Continue,
            DependencyDoctorFocus::Continue => DependencyDoctorFocus::InstallAction,
        };
    }

    pub fn next_dependency_action(&mut self) {
        let action_count = self.dependency_actions().len();
        if action_count == 0 {
            self.dependency_focus = DependencyDoctorFocus::Continue;
            return;
        }

        self.dependency_focus = DependencyDoctorFocus::InstallAction;
        self.dependency_action_index = (self.dependency_action_index + 1) % action_count;
    }

    pub fn previous_dependency_action(&mut self) {
        let action_count = self.dependency_actions().len();
        if action_count == 0 {
            self.dependency_focus = DependencyDoctorFocus::Continue;
            return;
        }

        self.dependency_focus = DependencyDoctorFocus::InstallAction;
        self.dependency_action_index = if self.dependency_action_index == 0 {
            action_count - 1
        } else {
            self.dependency_action_index - 1
        };
    }

    pub fn selected_dependency_action_index(&self) -> Option<usize> {
        let action_count = self.dependency_actions().len();
        if action_count == 0 {
            None
        } else {
            Some(self.dependency_action_index.min(action_count - 1))
        }
    }

    pub fn selected_dependency_action(&self) -> Option<InstallAction> {
        self.selected_dependency_action_index()
            .and_then(|index| self.dependency_actions().get(index).copied())
    }

    pub fn previous_step(&self) -> Option<SetupStep> {
        match self.step {
            SetupStep::Welcome => None,
            SetupStep::DependencyDoctor => Some(SetupStep::Welcome),
            SetupStep::VaultPath => Some(SetupStep::DependencyDoctor),
            SetupStep::AccessMode => Some(SetupStep::VaultPath),
            SetupStep::Auth => Some(SetupStep::VaultPath),
            SetupStep::PortsAndSettings => Some(match self.draft.access_mode {
                AccessModeDraft::LocalOnly => SetupStep::VaultPath,
                AccessModeDraft::RemoteOnly | AccessModeDraft::Both => SetupStep::Auth,
            }),
            SetupStep::Summary => Some(SetupStep::PortsAndSettings),
            SetupStep::ConnectionCard => None,
            SetupStep::RuntimeStatus => self
                .connection_card
                .as_ref()
                .map(|_| SetupStep::ConnectionCard),
        }
    }

    pub fn gateway_status(&self) -> GatewayServerStatus {
        match &self.server {
            Some(server) => server.status(),
            None => GatewayServerStatus::NotStarted,
        }
    }

    pub fn toggle_runtime_view(&mut self) {
        match self.runtime_view {
            RuntimeView::Status => self.show_runtime_logs(),
            RuntimeView::Logs => self.show_runtime_status(),
        }
    }

    pub fn show_runtime_logs(&mut self) {
        self.refresh_runtime_logs();
        self.runtime_logs.jump_to_end();
        self.runtime_view = RuntimeView::Logs;
    }

    pub fn show_runtime_status(&mut self) {
        self.runtime_view = RuntimeView::Status;
    }

    pub fn refresh_runtime_logs(&mut self) {
        self.runtime_logs.refresh();
    }

    pub fn scroll_logs_up(&mut self, lines: usize) {
        self.runtime_logs.scroll_up(lines);
    }

    pub fn scroll_logs_down(&mut self, lines: usize) {
        self.runtime_logs.scroll_down(lines);
    }

    pub fn jump_logs_to_end(&mut self) {
        self.runtime_logs.jump_to_end();
    }

    fn sync_dependency_focus(&mut self) {
        let action_count = self.dependency_actions().len();
        if action_count == 0 {
            self.dependency_focus = DependencyDoctorFocus::Continue;
            self.dependency_action_index = 0;
            return;
        }

        self.dependency_action_index = self.dependency_action_index.min(action_count - 1);
    }
}

pub fn validate_port_input(input: &str, label: &str) -> Result<u16, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    match trimmed.parse::<u16>() {
        Ok(0) => Err(format!("{label} must be greater than 0")),
        Ok(port) => Ok(port),
        Err(_) => Err(format!("{label} must be a valid port number (1-65535)")),
    }
}

pub fn validate_positive_u64_input(input: &str, label: &str) -> Result<u64, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    match trimmed.parse::<u64>() {
        Ok(0) => Err(format!("{label} must be greater than 0")),
        Ok(value) => Ok(value),
        Err(_) => Err(format!("{label} must be a valid positive integer")),
    }
}

pub fn install_action_label(action: InstallAction) -> &'static str {
    match action {
        InstallAction::InstallCloudflared => "Install cloudflared",
        InstallAction::InstallDocker => "Install Docker",
        InstallAction::InstallMacOSContainer => "Install macOS container runtime",
    }
}

fn dependency_actions_for(
    dependencies: &brain3_core::domain::setup::DependencyStatus,
) -> Vec<InstallAction> {
    let mut actions = Vec::new();

    if let DependencyAvailability::InstallAvailable(action) = dependencies.cloudflared {
        actions.push(action);
    }

    if let DependencyAvailability::InstallAvailable(action) =
        dependencies.preferred_container_runtime
    {
        if !actions.contains(&action) {
            actions.push(action);
        }
    }

    actions
}

fn access_mode_for_focus(focus: AccessModeField) -> AccessModeDraft {
    match focus {
        AccessModeField::LocalOnly => AccessModeDraft::LocalOnly,
        AccessModeField::RemoteOnly => AccessModeDraft::RemoteOnly,
        AccessModeField::Both => AccessModeDraft::Both,
    }
}

fn ports_focus_order(access_mode: &AccessModeDraft) -> &'static [PortsField] {
    const LOCAL_ONLY_ORDER: &[PortsField] = &[
        PortsField::GatewayPort,
        PortsField::ContainerHostPort,
        PortsField::ContainerMcpPort,
        PortsField::ContainerNetworkIsolation,
    ];
    const REMOTE_ORDER: &[PortsField] = &[
        PortsField::GatewayPort,
        PortsField::ContainerHostPort,
        PortsField::ContainerMcpPort,
        PortsField::AccessTokenLifetimeSecs,
        PortsField::RefreshTokenLifetimeSecs,
        PortsField::PkceRequired,
        PortsField::EnforceHostnameCheck,
        PortsField::ContainerNetworkIsolation,
    ];

    match access_mode {
        AccessModeDraft::LocalOnly => LOCAL_ONLY_ORDER,
        AccessModeDraft::RemoteOnly | AccessModeDraft::Both => REMOTE_ORDER,
    }
}
