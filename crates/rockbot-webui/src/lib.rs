//! Embedded browser bootstrap UI for RockBot.
//!
//! The gateway still serves a narrow public surface, but the HTML is now
//! rendered through Leptos components over shared UI-model types instead of a
//! single hand-built string blob.

mod render;

use leptos::prelude::*;
use rockbot_ui_model::BootstrapShellModel;

const APP_CSS: &str = include_str!("static/app.css");
const APP_JS: &str = include_str!("static/app.js");

/// Return the browser bootstrap shell.
pub fn get_dashboard_html() -> String {
    let app_html = view! { <render::BootstrapApp model=BootstrapShellModel::default() /> }.to_html();
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>RockBot</title>
  <link rel="stylesheet" href="/static/app.css">
</head>
<body>
  <div id="app-root">{app_html}</div>
  <script src="/static/app.js" defer></script>
</body>
</html>
"#
    )
}

/// Return a static asset by request path.
pub fn get_static_asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path {
        "/static/app.css" => Some(("text/css; charset=utf-8", APP_CSS)),
        "/static/app.js" => Some(("application/javascript; charset=utf-8", APP_JS)),
        _ => None,
    }
}
