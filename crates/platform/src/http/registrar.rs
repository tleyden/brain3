use std::borrow::Cow;

use oxide_auth::primitives::registrar::{BoundClient, ClientUrl, PreGrant, Registrar, RegistrarError};
use oxide_auth::primitives::scope::Scope;

/// Custom registrar that validates client_id but accepts any redirect_uri.
///
/// ClientMap requires registering exact redirect URIs, which breaks multi-client
/// support (ChatGPT and Claude use different redirect URIs at runtime). This
/// registrar accepts any redirect_uri as long as the client_id matches.
///
/// check() is intentionally unimplemented (returns Err) because this registrar
/// is only used in AuthorizationFlow, which never calls check(). The token flow
/// uses a separate ClientMap for credential validation.
pub struct BrainRegistrar {
    client_id: String,
}

impl BrainRegistrar {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
        }
    }
}

impl Registrar for BrainRegistrar {
    fn bound_redirect<'a>(&self, bound: ClientUrl<'a>) -> Result<BoundClient<'a>, RegistrarError> {
        if bound.client_id.as_ref() != self.client_id {
            tracing::debug!(
                submitted = %bound.client_id,
                expected = %self.client_id,
                "BrainRegistrar: client_id mismatch in bound_redirect"
            );
            return Err(RegistrarError::Unspecified);
        }
        let redirect_uri = bound.redirect_uri.ok_or(RegistrarError::Unspecified)?;
        Ok(BoundClient {
            client_id: bound.client_id,
            redirect_uri: Cow::Owned(redirect_uri.into_owned().into()),
        })
    }

    fn negotiate<'a>(
        &self,
        bound: BoundClient<'a>,
        _scope: Option<Scope>,
    ) -> Result<PreGrant, RegistrarError> {
        Ok(PreGrant {
            client_id: bound.client_id.into_owned(),
            redirect_uri: bound.redirect_uri.into_owned(),
            scope: "read".parse().unwrap_or_else(|_| "".parse().unwrap()),
        })
    }

    fn check(&self, _client_id: &str, _passphrase: Option<&[u8]>) -> Result<(), RegistrarError> {
        // Never called in AuthorizationFlow; token flow uses ClientMap.
        Err(RegistrarError::Unspecified)
    }
}
