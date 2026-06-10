use brain3_core::domain::oauth::AuthorizeRequest;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
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
            r#"<p style="color:#b91c1c;font-weight:600;">{}</p>"#,
            html_escape(msg)
        ),
        None => String::new(),
    };

    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Sign in to continue connecting your AI app</title>
  </head>
  <body style="font-family: sans-serif; max-width: 36rem; margin: 3rem auto; line-height: 1.5;">
    <h1>Sign in to continue connecting your AI app</h1>
    <p>ChatGPT, Claude, or another AI app is connecting to your local MCP gateway.</p>
    <p>Use the <code>USERNAME</code> and <code>PASSWORD</code> values from the <code>.env</code> file you configured earlier on this machine.</p>
    {error_html}
    <form method="post" action="/oauth/authorize">
      {fields_html}
      <label for="username">Username</label><br>
      <input id="username" name="username" type="text" autocomplete="username" required><br><br>
      <label for="password">Password</label><br>
      <input id="password" name="password" type="password" autocomplete="current-password" required><br><br>
      <button type="submit">Continue</button>
    </form>
  </body>
</html>"#
    )
}

pub fn render_misconfigured_page() -> String {
    r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Login credentials not configured</title>
  </head>
  <body style="font-family: sans-serif; max-width: 36rem; margin: 3rem auto; line-height: 1.5;">
    <h1>Login credentials not configured</h1>
    <p>This gateway requires a login before ChatGPT, Claude, or another AI app can finish connecting.</p>
    <p>Set <code>USERNAME</code> and <code>PASSWORD</code> in the <code>.env</code> file you configured earlier, then restart the gateway.</p>
  </body>
</html>"#
        .to_string()
}
