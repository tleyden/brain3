use std::time::Duration;

#[derive(Debug)]
pub enum HttpProbeOutcome {
    ValidAuthChallenge,
    ConnectionFailed(String),
    Timeout,
    WrongStatus { status: u16, body: String },
    WrongBody { status: u16, body: String },
}

impl HttpProbeOutcome {
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::ValidAuthChallenge)
    }

    pub fn summary(&self) -> String {
        match self {
            Self::ValidAuthChallenge => "OK (401 invalid_token)".into(),
            Self::ConnectionFailed(e) => format!("connection failed: {e}"),
            Self::Timeout => "no response within 8s".into(),
            Self::WrongStatus { status, body } => {
                format!("unexpected HTTP {status}: {}", truncate(body, 200))
            }
            Self::WrongBody { status, body } => {
                format!("HTTP {status} but unexpected body: {}", truncate(body, 200))
            }
        }
    }
}

pub async fn probe_tunnel_url(url: &str) -> HttpProbeOutcome {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => return HttpProbeOutcome::ConnectionFailed(e.to_string()),
    };

    let response = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            if e.is_timeout() {
                return HttpProbeOutcome::Timeout;
            }
            return HttpProbeOutcome::ConnectionFailed(e.to_string());
        }
    };

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    if status == 401 {
        if body.contains("invalid_token") {
            HttpProbeOutcome::ValidAuthChallenge
        } else {
            HttpProbeOutcome::WrongBody { status, body }
        }
    } else {
        HttpProbeOutcome::WrongStatus { status, body }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        None => s,
        Some((i, _)) => &s[..i],
    }
}
