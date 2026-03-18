//! Shared UI-facing models that can be rendered by both terminal and web
//! frontends without coupling those surfaces to each other's widget systems.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusTone {
    Idle,
    Ok,
    Warn,
    Danger,
}

impl StatusTone {
    pub fn css_class(self) -> &'static str {
        match self {
            Self::Idle => "pill-idle",
            Self::Ok => "pill-ok",
            Self::Warn => "pill-warn",
            Self::Danger => "pill-danger",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PillModel {
    pub label: String,
    pub tone: StatusTone,
}

impl PillModel {
    pub fn idle(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            tone: StatusTone::Idle,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeroModel {
    pub eyebrow: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactModel {
    pub label: String,
    pub value: String,
    pub href: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelModel {
    pub title: String,
    pub pill: PillModel,
    pub description: Option<String>,
    pub facts: Vec<FactModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapStep {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapShellModel {
    pub hero: HeroModel,
    pub gateway_panel: PanelModel,
    pub identity_panel: PanelModel,
    pub workspace_panel: PanelModel,
    pub steps: Vec<BootstrapStep>,
    pub nav_items: Vec<String>,
}

impl Default for BootstrapShellModel {
    fn default() -> Self {
        Self {
            hero: HeroModel {
                eyebrow: "RockBot Web".to_string(),
                title: "Import your client identity, then move onto the authenticated control plane."
                    .to_string(),
                body: "The public HTTPS listener is only a bootstrap surface. The real application surface belongs on the authenticated WebSocket with imported client identity material."
                    .to_string(),
            },
            gateway_panel: PanelModel {
                title: "Gateway".to_string(),
                pill: PillModel::idle("Checking"),
                description: Some(
                    "Bootstrap-only health and trust information from the public listener."
                        .to_string(),
                ),
                facts: vec![
                    FactModel {
                        label: "Health".to_string(),
                        value: "Loading...".to_string(),
                        href: None,
                    },
                    FactModel {
                        label: "CA Bundle".to_string(),
                        value: "Download public CA".to_string(),
                        href: Some("/api/cert/ca".to_string()),
                    },
                    FactModel {
                        label: "WS Auth".to_string(),
                        value: "Not connected".to_string(),
                        href: None,
                    },
                ],
            },
            identity_panel: PanelModel {
                title: "Browser Identity".to_string(),
                pill: PillModel::idle("No key imported"),
                description: Some(
                    "Import a PEM client certificate and private key. The browser stores the identity locally so you do not need to re-import it every time."
                        .to_string(),
                ),
                facts: vec![
                    FactModel {
                        label: "Storage".to_string(),
                        value: "IndexedDB-backed local identity persistence".to_string(),
                        href: None,
                    },
                    FactModel {
                        label: "Auth Path".to_string(),
                        value: "Certificate challenge/response over WebSocket".to_string(),
                        href: None,
                    },
                ],
            },
            workspace_panel: PanelModel {
                title: "Application Surface".to_string(),
                pill: PillModel::idle("Bootstrap mode"),
                description: Some(
                    "The next phase of the WebUI will render sessions, agents, providers, cron, and credentials over the same authenticated WS control plane used by native clients."
                        .to_string(),
                ),
                facts: vec![
                    FactModel {
                        label: "Preferred Stack".to_string(),
                        value: "Leptos + shared UI-model/state crates".to_string(),
                        href: None,
                    },
                    FactModel {
                        label: "Shared State".to_string(),
                        value: "Protocol, view-models, and theme semantics shared across TUI and WebUI".to_string(),
                        href: None,
                    },
                ],
            },
            steps: vec![
                BootstrapStep {
                    title: "1. Verify the gateway".to_string(),
                    body: "Use the public health endpoint and CA bundle to confirm you are connecting to the intended cluster."
                        .to_string(),
                },
                BootstrapStep {
                    title: "2. Import client identity".to_string(),
                    body: "Drop or select PEM certificate and private key files, then persist them locally in the browser."
                        .to_string(),
                },
                BootstrapStep {
                    title: "3. Authenticate over WS".to_string(),
                    body: "The browser authenticates over the bootstrap WebSocket and then moves onto the authenticated app control plane."
                        .to_string(),
                },
            ],
            nav_items: vec![
                "Sessions".to_string(),
                "Agents".to_string(),
                "Providers".to_string(),
                "Credentials".to_string(),
                "Cron".to_string(),
            ],
        }
    }
}
