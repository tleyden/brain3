use std::path::PathBuf;

use brain3_core::domain::setup::{
    ConnectionCard, DependencyAvailability, FinalizeSetupRequest, InstallAction, SetupDraftConfig,
    SetupPreparation, SetupStep, SetupSummary,
};
use brain3_platform::runtime::RuntimeBootstrap;

use crate::server::{GatewayServerHandle, GatewayServerStatus};

use super::runtime_logs::RuntimeLogs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthField {
    Username,
    ClientId,
    Password,
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
    pub dependency_focus: DependencyDoctorFocus,
    pub dependency_action_index: usize,
}

impl FirstRunTuiState {
    pub fn new(host: String, log_file: PathBuf, preparation: SetupPreparation) -> Self {
        let draft = preparation.draft.clone();
        let dependency_focus = if dependency_actions_for(&preparation.dependencies).is_empty() {
            DependencyDoctorFocus::Continue
        } else {
            DependencyDoctorFocus::InstallAction
        };

        Self {
            host,
            runtime_logs: RuntimeLogs::new(log_file.clone()),
            log_file,
            step: SetupStep::Welcome,
            runtime_view: RuntimeView::Status,
            vault_path_input: draft.vault_path.display().to_string(),
            username_input: draft.username.clone(),
            client_id_input: draft.client_id.clone(),
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
            dependency_focus,
            dependency_action_index: 0,
        }
    }

    pub fn new_runtime(
        host: String,
        log_file: PathBuf,
        preparation: SetupPreparation,
        display_url: String,
        runtime: RuntimeBootstrap,
        server: GatewayServerHandle,
    ) -> Self {
        let connection_card = ConnectionCard {
            server_url: display_url,
            client_id: runtime.config.oauth.client_id.clone(),
            client_secret: runtime.config.oauth.client_secret.clone(),
            username: runtime.config.oauth.username.clone(),
            password: runtime.config.oauth.password.clone(),
            log_file: log_file.clone(),
        };
        let mut state = Self::new(host, log_file, preparation);
        state.connection_card = Some(connection_card);
        state.runtime = Some(runtime);
        state.server = Some(server);
        state.step = SetupStep::RuntimeStatus;
        state.info_message = Some("Brain3 is running.".into());
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
            SetupStep::Auth => Some(SetupStep::VaultPath),
            SetupStep::Summary => Some(SetupStep::Auth),
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
