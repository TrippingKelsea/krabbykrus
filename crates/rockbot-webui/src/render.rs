use leptos::prelude::*;
use rockbot_ui_model::{BootstrapShellModel, BootstrapStep, FactModel, PanelModel};

#[component]
pub fn BootstrapApp(model: BootstrapShellModel) -> impl IntoView {
    view! {
        <main class="shell" aria-labelledby="app-title">
            <header class="masthead">
                <div class="brand">
                    <p class="eyebrow">{model.hero.eyebrow.clone()}</p>
                    <h1 id="app-title">{model.hero.title.clone()}</h1>
                    <p class="lede">{model.hero.body.clone()}</p>
                </div>

                <nav class="nav-card" aria-label="Planned application surfaces">
                    <p class="nav-card-title">Authenticated surfaces</p>
                    <ul class="nav-list">
                        {model
                            .nav_items
                            .into_iter()
                            .map(|item| view! { <li>{item}</li> })
                            .collect_view()}
                    </ul>
                </nav>
            </header>

            <section class="panel-grid" aria-label="Bootstrap status">
                <Panel panel=model.gateway_panel id_prefix="gateway".to_string() />
                <Panel panel=model.identity_panel id_prefix="identity".to_string() />
                <Panel panel=model.workspace_panel id_prefix="workspace".to_string() />
            </section>

            <section class="identity-controls panel" aria-labelledby="identity-controls-title">
                <div class="panel-header">
                    <div>
                        <p class="section-eyebrow">Import</p>
                        <h2 id="identity-controls-title">Client identity</h2>
                    </div>
                    <span id="identity-pill" class="pill pill-idle">No key imported</span>
                </div>
                <p class="help">
                    Import a PEM client certificate and PEM private key. RockBot stores the imported
                    identity locally in IndexedDB so the browser can re-authenticate without forcing
                    re-import on every page load.
                </p>

                <div id="dropzone" class="dropzone" tabindex="0" role="button" aria-label="Drop PEM files here">
                    <strong>Drop PEM files here</strong>
                    <span>or choose them manually below</span>
                </div>

                <div class="form-grid">
                    <label>
                        <span>Client Certificate (.crt/.pem)</span>
                        <input id="cert-file" type="file" accept=".crt,.pem" />
                    </label>
                    <label>
                        <span>Private Key (.key/.pem)</span>
                        <input id="key-file" type="file" accept=".key,.pem" />
                    </label>
                </div>

                <div class="actions">
                    <button id="save-btn" class="btn btn-primary" type="button">Save Identity</button>
                    <button id="clear-btn" class="btn btn-secondary" type="button">Forget Identity</button>
                </div>

                <pre id="identity-summary" class="summary">No client identity stored.</pre>
            </section>

            <section class="steps panel" aria-labelledby="next-steps-title">
                <div class="panel-header">
                    <div>
                        <p class="section-eyebrow">Flow</p>
                        <h2 id="next-steps-title">Bootstrap sequence</h2>
                    </div>
                    <span class="pill pill-idle">WS-first app</span>
                </div>

                <ol class="step-list">
                    {model
                        .steps
                        .into_iter()
                        .map(|step| view! { <BootstrapStepCard step /> })
                        .collect_view()}
                </ol>
            </section>
        </main>
    }
}

#[component]
fn Panel(panel: PanelModel, id_prefix: String) -> impl IntoView {
    let heading_id = format!("{id_prefix}-heading");
    let status_id = format!("{id_prefix}-status");
    let heading_for_section = heading_id.clone();
    let heading_for_title = heading_id.clone();

    view! {
        <section class="panel" aria-labelledby=heading_for_section>
            <div class="panel-header">
                <div>
                    <p class="section-eyebrow">Status</p>
                    <h2 id=heading_for_title>{panel.title}</h2>
                </div>
                <span id=status_id class=format!("pill {}", panel.pill.tone.css_class())>
                    {panel.pill.label}
                </span>
            </div>
            {panel
                .description
                .map(|description| view! { <p class="help">{description}</p> })}
            <dl class="facts">
                {panel
                    .facts
                    .into_iter()
                    .enumerate()
                    .map(|(index, fact)| {
                        view! { <FactCard fact id_prefix=format!("{id_prefix}-{index}") /> }
                    })
                    .collect_view()}
            </dl>
        </section>
    }
}

#[component]
fn FactCard(fact: FactModel, id_prefix: String) -> impl IntoView {
    let label_id = format!("{id_prefix}-label");
    let value_id = format!("{id_prefix}-value");
    let label_for_card = label_id.clone();
    let label_for_term = label_id.clone();
    let value_for_card = value_id.clone();
    let value_for_desc = value_id.clone();
    let is_health = fact.label == "Health";
    let is_ws_auth = fact.label == "WS Auth";

    view! {
        <div class="fact-card" aria-labelledby=label_for_card aria-describedby=value_for_card>
            <dt id=label_for_term>{fact.label.clone()}</dt>
            <dd id=value_for_desc>
                {match fact.href {
                    Some(href) => view! { <a href=href rel="noreferrer">{fact.value}</a> }.into_any(),
                    None if is_health => view! { <span id="health-text">{fact.value}</span> }.into_any(),
                    None if is_ws_auth => view! { <span id="ws-auth-text">{fact.value}</span> }.into_any(),
                    None => view! { <span>{fact.value}</span> }.into_any(),
                }}
            </dd>
        </div>
    }
}

#[component]
fn BootstrapStepCard(step: BootstrapStep) -> impl IntoView {
    view! {
        <li class="step-card">
            <h3>{step.title}</h3>
            <p>{step.body}</p>
        </li>
    }
}
