use std::path::PathBuf;

use brain3_core::domain::setup::{
    ConnectionCard, DependencyAvailability, FinalizeSetupRequest, InstallAction, SetupDraftConfig,
    SetupPreparation, SetupStep, SetupSummary,
};
use brain3_platform::runtime::RuntimeBootstrap;

use crate::server::{GatewayServerHandle, GatewayServerStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthField {
    Username,
    ClientId,
    Password,
}

pub struct FirstRunTuiState {
    pub host: String,
    pub log_file: PathBuf,
    pub step: SetupStep,
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
}

impl FirstRunTuiState {
    pub fn new(host: String, log_file: PathBuf, preparation: SetupPreparation) -> Self {
        let draft = preparation.draft.clone();
        Self {
            host,
            log_file,
            step: SetupStep::Welcome,
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
        }
    }

    pub fn new_runtime(
        host: String,
        log_file: PathBuf,
        preparation: SetupPreparation,
        runtime: RuntimeBootstrap,
        server: GatewayServerHandle,
    ) -> Self {
        let mut state = Self::new(host, log_file, preparation);
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
        let mut actions = Vec::new();

        if let DependencyAvailability::InstallAvailable(action) =
            self.preparation.dependencies.cloudflared
        {
            actions.push(action);
        }

        if let DependencyAvailability::InstallAvailable(action) =
            self.preparation.dependencies.preferred_container_runtime
        {
            if !actions.contains(&action) {
                actions.push(action);
            }
        }

        actions
    }

    pub fn set_dependencies(&mut self, dependencies: brain3_core::domain::setup::DependencyStatus) {
        self.preparation.dependencies = dependencies;
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
}

pub fn install_action_label(action: InstallAction) -> &'static str {
    match action {
        InstallAction::InstallCloudflared => "Install cloudflared",
        InstallAction::InstallDocker => "Install Docker",
        InstallAction::InstallMacOSContainer => "Install macOS container runtime",
    }
}
