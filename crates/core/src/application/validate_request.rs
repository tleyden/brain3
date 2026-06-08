use crate::domain::errors::ProxyError;
use crate::domain::oauth::constant_time_eq;

pub fn validate_bearer_token(auth_header: &str, expected_token: &str) -> Result<(), ProxyError> {
    let (scheme, token) = auth_header.split_once(' ').unwrap_or(("", ""));
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(ProxyError::Unauthorized(
            "Missing or invalid bearer token".into(),
        ));
    }
    if expected_token.is_empty()
        || !constant_time_eq(token.as_bytes(), expected_token.as_bytes())
    {
        return Err(ProxyError::Unauthorized(
            "Missing or invalid bearer token".into(),
        ));
    }
    Ok(())
}

pub fn validate_host(
    request_host: &str,
    expected_host: Option<&str>,
    enforce: bool,
) -> Result<(), ProxyError> {
    if !enforce {
        return Ok(());
    }
    let Some(expected) = expected_host else {
        return Ok(());
    };
    if request_host == expected {
        return Ok(());
    }
    Err(ProxyError::MisdirectedRequest(
        "Request host does not match the configured public hostname".into(),
    ))
}
