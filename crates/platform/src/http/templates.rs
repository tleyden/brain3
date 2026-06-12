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
        r#"<div class="security-banner">
          <div class="security-icon" aria-hidden="true">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
              <path d="M12 2L19 5V11C19 15.4 16.2 19.4 12 21C7.8 19.4 5 15.4 5 11V5L12 2Z" fill="currentColor" fill-opacity="0.12"/>
              <path d="M12 2L19 5V11C19 15.4 16.2 19.4 12 21C7.8 19.4 5 15.4 5 11V5L12 2Z" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/>
              <path d="M9.25 11.25C9.25 9.73 10.48 8.5 12 8.5C13.52 8.5 14.75 9.73 14.75 11.25V12.25H15C15.41 12.25 15.75 12.59 15.75 13V15.75C15.75 16.16 15.41 16.5 15 16.5H9C8.59 16.5 8.25 16.16 8.25 15.75V13C8.25 12.59 8.59 12.25 9 12.25H9.25V11.25Z" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>
              <path d="M10.75 12.25V11.25C10.75 10.56 11.31 10 12 10C12.69 10 13.25 10.56 13.25 11.25V12.25" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>
            </svg>
          </div>
          <p class="security-caption">Secure local sign-in</p>
        </div>
        <p class="lead">ChatGPT, Claude, or another AI app is requesting access to your local Brain3 MCP gateway.</p>
        <div class="callout">
          <h2>Where to find your credentials</h2>
          <p>See your app TUI (Terminal UI) to get these values under the MCP <code>[c]</code> MCP config settings.</p>
          <p>You can also use the <code>B3_USERNAME</code> and <code>B3_PASSWORD</code> values from the <code>.env</code> file configured on this machine, which are the same.</p>
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
        </form>"#,
        error_html = error_html,
        fields_html = fields_html,
    );

    render_page(
        "Sign in to finish connecting your AI app",
        "Brain3 Local Gateway",
        "Sign in to finish connecting your AI app",
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
