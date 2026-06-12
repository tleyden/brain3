use brain3_core::domain::oauth::AuthorizeRequest;

const LOGIN_STYLESHEET_PATH: &str = "/oauth/login.css";
const LOGIN_LOGO_PATH: &str = "/oauth/brain3-lockup-light.svg";

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn render_page(title: &str, kicker: &str, heading: &str, body_html: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title}</title>
    <link rel="stylesheet" href="{LOGIN_STYLESHEET_PATH}">
  </head>
  <body>
    <main class="page">
      <section class="card">
        <img class="logo" src="{LOGIN_LOGO_PATH}" alt="Brain3">
        <p class="kicker">{kicker}</p>
        <h1>{heading}</h1>
        {body_html}
      </section>
      <footer class="page-footer">
        <a class="footer-link" href="https://github.com/tleyden/brain3" target="_blank" rel="noreferrer noopener">
          <span class="footer-icon" aria-hidden="true">
            <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" fill="currentColor">
              <path d="M12 2C6.48 2 2 6.58 2 12.23C2 16.75 4.87 20.58 8.84 21.93C9.34 22.02 9.52 21.71 9.52 21.45C9.52 21.21 9.51 20.42 9.5 19.58C6.73 20.2 6.14 18.37 6.14 18.37C5.68 17.16 5.03 16.84 5.03 16.84C4.12 16.2 5.1 16.22 5.1 16.22C6.1 16.29 6.63 17.28 6.63 17.28C7.52 18.84 8.97 18.39 9.54 18.12C9.63 17.45 9.89 16.99 10.18 16.73C7.97 16.47 5.65 15.58 5.65 11.62C5.65 10.49 6.04 9.57 6.69 8.86C6.59 8.6 6.24 7.56 6.79 6.15C6.79 6.15 7.63 5.87 9.5 7.18C10.29 6.95 11.15 6.84 12 6.84C12.85 6.84 13.71 6.95 14.5 7.18C16.37 5.87 17.21 6.15 17.21 6.15C17.76 7.56 17.41 8.6 17.31 8.86C17.96 9.57 18.35 10.49 18.35 11.62C18.35 15.59 16.02 16.47 13.8 16.72C14.17 17.05 14.5 17.7 14.5 18.69C14.5 20.11 14.49 21.1 14.49 21.45C14.49 21.71 14.67 22.03 15.18 21.93C19.15 20.58 22 16.75 22 12.23C22 6.58 17.52 2 12 2Z"/>
            </svg>
          </span>
          <span>View Brain3 on GitHub</span>
        </a>
      </footer>
    </main>
  </body>
</html>"#,
        title = html_escape(title),
        kicker = html_escape(kicker),
        heading = html_escape(heading),
        body_html = body_html,
    )
}

pub fn render_login_form(req: &AuthorizeRequest, error: Option<&str>) -> String {
    let hidden_fields = [
        ("response_type", &req.response_type),
        ("client_id", &req.client_id),
        ("redirect_uri", &req.redirect_uri),
    ];

    let mut fields_html = String::new();
    for (name, value) in &hidden_fields {
        fields_html.push_str(&format!(
            r#"<input type="hidden" name="{}" value="{}">"#,
            html_escape(name),
            html_escape(value),
        ));
        fields_html.push('\n');
    }

    if let Some(state) = &req.state {
        fields_html.push_str(&format!(
            r#"<input type="hidden" name="state" value="{}">"#,
            html_escape(state),
        ));
        fields_html.push('\n');
    }

    if let Some(challenge) = &req.code_challenge {
        fields_html.push_str(&format!(
            r#"<input type="hidden" name="code_challenge" value="{}">"#,
            html_escape(challenge),
        ));
        fields_html.push('\n');
    }

    let method = req.code_challenge_method.as_deref().unwrap_or("S256");
    fields_html.push_str(&format!(
        r#"<input type="hidden" name="code_challenge_method" value="{}">"#,
        html_escape(method),
    ));

    let error_html = match error {
        Some(msg) => format!(
            r#"<div class="error" role="alert"><strong>Sign-in failed</strong><p>{}</p></div>"#,
            html_escape(msg)
        ),
        None => String::new(),
    };

    let body_html = format!(
        r##"<div class="meta-badges">
          <div class="security-banner">
            <div class="security-icon" aria-hidden="true">
              <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
                <path d="M12 2L19 5V11C19 15.4 16.2 19.4 12 21C7.8 19.4 5 15.4 5 11V5L12 2Z" fill="currentColor" fill-opacity="0.12"/>
                <path d="M12 2L19 5V11C19 15.4 16.2 19.4 12 21C7.8 19.4 5 15.4 5 11V5L12 2Z" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/>
                <path d="M9.25 11.25C9.25 9.73 10.48 8.5 12 8.5C13.52 8.5 14.75 9.73 14.75 11.25V12.25H15C15.41 12.25 15.75 12.59 15.75 13V15.75C15.75 16.16 15.41 16.5 15 16.5H9C8.59 16.5 8.25 16.16 8.25 15.75V13C8.25 12.59 8.59 12.25 9 12.25H9.25V11.25Z" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>
                <path d="M10.75 12.25V11.25C10.75 10.56 11.31 10 12 10C12.69 10 13.25 10.56 13.25 11.25V12.25" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>
              </svg>
            </div>
            <p class="security-caption">Sign In</p>
          </div>
          <div class="tunnel-badge" aria-label="Cloudflare Tunnel enabled by default">
            <span class="tunnel-icon" aria-hidden="true">
              <svg viewBox="0 0 48 32" xmlns="http://www.w3.org/2000/svg">
                <path d="M18.9 25.7H40.4C43.5 25.7 46 23.2 46 20.1C46 17.2 43.8 14.8 41 14.5C40.5 10 36.7 6.5 32.1 6.5C28.3 6.5 24.9 8.9 23.6 12.3C22.8 11.9 21.9 11.7 21 11.7C17.9 11.7 15.3 14 14.8 17H14.5C11.2 17 8.5 19.7 8.5 23C8.5 24 8.8 24.9 9.2 25.7H18.9Z" fill="#F38020"/>
                <path d="M12.5 26.7H30.6C33.1 26.7 35.1 24.7 35.1 22.2C35.1 19.9 33.4 18 31.2 17.7C30.8 14.1 27.8 11.3 24.1 11.3C21.1 11.3 18.4 13.2 17.4 15.9C16.8 15.6 16.1 15.4 15.4 15.4C12.9 15.4 10.8 17.3 10.4 19.7H10.2C7.5 19.7 5.4 21.8 5.4 24.5C5.4 25.3 5.6 26 6 26.7H12.5Z" fill="#FBB040"/>
                <path d="M31.1 24.8H39.7C41.4 24.8 42.8 23.4 42.8 21.7C42.8 20.2 41.7 18.9 40.2 18.7C39.9 16.1 37.8 14.1 35.1 14.1C32.9 14.1 31 15.5 30.2 17.5C29.8 17.3 29.3 17.2 28.8 17.2C27 17.2 25.6 18.5 25.3 20.2H25.2C23.3 20.2 21.8 21.7 21.8 23.6C21.8 24 21.9 24.4 22.1 24.8H31.1Z" fill="#F38020"/>
              </svg>
            </span>
            <span class="tunnel-text"><span class="tunnel-label">Cloudflare Tunnel</span><span class="tunnel-subtitle">default route</span></span>
          </div>
        </div>
        <p class="lead">ChatGPT, Claude, or another AI app is requesting access to your local Brain3 MCP gateway.</p>
        <div class="callout">
          <h2>Where to find your credentials</h2>
          <div class="credential-options">
            <div class="credential-option">
              <p>See your app TUI (Terminal UI) to get these values under the MCP <code>[c]</code> MCP config settings.</p>
            </div>
            <div class="credential-divider" aria-hidden="true">
              <span class="credential-divider-line"></span>
              <span class="credential-divider-label">OR</span>
              <span class="credential-divider-line"></span>
            </div>
            <div class="credential-option">
              <p>You can also use the <code>B3_USERNAME</code> and <code>B3_PASSWORD</code> values from the <code>.env</code> file configured on this machine, which are the same.</p>
            </div>
          </div>
        </div>
        {error_html}
        <form class="form" method="post" action="/oauth/authorize">
          {fields_html}
          <div class="field">
            <label for="username">Username</label>
            <input id="username" name="username" type="text" autocomplete="username" required autofocus>
          </div>
          <div class="field">
            <label for="password">Password</label>
            <input id="password" name="password" type="password" autocomplete="current-password" required>
            <p class="help-text">Enter the Brain3 gateway credentials shown in your TUI or <code>.env</code> file.</p>
          </div>
          <div class="actions">
            <button type="submit">Continue</button>
          </div>
        </form>"##,
        error_html = error_html,
        fields_html = fields_html,
    );

    render_page(
        "Sign-in to your Brain3 Local Gateway",
        "Brain3 Local Gateway",
        "Sign-in to your Brain3 Local Gateway",
        &body_html,
    )
}

pub fn render_misconfigured_page() -> String {
    let body_html = r#"<p class="lead">This gateway requires a login before ChatGPT, Claude, or another AI app can finish connecting.</p>
        <div class="callout">
          <h2>What to do next</h2>
          <p>Set <code>B3_USERNAME</code> and <code>B3_PASSWORD</code> in your app TUI (Terminal UI) under MCP <code>[c]</code> MCP config settings, or edit the <code>.env</code> file directly.</p>
          <p>After updating the credentials, restart the gateway and try connecting again.</p>
        </div>
        <ul class="status-list">
          <li>This page is shown because the local Brain3 gateway does not currently have login credentials configured.</li>
          <li>The TUI and <code>.env</code> should show the same credential values once configured.</li>
        </ul>"#;

    render_page(
        "Login credentials not configured",
        "Brain3 Local Gateway",
        "Login credentials not configured",
        body_html,
    )
}
