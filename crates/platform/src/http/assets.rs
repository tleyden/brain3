use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

const LOGIN_CSS: &str = include_str!("assets/login.css");
const LOGIN_LOGO_SVG: &str = include_str!("../../../../docs/logo/brain3-lockup-light.svg");

pub async fn login_stylesheet() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/css; charset=utf-8")], LOGIN_CSS)
}

pub async fn login_logo() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        LOGIN_LOGO_SVG,
    )
}
