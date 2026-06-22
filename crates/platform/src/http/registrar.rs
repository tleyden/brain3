use std::borrow::Cow;

use oxide_auth::primitives::registrar::{BoundClient, ClientUrl, PreGrant, Registrar, RegistrarError};
use oxide_auth::primitives::scope::Scope;
use subtle::ConstantTimeEq;

/// Gateway registrar for the single preregistered confidential client.
///
/// The gateway intentionally supports one configured client id + secret pair,
/// while allowing runtime redirect URIs from AI clients such as ChatGPT and
/// Claude. The redirect URI is bound into the authorization grant during the
/// authorize step and compared again during code exchange by oxide-auth.
pub struct GatewayRegistrar {
    client_id: String,
    client_secret: Vec<u8>,
    default_scope: Scope,
}

impl GatewayRegistrar {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<Vec<u8>>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            default_scope: "read"
                .parse()
                .expect("static gateway scope should parse"),
        }
    }
}

impl Registrar for GatewayRegistrar {
    fn bound_redirect<'a>(&self, bound: ClientUrl<'a>) -> Result<BoundClient<'a>, RegistrarError> {
        if bound.client_id.as_ref() != self.client_id {
            tracing::debug!(
                submitted = %bound.client_id,
                expected = %self.client_id,
                "GatewayRegistrar: client_id mismatch in bound_redirect"
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
            scope: self.default_scope.clone(),
        })
    }

    fn check(&self, client_id: &str, passphrase: Option<&[u8]>) -> Result<(), RegistrarError> {
        let client_id_matches = client_id
            .as_bytes()
            .ct_eq(self.client_id.as_bytes())
            .into();
        let provided_secret = passphrase.ok_or(RegistrarError::Unspecified)?;
        let secret_matches = provided_secret.ct_eq(self.client_secret.as_slice()).into();

        if client_id_matches && secret_matches {
            Ok(())
        } else {
            tracing::debug!(
                submitted = %client_id,
                expected = %self.client_id,
                "GatewayRegistrar: client credential validation failed"
            );
            Err(RegistrarError::Unspecified)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use oxide_auth::primitives::registrar::{ClientUrl, ExactUrl};

    use super::*;

    #[test]
    fn registrar_accepts_configured_client_and_round_trips_redirect_uri() {
        let registrar = GatewayRegistrar::new("brain3-oauth2-client", b"hardcoded-secret".to_vec());
        let redirect_uri = "https://chatgpt.com/connector/oauth/test"
            .parse::<ExactUrl>()
            .expect("redirect URI should parse");

        let bound = registrar
            .bound_redirect(ClientUrl {
                client_id: Cow::Borrowed("brain3-oauth2-client"),
                redirect_uri: Some(Cow::Owned(redirect_uri.clone())),
            })
            .expect("configured client and redirect URI should bind");

        assert_eq!(bound.client_id.as_ref(), "brain3-oauth2-client");
        assert_eq!(bound.redirect_uri.to_url(), redirect_uri.to_url());
    }

    #[test]
    fn registrar_rejects_client_secret_checks_today() {
        let registrar = GatewayRegistrar::new("brain3-oauth2-client", b"hardcoded-secret".to_vec());

        let result = registrar.check("brain3-oauth2-client", Some(b"hardcoded-secret"));

        assert!(result.is_ok(), "registrar should validate configured client secret");
    }
}
