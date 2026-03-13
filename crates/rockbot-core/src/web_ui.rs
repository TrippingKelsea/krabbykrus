//! Embedded Web UI for RockBot Gateway
//!
//! Navigation synchronized with TUI:
//! - Dashboard (status overview)
//! - Credentials (vault management with sub-tabs)
//! - Agents (configuration)
//! - Sessions (active sessions + chat)
//! - Models (provider config with detail panel)
//! - Settings (gateway config)

/// Returns the main web UI HTML
#[allow(clippy::too_many_lines)]
pub fn get_dashboard_html() -> &'static str {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>RockBot Gateway</title>
    <style>
        :root {
            --bg: #0f0f1a;
            --surface: #1a1a2e;
            --surface-2: #232342;
            --primary: #e94560;
            --secondary: #0f3460;
            --accent: #7c3aed;
            --text: #f0f0f0;
            --text-dim: #8888aa;
            --success: #10b981;
            --warning: #f59e0b;
            --error: #ef4444;
            --border: #2a2a4a;
        }
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body {
            font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
            background: var(--bg);
            color: var(--text);
            min-height: 100vh;
        }

        /* Layout */
        .app { display: flex; min-height: 100vh; }
        .sidebar {
            width: 220px;
            background: var(--surface);
            border-right: 1px solid var(--border);
            padding: 1rem;
            display: flex;
            flex-direction: column;
        }
        .main { flex: 1; overflow-y: auto; display: flex; flex-direction: column; }
        .content { padding: 2rem; max-width: 1400px; margin: 0 auto; flex: 1; width: 100%; }

        /* Sidebar */
        .logo { font-size: 1.5rem; font-weight: 700; color: var(--primary); padding: 1rem 0.5rem; margin-bottom: 1rem; display: flex; align-items: center; gap: 0.5rem; }
        .nav { list-style: none; flex: 1; }
        .nav-item {
            padding: 0.75rem 1rem; border-radius: 8px; cursor: pointer;
            display: flex; align-items: center; gap: 0.75rem;
            color: var(--text-dim); transition: all 0.15s; margin-bottom: 0.25rem;
            font-size: 0.9rem;
        }
        .nav-item:hover { background: var(--surface-2); color: var(--text); }
        .nav-item.active { background: var(--primary); color: white; }
        .nav-item .icon { font-size: 1.1rem; width: 24px; text-align: center; }
        .status-dot { width: 8px; height: 8px; border-radius: 50%; margin-left: auto; }
        .status-dot.online { background: var(--success); }
        .status-dot.offline { background: var(--error); }
        .sidebar-footer { padding-top: 1rem; border-top: 1px solid var(--border); margin-top: auto; font-size: 0.75rem; color: var(--text-dim); }

        /* Cards */
        .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1.5rem; margin-bottom: 2rem; }
        .grid-4 { grid-template-columns: repeat(4, 1fr); }
        .card { background: var(--surface); border: 1px solid var(--border); border-radius: 12px; padding: 1.5rem; }
        .card-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 1rem; }
        .card h3 { font-size: 0.875rem; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.05em; }
        .card .value { font-size: 2rem; font-weight: 700; }
        .card .sub { color: var(--text-dim); font-size: 0.875rem; margin-top: 0.5rem; }

        /* Tables */
        table { width: 100%; border-collapse: collapse; }
        th, td { text-align: left; padding: 0.875rem 1rem; border-bottom: 1px solid var(--border); }
        th { color: var(--text-dim); font-weight: 500; font-size: 0.8rem; text-transform: uppercase; }
        tbody tr:hover { background: var(--surface-2); }

        /* Buttons */
        .btn {
            padding: 0.625rem 1.25rem; border-radius: 8px; border: none; cursor: pointer;
            font-size: 0.875rem; font-weight: 500; transition: all 0.15s;
            display: inline-flex; align-items: center; gap: 0.5rem;
        }
        .btn-primary { background: var(--primary); color: white; }
        .btn-primary:hover { filter: brightness(1.1); }
        .btn-secondary { background: var(--surface-2); color: var(--text); border: 1px solid var(--border); }
        .btn-secondary:hover { background: var(--border); }
        .btn-danger { background: var(--error); color: white; }
        .btn-danger:hover { filter: brightness(1.1); }
        .btn-success { background: var(--success); color: white; }
        .btn-sm { padding: 0.375rem 0.75rem; font-size: 0.75rem; }
        .btn-icon { padding: 0.5rem; min-width: 2rem; justify-content: center; }

        /* Forms */
        .form-group { margin-bottom: 1rem; }
        .form-group label { display: block; margin-bottom: 0.5rem; color: var(--text-dim); font-size: 0.875rem; }
        .form-group .hint { font-size: 0.75rem; color: var(--text-dim); margin-top: 0.25rem; }
        input, select, textarea {
            width: 100%; padding: 0.75rem; background: var(--bg);
            border: 1px solid var(--border); border-radius: 8px; color: var(--text); font-size: 0.875rem;
            font-family: inherit;
        }
        input:focus, select:focus, textarea:focus { outline: none; border-color: var(--primary); }
        input[readonly] { opacity: 0.7; cursor: default; }
        .form-row { display: flex; gap: 1rem; }
        .form-row .form-group { flex: 1; }
        .required::after { content: ' *'; color: var(--error); }

        /* Modal */
        .modal-overlay {
            position: fixed; inset: 0; background: rgba(0,0,0,0.7);
            display: flex; align-items: center; justify-content: center; z-index: 1000;
        }
        .modal {
            background: var(--surface); border-radius: 12px; padding: 2rem;
            min-width: 500px; max-width: 700px; max-height: 85vh; overflow-y: auto;
        }
        .modal h2 { margin-bottom: 1.5rem; }
        .modal-actions { display: flex; justify-content: flex-end; gap: 0.75rem; margin-top: 1.5rem; padding-top: 1.5rem; border-top: 1px solid var(--border); }

        /* Form sections */
        .form-section {
            background: var(--surface-2); border-radius: 8px; padding: 1rem; margin-bottom: 1rem;
            border: 1px solid var(--border);
        }
        .form-section h4 { font-size: 0.875rem; color: var(--accent); margin-bottom: 1rem; }

        /* Tabs */
        .tab-bar { display: flex; gap: 0; margin-bottom: 1.5rem; border-bottom: 2px solid var(--border); }
        .tab-item {
            padding: 0.75rem 1.25rem; cursor: pointer; color: var(--text-dim);
            border-bottom: 2px solid transparent; margin-bottom: -2px;
            transition: all 0.15s; font-size: 0.9rem; font-weight: 500;
        }
        .tab-item:hover { color: var(--text); background: var(--surface-2); }
        .tab-item.active { color: var(--primary); border-bottom-color: var(--primary); }

        /* Chat */
        .chat-container { display: flex; flex-direction: column; height: calc(100vh - 200px); }
        .chat-messages { flex: 1; overflow-y: auto; padding: 1rem; display: flex; flex-direction: column; gap: 1rem; }
        .message { max-width: 80%; padding: 1rem; border-radius: 12px; }
        .message.user { background: var(--primary); align-self: flex-end; }
        .message.assistant { background: var(--surface-2); align-self: flex-start; }
        .message pre { background: var(--bg); padding: 0.75rem; border-radius: 6px; overflow-x: auto; margin-top: 0.5rem; font-size: 0.8rem; }
        .chat-input { display: flex; gap: 0.75rem; padding: 1rem; background: var(--surface); border-top: 1px solid var(--border); }
        .chat-input input { flex: 1; }

        /* Badges */
        .badge { padding: 0.25rem 0.75rem; border-radius: 999px; font-size: 0.75rem; font-weight: 500; display: inline-block; }
        .badge-success { background: rgba(16,185,129,0.2); color: var(--success); }
        .badge-warning { background: rgba(245,158,11,0.2); color: var(--warning); }
        .badge-error { background: rgba(239,68,68,0.2); color: var(--error); }
        .badge-info { background: rgba(124,58,237,0.2); color: var(--accent); }

        /* Page header */
        .page-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 2rem; }
        .page-header h1 { font-size: 1.5rem; }

        /* Split layout */
        .split { display: grid; gap: 1.5rem; }
        .split-40-60 { grid-template-columns: 40% 1fr; }
        .split-35-65 { grid-template-columns: 35% 1fr; }

        /* List items */
        .list-container { max-height: 500px; overflow-y: auto; }
        .list-item { padding: 0.875rem 1rem; border-bottom: 1px solid var(--border); cursor: pointer; transition: background 0.1s; }
        .list-item:hover { background: var(--surface-2); }
        .list-item.active { background: var(--primary); color: white; }
        .list-item .title { font-weight: 500; }
        .list-item .meta { font-size: 0.75rem; color: var(--text-dim); margin-top: 0.25rem; }
        .list-item.active .meta { color: rgba(255,255,255,0.7); }

        /* Credential indicator */
        .cred-dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 6px; }
        .cred-dot.configured { background: var(--success); }
        .cred-dot.unconfigured { background: var(--warning); }

        /* Detail panel */
        .detail-field { margin-bottom: 1rem; }
        .detail-field label { display: block; font-size: 0.75rem; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 0.25rem; }
        .detail-field .val { font-size: 0.9rem; }

        /* Provider detail */
        .capability { display: inline-flex; align-items: center; gap: 0.25rem; margin-right: 1rem; font-size: 0.85rem; }
        .capability.yes { color: var(--success); }
        .capability.no { color: var(--text-dim); }
        .config-hint { background: var(--surface-2); border-radius: 8px; padding: 1rem; margin-top: 1rem; border-left: 3px solid var(--accent); }
        .config-hint h4 { color: var(--accent); font-size: 0.85rem; margin-bottom: 0.5rem; }
        .config-hint code { background: var(--bg); padding: 0.125rem 0.375rem; border-radius: 4px; font-size: 0.8rem; }
        .config-hint pre { background: var(--bg); padding: 0.75rem; border-radius: 6px; margin-top: 0.5rem; font-size: 0.8rem; overflow-x: auto; }

        /* Toast notification */
        .toast {
            position: fixed; bottom: 1.5rem; right: 1.5rem; z-index: 2000;
            padding: 0.875rem 1.25rem; border-radius: 8px; font-size: 0.875rem;
            max-width: 400px; animation: slideIn 0.2s ease-out;
            box-shadow: 0 4px 12px rgba(0,0,0,0.3);
        }
        .toast-success { background: var(--success); color: white; }
        .toast-error { background: var(--error); color: white; }
        @keyframes slideIn { from { transform: translateY(1rem); opacity: 0; } to { transform: translateY(0); opacity: 1; } }

        /* Status bar */
        .status-bar {
            background: var(--surface); border-top: 1px solid var(--border);
            padding: 0.5rem 2rem; font-size: 0.75rem; color: var(--text-dim);
            display: flex; justify-content: space-between; align-items: center;
        }

        /* Utilities */
        .hidden { display: none !important; }
        .text-dim { color: var(--text-dim); }
        .text-success { color: var(--success); }
        .text-warning { color: var(--warning); }
        .text-error { color: var(--error); }
        .mt-1 { margin-top: 0.5rem; }
        .mt-2 { margin-top: 1rem; }
        .mb-1 { margin-bottom: 0.5rem; }
        .mb-2 { margin-bottom: 1rem; }
        .flex { display: flex; }
        .gap-1 { gap: 0.5rem; }
        .gap-2 { gap: 0.75rem; }
        .items-center { align-items: center; }
        .justify-between { justify-content: space-between; }
        .flex-1 { flex: 1; }

        /* Keyboard shortcuts hint */
        .shortcuts { position: fixed; bottom: 3rem; right: 1rem; background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 0.5rem 1rem; font-size: 0.75rem; color: var(--text-dim); }
        .shortcuts kbd { background: var(--bg); padding: 0.125rem 0.375rem; border-radius: 4px; margin: 0 0.125rem; }
    </style>
</head>
<body>
    <div class="app">
        <aside class="sidebar">
            <div class="logo">&#x1f980; RockBot</div>
            <ul class="nav">
                <li class="nav-item active" data-page="dashboard">
                    <span class="icon">&#x1f4ca;</span> Dashboard
                    <span class="status-dot online" id="status-dot"></span>
                </li>
                <li class="nav-item" data-page="credentials">
                    <span class="icon">&#x1f510;</span> Credentials
                </li>
                <li class="nav-item" data-page="agents">
                    <span class="icon">&#x1f916;</span> Agents
                </li>
                <li class="nav-item" data-page="sessions">
                    <span class="icon">&#x1f4ac;</span> Sessions
                </li>
                <li class="nav-item" data-page="models">
                    <span class="icon">&#x1f9e0;</span> Models
                </li>
                <li class="nav-item" data-page="settings">
                    <span class="icon">&#x2699;&#xfe0f;</span> Settings
                </li>
            </ul>
            <div class="sidebar-footer">
                <div id="version-info">v0.1.0</div>
            </div>
        </aside>

        <main class="main">
            <!-- Dashboard Page -->
            <div id="page-dashboard" class="content page">
                <div class="page-header">
                    <h1>Dashboard</h1>
                    <span class="badge badge-success" id="gateway-status">Online</span>
                </div>
                <div class="grid grid-4">
                    <div class="card"><h3>Gateway</h3><div class="value" id="stat-gateway">&#x25cf;</div><div class="sub" id="stat-version">-</div></div>
                    <div class="card"><h3>Agents</h3><div class="value"><span id="stat-agents">0</span></div><div class="sub" id="stat-pending"></div></div>
                    <div class="card"><h3>Sessions</h3><div class="value" id="stat-sessions">0</div><div class="sub" id="stat-sessions-sub">active</div></div>
                    <div class="card"><h3>Vault</h3><div class="value" id="stat-vault">-</div><div class="sub" id="stat-vault-info"></div></div>
                </div>
                <div class="card">
                    <div class="card-header"><h3>Configured Agents</h3><button class="btn btn-primary btn-sm" onclick="reloadAgents()">&#x21bb; Reload</button></div>
                    <table><thead><tr><th>Agent ID</th><th>Model</th><th>Sessions</th><th>Status</th></tr></thead><tbody id="agents-table"></tbody></table>
                </div>
            </div>

            <!-- Credentials Page -->
            <div id="page-credentials" class="content page hidden">
                <div class="page-header">
                    <h1>Credential Vault</h1>
                    <div id="cred-header-actions"></div>
                </div>

                <!-- Vault gate: init/unlock -->
                <div id="vault-gate">
                    <div id="vault-init-section" class="card mb-2 hidden">
                        <h3 class="mb-2">Initialize Vault</h3>
                        <p class="text-dim mb-2">Set up the credential vault to securely store API keys and secrets.</p>
                        <div class="form-group"><label>Password (min 8 characters)</label><input type="password" id="init-password" placeholder="Enter password"></div>
                        <div class="form-group"><label>Confirm Password</label><input type="password" id="init-password-confirm" placeholder="Confirm password"></div>
                        <button class="btn btn-primary" onclick="initializeVault()">Initialize Vault</button>
                    </div>
                    <div id="vault-unlock-section" class="card mb-2 hidden">
                        <h3 class="mb-1">&#x1f512; Vault Locked</h3>
                        <p class="text-dim mb-2">Enter your password to unlock the vault.</p>
                        <div class="flex gap-2"><input type="password" id="unlock-password" placeholder="Password" style="flex:1" onkeypress="if(event.key==='Enter')unlockVault()"><button class="btn btn-primary" onclick="unlockVault()">Unlock</button></div>
                    </div>
                    <div id="vault-disabled-section" class="card mb-2 hidden">
                        <p class="text-dim">Credential management is disabled in the gateway configuration.</p>
                    </div>
                </div>

                <!-- Vault unlocked content with sub-tabs -->
                <div id="vault-content" class="hidden">
                    <div class="card mb-2">
                        <div class="flex items-center gap-2">
                            <span class="badge badge-success">&#x1f513; Unlocked</span>
                            <button class="btn btn-secondary btn-sm" onclick="lockVault()">Lock Vault</button>
                        </div>
                    </div>

                    <div class="tab-bar" id="cred-tabs">
                        <div class="tab-item active" data-credtab="endpoints" onclick="showCredTab('endpoints')">Endpoints</div>
                        <div class="tab-item" data-credtab="providers" onclick="showCredTab('providers')">Providers</div>
                        <div class="tab-item" data-credtab="permissions" onclick="showCredTab('permissions')">Permissions</div>
                        <div class="tab-item" data-credtab="audit" onclick="showCredTab('audit')">Audit</div>
                    </div>

                    <!-- Endpoints sub-tab -->
                    <div id="credtab-endpoints" class="credtab-content">
                        <div class="split split-40-60">
                            <div class="card">
                                <div class="card-header"><h3>Endpoints</h3><button class="btn btn-primary btn-sm" onclick="showAddEndpoint()">+ Add</button></div>
                                <div id="endpoint-list" class="list-container">
                                    <p class="text-dim" style="padding:1rem">Loading...</p>
                                </div>
                            </div>
                            <div class="card">
                                <h3 class="mb-2">Details</h3>
                                <div id="endpoint-details">
                                    <p class="text-dim">Select an endpoint to view details</p>
                                </div>
                            </div>
                        </div>
                    </div>

                    <!-- Providers sub-tab -->
                    <div id="credtab-providers" class="credtab-content hidden">
                        <div class="split split-35-65">
                            <div class="card">
                                <h3 class="mb-2">Categories</h3>
                                <div id="schema-category-list" class="list-container"></div>
                            </div>
                            <div class="card">
                                <h3 class="mb-2">Providers</h3>
                                <div id="schema-provider-list" class="list-container">
                                    <p class="text-dim" style="padding:1rem">Start the gateway to see providers</p>
                                </div>
                            </div>
                        </div>
                    </div>

                    <!-- Permissions sub-tab -->
                    <div id="credtab-permissions" class="credtab-content hidden">
                        <div class="card">
                            <div class="card-header"><h3>Permission Rules</h3><button class="btn btn-primary btn-sm" onclick="showAddPermission()">+ Add Rule</button></div>
                            <div id="permissions-content">
                                <table><thead><tr><th>Pattern</th><th>Level</th><th>Description</th><th>Actions</th></tr></thead><tbody id="permissions-table"></tbody></table>
                            </div>
                        </div>
                    </div>

                    <!-- Audit sub-tab -->
                    <div id="credtab-audit" class="credtab-content hidden">
                        <div class="card">
                            <div class="card-header"><h3>Audit Log</h3><button class="btn btn-secondary btn-sm" onclick="loadAuditLog()">&#x21bb; Refresh</button></div>
                            <div id="audit-content">
                                <table><thead><tr><th>Time</th><th>Action</th><th>Path</th><th>Agent</th><th>Result</th></tr></thead><tbody id="audit-table"></tbody></table>
                            </div>
                        </div>
                    </div>
                </div>
            </div>

            <!-- Agents Page -->
            <div id="page-agents" class="content page hidden">
                <div class="page-header">
                    <h1>Agent Configuration</h1>
                    <div class="flex gap-1">
                        <button class="btn btn-primary" onclick="showCreateAgent()">+ Create Agent</button>
                        <button class="btn btn-secondary" onclick="refreshAgents()">&#x21bb; Refresh</button>
                    </div>
                </div>
                <div class="split split-40-60">
                    <div class="card">
                        <h3 class="mb-2">Agents</h3>
                        <div id="agent-list" class="list-container"></div>
                    </div>
                    <div class="card">
                        <h3 class="mb-2">Details</h3>
                        <div id="agent-details">
                            <p class="text-dim">Select an agent to view details</p>
                        </div>
                    </div>
                </div>
            </div>

            <!-- Sessions Page -->
            <div id="page-sessions" class="content page hidden">
                <div class="page-header">
                    <h1>Active Sessions</h1>
                    <select id="chat-agent" class="btn btn-secondary"><option value="">New Chat...</option></select>
                </div>
                <div class="split split-35-65">
                    <div class="card">
                        <h3 class="mb-2">Sessions</h3>
                        <div id="session-list" class="list-container"></div>
                    </div>
                    <div class="card chat-container">
                        <div class="chat-messages" id="chat-messages">
                            <div class="message assistant"><p>Select a session or start a new chat.</p></div>
                        </div>
                        <div class="chat-input">
                            <input type="text" id="chat-input" placeholder="Type a message..." onkeypress="if(event.key==='Enter')sendMessage()">
                            <button class="btn btn-primary" onclick="sendMessage()">Send</button>
                        </div>
                    </div>
                </div>
            </div>

            <!-- Models Page -->
            <div id="page-models" class="content page hidden">
                <div class="page-header"><h1>Model Providers</h1></div>
                <div class="split split-35-65">
                    <div class="card">
                        <h3 class="mb-2">Providers</h3>
                        <div id="provider-list" class="list-container">
                            <p class="text-dim" style="padding:1rem">Loading providers...</p>
                        </div>
                    </div>
                    <div class="card">
                        <h3 class="mb-2">Details</h3>
                        <div id="provider-details">
                            <p class="text-dim">No providers loaded. Make sure the gateway is running.</p>
                        </div>
                    </div>
                </div>
            </div>

            <!-- Settings Page -->
            <div id="page-settings" class="content page hidden">
                <div class="page-header"><h1>Settings</h1></div>
                <div class="grid">
                    <div class="card">
                        <h3 class="mb-2">Gateway</h3>
                        <div class="flex items-center justify-between mb-2">
                            <span>Status</span>
                            <span class="badge badge-success" id="settings-gateway-status">Running</span>
                        </div>
                        <div class="flex items-center justify-between mb-2">
                            <span>Version</span>
                            <span id="settings-version">-</span>
                        </div>
                        <div class="flex items-center justify-between mb-2">
                            <span>Port</span>
                            <span>18080</span>
                        </div>
                        <div class="flex items-center justify-between mb-2">
                            <span>Active Sessions</span>
                            <span id="settings-sessions">0</span>
                        </div>
                        <div class="flex items-center justify-between mb-2">
                            <span>Configured Agents</span>
                            <span id="settings-agents">0</span>
                        </div>
                        <div class="flex gap-1 mt-2">
                            <button class="btn btn-secondary btn-sm" onclick="restartGateway()">Restart</button>
                            <button class="btn btn-danger btn-sm" onclick="stopGateway()">Stop</button>
                        </div>
                    </div>
                    <div class="card">
                        <h3 class="mb-2">Paths</h3>
                        <div class="form-group">
                            <label>Config File</label>
                            <input type="text" id="config-path" readonly value="~/.config/rockbot/rockbot.toml">
                        </div>
                        <div class="form-group">
                            <label>Vault Path</label>
                            <input type="text" id="vault-path" readonly value="~/.config/rockbot/vault">
                        </div>
                    </div>
                </div>
                <div class="card mt-2">
                    <h3 class="mb-2">About</h3>
                    <p><strong>&#x1f980; RockBot</strong> &mdash; A Rust-native AI agent framework</p>
                    <p class="text-dim mt-1">https://github.com/TrippingKelsea/rockbot</p>
                    <p class="text-dim mt-1">Local-first, secure credential management, multi-provider LLM support</p>
                </div>
            </div>

            <!-- Status bar -->
            <div class="status-bar">
                <span id="statusbar-left">Ready</span>
                <span id="statusbar-right"></span>
            </div>
        </main>
    </div>

    <!-- Add/Edit Endpoint Modal -->
    <div id="modal-endpoint" class="modal-overlay hidden">
        <div class="modal">
            <h2 id="endpoint-modal-title">Add Service Endpoint</h2>
            <div class="form-group">
                <label class="required">Endpoint Name</label>
                <input type="text" id="endpoint-name" placeholder="e.g., My Home Assistant">
            </div>
            <div class="form-group" id="endpoint-type-group">
                <label>Service Type</label>
                <select id="endpoint-type" onchange="updateEndpointForm()">
                    <option value="home_assistant">Home Assistant</option>
                    <option value="generic_rest">Generic REST API</option>
                    <option value="generic_oauth2">OAuth2 Service</option>
                    <option value="api_key_service">API Key Service</option>
                    <option value="basic_auth_service">Basic Auth Service</option>
                    <option value="bearer_token">Bearer Token</option>
                </select>
            </div>
            <div id="endpoint-form-dynamic"></div>
            <div class="modal-actions">
                <button class="btn btn-secondary" onclick="closeModal('endpoint')">Cancel</button>
                <button class="btn btn-primary" id="endpoint-save-btn" onclick="saveEndpoint()">Save</button>
            </div>
        </div>
    </div>

    <!-- Agent Create/Edit Modal -->
    <div id="modal-agent" class="modal-overlay hidden">
        <div class="modal">
            <h2 id="agent-modal-title">Create Agent</h2>
            <div class="form-group">
                <label class="required">Agent ID</label>
                <input type="text" id="agent-id" placeholder="e.g., my-agent">
                <div class="hint">Unique identifier (no spaces or slashes)</div>
            </div>
            <div class="form-group">
                <label>Model</label>
                <input type="text" id="agent-model" placeholder="e.g., anthropic/claude-sonnet-4-20250514">
                <div class="hint">Leave empty to use default model</div>
            </div>
            <div class="form-group">
                <label>Parent Agent (Subagent)</label>
                <select id="agent-parent"><option value="">None (top-level agent)</option></select>
                <div class="hint">Set a parent to create a subagent</div>
            </div>
            <div class="form-group">
                <label>Workspace</label>
                <input type="text" id="agent-workspace" placeholder="uses default if empty">
            </div>
            <div class="form-group">
                <label>Max Tool Calls</label>
                <input type="number" id="agent-max-tools" placeholder="dynamic (32-160)" min="1" max="200">
            </div>
            <div class="form-group">
                <label>Temperature</label>
                <input type="number" id="agent-temperature" value="0.3" min="0" max="2" step="0.1">
            </div>
            <div class="form-group">
                <label>Max Tokens</label>
                <input type="number" id="agent-max-tokens" value="16000" min="100" max="128000" step="1000">
            </div>
            <div class="form-group">
                <label>System Prompt</label>
                <textarea id="agent-system-prompt" rows="4" placeholder="Optional system prompt override"></textarea>
            </div>
            <div id="agent-subagents-info" class="hidden" style="margin-bottom:1rem;padding:0.75rem;background:var(--surface-2);border-radius:8px;border-left:3px solid var(--accent)">
                <strong style="color:var(--accent)">Subagents:</strong> <span id="agent-subagents-list"></span>
            </div>
            <div class="modal-actions">
                <button class="btn btn-secondary" onclick="closeModal('agent')">Cancel</button>
                <button class="btn btn-primary" id="agent-save-btn" onclick="saveAgent()">Create</button>
            </div>
        </div>
    </div>

    <!-- Edit Provider Modal -->
    <div id="modal-provider" class="modal-overlay hidden">
        <div class="modal">
            <h2 id="provider-modal-title">Configure Provider</h2>
            <div id="provider-modal-content"></div>
            <div class="modal-actions">
                <button class="btn btn-secondary" onclick="closeModal('provider')">Cancel</button>
                <button class="btn btn-primary" onclick="saveProviderConfig()">Save</button>
            </div>
        </div>
    </div>

    <!-- Add Permission Modal -->
    <div id="modal-permission" class="modal-overlay hidden">
        <div class="modal">
            <h2>Add Permission Rule</h2>
            <div class="form-group">
                <label class="required">Path Pattern</label>
                <input type="text" id="perm-path" placeholder="e.g., homeassistant/**">
                <div class="hint">Glob pattern matching credential paths</div>
            </div>
            <div class="form-group">
                <label>Permission Level</label>
                <select id="perm-level">
                    <option value="allow">Allow</option>
                    <option value="allow_hil">Allow with HIL</option>
                    <option value="allow_hil_2fa">Allow with HIL + 2FA</option>
                    <option value="deny">Deny</option>
                </select>
            </div>
            <div class="form-group">
                <label>Description</label>
                <input type="text" id="perm-description" placeholder="Optional description">
            </div>
            <div class="modal-actions">
                <button class="btn btn-secondary" onclick="closeModal('permission')">Cancel</button>
                <button class="btn btn-primary" onclick="savePermission()">Add Rule</button>
            </div>
        </div>
    </div>

    <div class="shortcuts">Press <kbd>1</kbd>-<kbd>6</kbd> for quick nav</div>

    <script>
        // ========== State ==========
        let currentPage = 'dashboard';
        let sessionKey = 'web-' + Date.now();
        let selectedSession = null;
        let selectedAgentIdx = null;
        let agents = [];
        let allAgents = []; // from /api/agents (richer data)
        let sessions = [];
        let providers = [];
        let selectedProviderIdx = null;
        let credentialSchemas = [];
        let endpoints = [];
        let selectedEndpointIdx = null;
        let editingAgentId = null;
        let editingEndpointId = null;
        let currentCredTab = 'endpoints';
        let selectedSchemaCategory = 0;
        let permissions = [];
        let auditEntries = [];

        const SCHEMA_CATEGORIES = [
            { label: 'All', icon: '', filter: null },
            { label: 'Model Providers', icon: '', filter: 'model' },
            { label: 'Communication', icon: '', filter: 'communication' },
            { label: 'Tools', icon: '', filter: 'tool' }
        ];

        // ========== Navigation ==========
        document.querySelectorAll('.nav-item').forEach(item => {
            item.addEventListener('click', () => showPage(item.dataset.page));
        });

        document.addEventListener('keydown', e => {
            if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') return;
            const pages = ['dashboard', 'credentials', 'agents', 'sessions', 'models', 'settings'];
            if (e.key >= '1' && e.key <= '6') showPage(pages[parseInt(e.key) - 1]);
        });

        function showPage(page) {
            document.querySelectorAll('.nav-item').forEach(i => i.classList.remove('active'));
            document.querySelector(`[data-page="${page}"]`)?.classList.add('active');
            document.querySelectorAll('.page').forEach(p => p.classList.add('hidden'));
            document.getElementById(`page-${page}`)?.classList.remove('hidden');
            currentPage = page;
            if (page === 'credentials') loadCredentials();
            if (page === 'agents') loadAgentsPage();
            if (page === 'sessions') loadSessionsPage();
            if (page === 'models') loadModelsPage();
        }

        // ========== API Helpers ==========
        async function api(url, method = 'GET', data = null) {
            const opts = { method, headers: { 'Content-Type': 'application/json' } };
            if (data) opts.body = JSON.stringify(data);
            const res = await fetch(url, opts);
            const json = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(json.error || `HTTP ${res.status}`);
            return json;
        }

        function toast(msg, isError = false) {
            const el = document.createElement('div');
            el.className = 'toast ' + (isError ? 'toast-error' : 'toast-success');
            el.textContent = msg;
            document.body.appendChild(el);
            setTimeout(() => el.remove(), 3000);
        }

        function setStatus(msg) { document.getElementById('statusbar-left').textContent = msg; }
        function escapeHtml(t) { return String(t).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }
        function formatMessage(t) { return '<p>' + escapeHtml(t).replace(/```([\s\S]*?)```/g,'</p><pre>$1</pre><p>').replace(/`([^`]+)`/g,'<code>$1</code>').replace(/\n/g,'<br>') + '</p>'; }

        function closeModal(name) {
            document.getElementById(`modal-${name}`).classList.add('hidden');
        }

        // ========== Dashboard ==========
        async function loadDashboard() {
            try {
                const health = await api('/health');
                document.getElementById('stat-version').textContent = health.version || '-';
                document.getElementById('stat-gateway').innerHTML = '&#x25cf;';
                document.getElementById('stat-gateway').style.color = 'var(--success)';
                document.getElementById('stat-agents').textContent = health.agents?.length || 0;
                document.getElementById('stat-sessions').textContent = health.active_sessions || 0;
                document.getElementById('status-dot').className = 'status-dot online';
                document.getElementById('gateway-status').textContent = 'Online';
                document.getElementById('gateway-status').className = 'badge badge-success';
                document.getElementById('version-info').textContent = 'v' + (health.version || '0.1.0');
                document.getElementById('settings-version').textContent = health.version || '-';
                document.getElementById('settings-sessions').textContent = health.active_sessions || 0;
                document.getElementById('settings-agents').textContent = health.agents?.length || 0;

                agents = health.agents || [];
                renderAgentsTable();

                const pending = await api('/api/gateway/pending').catch(() => ({ count: 0 }));
                document.getElementById('stat-pending').textContent = pending.count > 0 ? `+${pending.count} pending` : '';

                const vault = await api('/api/credentials/status').catch(() => ({}));
                document.getElementById('stat-vault').textContent = !vault.initialized ? 'Init' : (vault.locked ? '&#x1f512;' : '&#x1f513;');
                document.getElementById('stat-vault-info').textContent = vault.initialized ? `${vault.endpoint_count || 0} endpoints` : 'Not initialized';

                setStatus('Gateway online');
                document.getElementById('statusbar-right').textContent = `${health.agents?.length || 0} agents | ${health.active_sessions || 0} sessions`;
            } catch (e) {
                document.getElementById('stat-gateway').innerHTML = '&#x25cb;';
                document.getElementById('stat-gateway').style.color = 'var(--error)';
                document.getElementById('status-dot').className = 'status-dot offline';
                document.getElementById('gateway-status').textContent = 'Offline';
                document.getElementById('gateway-status').className = 'badge badge-error';
                document.getElementById('settings-gateway-status').textContent = 'Stopped';
                document.getElementById('settings-gateway-status').className = 'badge badge-error';
                setStatus('Gateway offline');
            }
        }

        function renderAgentsTable() {
            const tbody = document.getElementById('agents-table');
            if (agents.length === 0) {
                tbody.innerHTML = '<tr><td colspan="4" class="text-dim">No agents configured</td></tr>';
                return;
            }
            tbody.innerHTML = agents.map(a => {
                const id = a.agent_id || a.id || a;
                const status = a.status || (a.llm_healthy ? 'active' : 'error');
                const badgeClass = status === 'active' ? 'badge-success' : status === 'pending' ? 'badge-warning' : status === 'error' ? 'badge-error' : 'badge-info';
                return `<tr>
                    <td>${escapeHtml(id)}</td>
                    <td class="text-dim">${escapeHtml(a.model || '-')}</td>
                    <td>${a.session_count || 0}</td>
                    <td><span class="badge ${badgeClass}">${status}</span></td>
                </tr>`;
            }).join('');
        }

        async function reloadAgents() {
            try {
                const res = await api('/api/gateway/reload', 'POST');
                toast(`Reloaded: ${res.agents_created || 0} created, ${res.agents_pending || 0} pending`);
                loadDashboard();
            } catch (e) { toast(e.message, true); }
        }

        // ========== Credentials ==========
        async function loadCredentials() {
            try {
                const status = await api('/api/credentials/status');

                // Hide all gate sections
                document.getElementById('vault-init-section').classList.add('hidden');
                document.getElementById('vault-unlock-section').classList.add('hidden');
                document.getElementById('vault-disabled-section').classList.add('hidden');
                document.getElementById('vault-content').classList.add('hidden');
                document.getElementById('cred-header-actions').innerHTML = '';

                if (!status.enabled) {
                    document.getElementById('vault-disabled-section').classList.remove('hidden');
                    return;
                }
                if (!status.initialized) {
                    document.getElementById('vault-init-section').classList.remove('hidden');
                    return;
                }
                if (status.locked) {
                    document.getElementById('vault-unlock-section').classList.remove('hidden');
                    return;
                }

                document.getElementById('vault-content').classList.remove('hidden');
                loadEndpoints();
                loadCredentialSchemas();
                loadPermissions();
                loadAuditLog();
            } catch (e) { toast(e.message, true); }
        }

        function showCredTab(tab) {
            currentCredTab = tab;
            document.querySelectorAll('.tab-item[data-credtab]').forEach(t => t.classList.remove('active'));
            document.querySelector(`[data-credtab="${tab}"]`)?.classList.add('active');
            document.querySelectorAll('.credtab-content').forEach(c => c.classList.add('hidden'));
            document.getElementById(`credtab-${tab}`)?.classList.remove('hidden');
        }

        // --- Endpoints ---
        async function loadEndpoints() {
            try {
                endpoints = await api('/api/credentials/endpoints');
                renderEndpointList();
            } catch (e) { endpoints = []; renderEndpointList(); }
        }

        function renderEndpointList() {
            const container = document.getElementById('endpoint-list');
            if (endpoints.length === 0) {
                container.innerHTML = '<p class="text-dim" style="padding:1rem">No endpoints configured. Click "+ Add" to get started.</p>';
                return;
            }
            container.innerHTML = endpoints.map((ep, i) => {
                const active = i === selectedEndpointIdx;
                return `<div class="list-item ${active ? 'active' : ''}" onclick="selectEndpoint(${i})">
                    <div class="title"><span class="cred-dot configured"></span>${escapeHtml(ep.name)}</div>
                    <div class="meta">${escapeHtml(ep.endpoint_type)} &bull; ${escapeHtml(ep.base_url || '')}</div>
                </div>`;
            }).join('');
        }

        function selectEndpoint(idx) {
            selectedEndpointIdx = idx;
            renderEndpointList();
            const ep = endpoints[idx];
            if (!ep) return;
            document.getElementById('endpoint-details').innerHTML = `
                <div class="detail-field"><label>ID</label><div class="val text-dim" style="font-family:monospace;font-size:0.8rem">${escapeHtml(ep.id)}</div></div>
                <div class="detail-field"><label>Name</label><div class="val">${escapeHtml(ep.name)}</div></div>
                <div class="detail-field"><label>Type</label><div class="val"><span class="badge badge-info">${escapeHtml(ep.endpoint_type)}</span></div></div>
                <div class="detail-field"><label>URL</label><div class="val">${escapeHtml(ep.base_url || '-')}</div></div>
                <div class="detail-field"><label>Created</label><div class="val text-dim">${ep.created_at || '-'}</div></div>
                <div class="flex gap-1 mt-2">
                    <button class="btn btn-primary btn-sm" onclick="showEditEndpoint('${ep.id}')">Edit</button>
                    <button class="btn btn-danger btn-sm" onclick="deleteEndpoint('${ep.id}')">Delete</button>
                </div>
            `;
        }

        async function initializeVault() {
            const pw = document.getElementById('init-password').value;
            const confirm_pw = document.getElementById('init-password-confirm').value;
            if (pw.length < 8) { toast('Password must be at least 8 characters', true); return; }
            if (pw !== confirm_pw) { toast('Passwords do not match', true); return; }
            try {
                await api('/api/credentials/init', 'POST', { method: 'password', password: pw });
                toast('Vault initialized');
                loadCredentials();
            } catch (e) { toast(e.message, true); }
        }

        async function unlockVault() {
            try {
                await api('/api/credentials/unlock', 'POST', { password: document.getElementById('unlock-password').value });
                toast('Vault unlocked');
                loadCredentials();
            } catch (e) { toast(e.message, true); }
        }

        async function lockVault() {
            try { await api('/api/credentials/lock', 'POST'); toast('Vault locked'); loadCredentials(); }
            catch (e) { toast(e.message, true); }
        }

        // Endpoint type configs
        const ENDPOINT_CONFIGS = {
            home_assistant: [
                { id: 'url', label: 'Home Assistant URL', placeholder: 'http://homeassistant.local:8123', required: true },
                { id: 'token', label: 'Long-Lived Access Token', type: 'password', placeholder: 'eyJ0eXAi...', required: true }
            ],
            generic_rest: [
                { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com', required: true },
                { id: 'token', label: 'Bearer Token', type: 'password', placeholder: 'Your token' }
            ],
            generic_oauth2: [
                { id: 'url', label: 'API Base URL', placeholder: 'https://api.example.com', required: true },
                { id: 'auth_url', label: 'Authorization URL', placeholder: 'https://auth.example.com/authorize' },
                { id: 'token_url', label: 'Token URL', placeholder: 'https://auth.example.com/token' },
                { id: 'client_id', label: 'Client ID' },
                { id: 'client_secret', label: 'Client Secret', type: 'password' },
                { id: 'scopes', label: 'Scopes', placeholder: 'read write offline_access' },
                { id: 'redirect_uri', label: 'Redirect URI', placeholder: 'http://localhost:18080/oauth/callback' }
            ],
            api_key_service: [
                { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com', required: true },
                { id: 'api_key', label: 'API Key', type: 'password', required: true },
                { id: 'header_name', label: 'Header Name', placeholder: 'X-API-Key' }
            ],
            basic_auth_service: [
                { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com', required: true },
                { id: 'username', label: 'Username', required: true },
                { id: 'password', label: 'Password', type: 'password', required: true }
            ],
            bearer_token: [
                { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com', required: true },
                { id: 'token', label: 'Token', type: 'password', required: true }
            ]
        };

        function showAddEndpoint() {
            editingEndpointId = null;
            document.getElementById('endpoint-modal-title').textContent = 'Add Service Endpoint';
            document.getElementById('endpoint-save-btn').textContent = 'Save';
            document.getElementById('endpoint-name').value = '';
            document.getElementById('endpoint-type').value = 'home_assistant';
            document.getElementById('endpoint-type-group').classList.remove('hidden');
            updateEndpointForm();
            document.getElementById('modal-endpoint').classList.remove('hidden');
        }

        function showEditEndpoint(id) {
            const ep = endpoints.find(e => e.id === id);
            if (!ep) return;
            editingEndpointId = id;
            document.getElementById('endpoint-modal-title').textContent = 'Edit Endpoint: ' + ep.name;
            document.getElementById('endpoint-save-btn').textContent = 'Update';
            document.getElementById('endpoint-name').value = ep.name;
            document.getElementById('endpoint-type').value = ep.endpoint_type;
            document.getElementById('endpoint-type-group').classList.add('hidden');
            updateEndpointForm();
            // Pre-fill URL
            const urlField = document.getElementById('field-url');
            if (urlField) urlField.value = ep.base_url || '';
            document.getElementById('modal-endpoint').classList.remove('hidden');
        }

        function updateEndpointForm() {
            const type = document.getElementById('endpoint-type').value;
            const container = document.getElementById('endpoint-form-dynamic');
            container.innerHTML = (ENDPOINT_CONFIGS[type] || []).map(f => `
                <div class="form-group">
                    <label class="${f.required ? 'required' : ''}">${f.label}</label>
                    <input type="${f.type || 'text'}" id="field-${f.id}" placeholder="${f.placeholder || ''}">
                </div>
            `).join('');
        }

        async function saveEndpoint() {
            const name = document.getElementById('endpoint-name').value.trim();
            const type = document.getElementById('endpoint-type').value;
            if (!name) { toast('Name is required', true); return; }

            const url = document.getElementById('field-url')?.value;
            if (!url) { toast('URL is required', true); return; }

            // Collect all dynamic fields
            const fields = ENDPOINT_CONFIGS[type] || [];
            const secretField = fields.find(f => f.type === 'password');
            const secretValue = secretField ? document.getElementById(`field-${secretField.id}`)?.value : null;

            try {
                if (editingEndpointId) {
                    // For edit, we delete and recreate (API doesn't have PATCH)
                    await api(`/api/credentials/endpoints/${editingEndpointId}`, 'DELETE');
                }
                const ep = await api('/api/credentials/endpoints', 'POST', { name, endpoint_type: type, base_url: url });

                // Store credential if secret field is filled
                if (secretValue && ep.id) {
                    const encoded = btoa(secretValue);
                    await api(`/api/credentials/endpoints/${ep.id}/credential`, 'POST', {
                        credential_type: 'bearer_token',
                        secret: encoded
                    }).catch(() => {}); // Best effort
                }

                closeModal('endpoint');
                toast(editingEndpointId ? 'Endpoint updated' : 'Endpoint added');
                loadEndpoints();
            } catch (e) { toast(e.message, true); }
        }

        async function deleteEndpoint(id) {
            if (!confirm('Delete this endpoint?')) return;
            try {
                await api(`/api/credentials/endpoints/${id}`, 'DELETE');
                toast('Endpoint deleted');
                selectedEndpointIdx = null;
                document.getElementById('endpoint-details').innerHTML = '<p class="text-dim">Select an endpoint to view details</p>';
                loadEndpoints();
            } catch (e) { toast(e.message, true); }
        }

        // --- Credential Schemas (Providers sub-tab) ---
        async function loadCredentialSchemas() {
            try {
                const res = await api('/api/credentials/schemas');
                credentialSchemas = res.schemas || [];
                renderSchemaCategories();
                renderSchemaProviders();
            } catch (e) {
                credentialSchemas = [];
                renderSchemaCategories();
                renderSchemaProviders();
            }
        }

        function renderSchemaCategories() {
            const container = document.getElementById('schema-category-list');
            container.innerHTML = SCHEMA_CATEGORIES.map((cat, i) => {
                const count = cat.filter
                    ? credentialSchemas.filter(s => s.category === cat.filter).length
                    : credentialSchemas.length;
                return `<div class="list-item ${i === selectedSchemaCategory ? 'active' : ''}" onclick="selectSchemaCategory(${i})">
                    <div class="title">${cat.icon} ${cat.label}</div>
                    <div class="meta">${count} provider${count !== 1 ? 's' : ''}</div>
                </div>`;
            }).join('');
        }

        function selectSchemaCategory(idx) {
            selectedSchemaCategory = idx;
            renderSchemaCategories();
            renderSchemaProviders();
        }

        function renderSchemaProviders() {
            const container = document.getElementById('schema-provider-list');
            const cat = SCHEMA_CATEGORIES[selectedSchemaCategory];
            const filtered = cat.filter
                ? credentialSchemas.filter(s => s.category === cat.filter)
                : credentialSchemas;

            if (filtered.length === 0) {
                container.innerHTML = '<p class="text-dim" style="padding:1rem">Start the gateway to see providers</p>';
                return;
            }

            container.innerHTML = filtered.map(s => {
                const catIcon = s.category === 'model' ? '&#x1f9e0;' : s.category === 'communication' ? '&#x1f4ac;' : '&#x1f527;';
                const configured = endpoints.some(ep => ep.name?.toLowerCase().includes(s.provider_id) || ep.endpoint_type?.includes(s.provider_id));
                return `<div class="list-item" onclick="showSchemaProviderConfig('${escapeHtml(s.provider_id)}')">
                    <div class="title"><span class="cred-dot ${configured ? 'configured' : 'unconfigured'}"></span>${catIcon} ${escapeHtml(s.provider_name)}</div>
                    <div class="meta">${escapeHtml(s.provider_id)} &bull; ${(s.auth_methods || []).length} auth method${(s.auth_methods || []).length !== 1 ? 's' : ''}</div>
                </div>`;
            }).join('');
        }

        function showSchemaProviderConfig(providerId) {
            const schema = credentialSchemas.find(s => s.provider_id === providerId);
            if (!schema || !schema.auth_methods || schema.auth_methods.length === 0) {
                toast('No auth configuration available for this provider', true);
                return;
            }

            document.getElementById('provider-modal-title').textContent = 'Configure ' + schema.provider_name;

            let html = '';
            if (schema.auth_methods.length > 1) {
                html += `<div class="form-group"><label>Auth Method</label><select id="schema-auth-method" onchange="updateSchemaFields('${providerId}')">`;
                schema.auth_methods.forEach((m, i) => {
                    html += `<option value="${i}">${escapeHtml(m.label || m.id)}</option>`;
                });
                html += '</select></div>';
            }
            html += `<div id="schema-fields"></div>`;
            if (schema.auth_methods[0]?.hint) {
                html += `<div class="config-hint"><h4>Setup</h4><p class="text-dim">${escapeHtml(schema.auth_methods[0].hint)}</p></div>`;
            }
            if (schema.auth_methods[0]?.docs_url) {
                html += `<p class="mt-1"><a href="${escapeHtml(schema.auth_methods[0].docs_url)}" target="_blank" style="color:var(--accent)">Documentation &rarr;</a></p>`;
            }

            document.getElementById('provider-modal-content').innerHTML = html;
            document.getElementById('provider-modal-content').dataset.providerId = providerId;
            updateSchemaFields(providerId);
            document.getElementById('modal-provider').classList.remove('hidden');
        }

        function updateSchemaFields(providerId) {
            const schema = credentialSchemas.find(s => s.provider_id === providerId);
            if (!schema) return;
            const methodIdx = parseInt(document.getElementById('schema-auth-method')?.value || '0');
            const method = schema.auth_methods[methodIdx];
            if (!method || !method.fields) return;

            const container = document.getElementById('schema-fields');
            container.innerHTML = method.fields.map(f => `
                <div class="form-group">
                    <label class="${f.required ? 'required' : ''}">${escapeHtml(f.label || f.id)}</label>
                    <input type="${f.secret ? 'password' : 'text'}" id="schema-field-${f.id}"
                        placeholder="${escapeHtml(f.placeholder || f.hint || '')}"
                        ${f.default_value ? `value="${escapeHtml(f.default_value)}"` : ''}>
                    ${f.hint ? `<div class="hint">${escapeHtml(f.hint)}</div>` : ''}
                </div>
            `).join('');
        }

        async function saveProviderConfig() {
            const providerId = document.getElementById('provider-modal-content').dataset.providerId;
            const schema = credentialSchemas.find(s => s.provider_id === providerId);
            if (!schema) return;

            const methodIdx = parseInt(document.getElementById('schema-auth-method')?.value || '0');
            const method = schema.auth_methods[methodIdx];
            if (!method) return;

            // Collect field values
            const secretField = (method.fields || []).find(f => f.secret);
            const urlField = (method.fields || []).find(f => f.id === 'base_url' || f.id === 'url' || f.id === 'endpoint');
            const secretValue = secretField ? document.getElementById(`schema-field-${secretField.id}`)?.value : null;
            const urlValue = urlField ? document.getElementById(`schema-field-${urlField.id}`)?.value : null;

            try {
                // Create endpoint for this provider
                const ep = await api('/api/credentials/endpoints', 'POST', {
                    name: schema.provider_name,
                    endpoint_type: 'api_key_service',
                    base_url: urlValue || `${providerId}://configured`
                });

                // Store the secret if available
                if (secretValue && ep.id) {
                    await api(`/api/credentials/endpoints/${ep.id}/credential`, 'POST', {
                        credential_type: 'bearer_token',
                        secret: btoa(secretValue)
                    }).catch(() => {});
                }

                closeModal('provider');
                toast(`${schema.provider_name} configured`);
                loadEndpoints();
                loadCredentialSchemas();
            } catch (e) { toast(e.message, true); }
        }

        // --- Permissions sub-tab ---
        async function loadPermissions() {
            try {
                permissions = await api('/api/credentials/permissions');
                renderPermissions();
            } catch (e) { permissions = []; renderPermissions(); }
        }

        function renderPermissions() {
            const tbody = document.getElementById('permissions-table');
            if (!Array.isArray(permissions) || permissions.length === 0) {
                tbody.innerHTML = '<tr><td colspan="4" class="text-dim">No permission rules configured. Click "+ Add Rule" to create one.</td></tr>';
                return;
            }
            tbody.innerHTML = permissions.map(p => {
                const levelBadge = p.level === 'allow' ? 'badge-success' :
                    p.level === 'deny' ? 'badge-error' : 'badge-warning';
                return `<tr>
                    <td style="font-family:monospace">${escapeHtml(p.path_pattern)}</td>
                    <td><span class="badge ${levelBadge}">${escapeHtml(p.level)}</span></td>
                    <td class="text-dim">${escapeHtml(p.description || '-')}</td>
                    <td><button class="btn btn-danger btn-sm" onclick="deletePermission('${p.id}')">Delete</button></td>
                </tr>`;
            }).join('');
        }

        function showAddPermission() {
            document.getElementById('perm-path').value = '';
            document.getElementById('perm-level').value = 'allow';
            document.getElementById('perm-description').value = '';
            document.getElementById('modal-permission').classList.remove('hidden');
        }

        async function savePermission() {
            const path = document.getElementById('perm-path').value.trim();
            if (!path) { toast('Path pattern is required', true); return; }
            try {
                await api('/api/credentials/permissions', 'POST', {
                    path_pattern: path,
                    level: document.getElementById('perm-level').value,
                    description: document.getElementById('perm-description').value.trim() || null
                });
                closeModal('permission');
                toast('Permission rule added');
                loadPermissions();
            } catch (e) { toast(e.message, true); }
        }

        async function deletePermission(id) {
            if (!confirm('Delete this permission rule?')) return;
            try {
                await api(`/api/credentials/permissions/${id}`, 'DELETE');
                toast('Permission deleted');
                loadPermissions();
            } catch (e) { toast(e.message, true); }
        }

        // --- Audit sub-tab ---
        async function loadAuditLog() {
            try {
                auditEntries = await api('/api/credentials/audit?limit=100');
                renderAuditLog();
            } catch (e) { auditEntries = []; renderAuditLog(); }
        }

        function renderAuditLog() {
            const tbody = document.getElementById('audit-table');
            const entries = Array.isArray(auditEntries) ? auditEntries : [];
            if (entries.length === 0) {
                tbody.innerHTML = '<tr><td colspan="5" class="text-dim">No audit entries recorded yet.</td></tr>';
                return;
            }
            tbody.innerHTML = entries.map(e => {
                const resultBadge = e.result === 'allowed' || e.result === 'success'
                    ? 'badge-success' : e.result === 'denied' ? 'badge-error' : 'badge-warning';
                return `<tr>
                    <td class="text-dim" style="font-size:0.8rem">${escapeHtml(e.timestamp || e.created_at || '-')}</td>
                    <td>${escapeHtml(e.action || e.operation || '-')}</td>
                    <td style="font-family:monospace;font-size:0.8rem">${escapeHtml(e.path || e.credential_path || '-')}</td>
                    <td class="text-dim">${escapeHtml(e.agent_id || '-')}</td>
                    <td><span class="badge ${resultBadge}">${escapeHtml(e.result || e.outcome || '-')}</span></td>
                </tr>`;
            }).join('');
        }

        // ========== Agents Page ==========
        async function loadAgentsPage() {
            try {
                allAgents = await api('/api/agents');
                // Also update agents from health for dashboard
                loadDashboard().catch(() => {});
            } catch (e) {
                allAgents = [];
            }
            renderAgentList();
        }

        function renderAgentList() {
            const container = document.getElementById('agent-list');
            if (allAgents.length === 0) {
                container.innerHTML = '<p class="text-dim" style="padding:1rem">No agents configured. Click "Create Agent" to get started.</p>';
                return;
            }

            const topLevel = allAgents.filter(a => !a.parent_id);
            const subOf = pid => allAgents.filter(a => a.parent_id === pid);

            let html = '';
            const renderItem = (a, indent) => {
                const idx = allAgents.indexOf(a);
                const id = a.id || a;
                const status = a.status || 'active';
                const dotClass = status === 'active' ? 'configured' : 'unconfigured';
                return `<div class="list-item ${idx === selectedAgentIdx ? 'active' : ''}" onclick="selectAgent(${idx})">
                    <div class="title"><span class="cred-dot ${dotClass}"></span>${indent ? '&nbsp;&nbsp;&#x2514; ' : ''}${escapeHtml(id)}</div>
                    <div class="meta">${escapeHtml(a.model || 'default')}${a.parent_id ? ' (subagent)' : ''}</div>
                </div>`;
            };
            topLevel.forEach(a => {
                html += renderItem(a, false);
                subOf(a.id || a).forEach(sub => { html += renderItem(sub, true); });
            });
            // Orphan subagents
            allAgents.filter(a => a.parent_id && !topLevel.some(t => (t.id || t) === a.parent_id)).forEach(a => {
                html += renderItem(a, true);
            });
            container.innerHTML = html;
        }

        function selectAgent(idx) {
            selectedAgentIdx = idx;
            renderAgentList();
            const agent = allAgents[idx];
            if (!agent) { document.getElementById('agent-details').innerHTML = '<p class="text-dim">Select an agent</p>'; return; }
            const id = agent.id || agent;
            const subs = allAgents.filter(a => a.parent_id === id).map(a => a.id || a);
            const status = agent.status || 'active';
            const statusBadge = status === 'pending'
                ? `<span class="badge badge-warning">Pending</span>${agent.reason ? `<div class="hint">${escapeHtml(agent.reason)}</div>` : ''}`
                : status === 'error' ? '<span class="badge badge-error">Error</span>'
                : agent.enabled === false ? '<span class="badge badge-error">Disabled</span>'
                : '<span class="badge badge-success">Active</span>';

            document.getElementById('agent-details').innerHTML = `
                <div class="detail-field"><label>ID</label><div class="val">${escapeHtml(id)}</div></div>
                <div class="detail-field"><label>Model</label><div class="val">${escapeHtml(agent.model || 'default')}</div></div>
                <div class="detail-field"><label>Status</label><div class="val">${statusBadge}</div></div>
                ${agent.parent_id ? `<div class="detail-field"><label>Parent</label><div class="val">${escapeHtml(agent.parent_id)} <span class="text-dim">(subagent)</span></div></div>` : ''}
                ${subs.length > 0 ? `<div class="detail-field"><label>Subagents</label><div class="val">${subs.map(s => '<span class="badge badge-info" style="margin-right:4px">' + escapeHtml(s) + '</span>').join('')}</div></div>` : ''}
                ${agent.workspace ? `<div class="detail-field"><label>Workspace</label><div class="val text-dim" style="font-size:0.85rem">${escapeHtml(agent.workspace)}</div></div>` : ''}
                <div class="detail-field"><label>Max Tool Calls</label><div class="val">${agent.max_tool_calls || 'dynamic'}</div></div>
                <div class="detail-field"><label>Temperature</label><div class="val">${agent.temperature != null ? agent.temperature : '0.3'}</div></div>
                <div class="detail-field"><label>Max Tokens</label><div class="val">${agent.max_tokens || 16000}</div></div>
                <div class="detail-field"><label>Sessions</label><div class="val">${agent.session_count || 0}</div></div>
                ${agent.system_prompt ? `<div class="detail-field"><label>System Prompt</label><div style="background:var(--surface-2);padding:0.75rem;border-radius:6px;font-size:0.85rem;max-height:120px;overflow-y:auto;white-space:pre-wrap">${escapeHtml(agent.system_prompt)}</div></div>` : ''}
                <div class="flex gap-1 mt-2">
                    <button class="btn btn-primary btn-sm" onclick="showEditAgent('${escapeHtml(id)}')">Edit</button>
                    <button class="btn btn-secondary btn-sm" onclick="showCreateSubagent('${escapeHtml(id)}')">+ Subagent</button>
                    <button class="btn btn-danger btn-sm" onclick="deleteAgent('${escapeHtml(id)}')">Delete</button>
                </div>
            `;
        }

        function showCreateAgent() {
            editingAgentId = null;
            document.getElementById('agent-modal-title').textContent = 'Create Agent';
            document.getElementById('agent-save-btn').textContent = 'Create';
            document.getElementById('agent-id').value = '';
            document.getElementById('agent-id').readOnly = false;
            document.getElementById('agent-model').value = '';
            document.getElementById('agent-workspace').value = '';
            document.getElementById('agent-max-tools').value = '';
            document.getElementById('agent-temperature').value = '0.3';
            document.getElementById('agent-max-tokens').value = '16000';
            document.getElementById('agent-system-prompt').value = '';
            populateParentSelect('');
            document.getElementById('agent-subagents-info').classList.add('hidden');
            document.getElementById('modal-agent').classList.remove('hidden');
        }

        function showCreateSubagent(parentId) {
            showCreateAgent();
            document.getElementById('agent-parent').value = parentId;
        }

        function showEditAgent(id) {
            const agent = allAgents.find(a => (a.id || a) === id);
            if (!agent) return;
            editingAgentId = id;
            document.getElementById('agent-modal-title').textContent = 'Edit Agent: ' + id;
            document.getElementById('agent-save-btn').textContent = 'Save';
            document.getElementById('agent-id').value = id;
            document.getElementById('agent-id').readOnly = true;
            document.getElementById('agent-model').value = agent.model || '';
            document.getElementById('agent-workspace').value = agent.workspace || '';
            document.getElementById('agent-max-tools').value = agent.max_tool_calls || '';
            document.getElementById('agent-temperature').value = agent.temperature != null ? agent.temperature : 0.3;
            document.getElementById('agent-max-tokens').value = agent.max_tokens || 16000;
            document.getElementById('agent-system-prompt').value = agent.system_prompt || '';
            populateParentSelect(agent.parent_id || '');
            const subs = allAgents.filter(a => a.parent_id === id);
            if (subs.length > 0) {
                document.getElementById('agent-subagents-list').textContent = subs.map(a => a.id || a).join(', ');
                document.getElementById('agent-subagents-info').classList.remove('hidden');
            } else {
                document.getElementById('agent-subagents-info').classList.add('hidden');
            }
            document.getElementById('modal-agent').classList.remove('hidden');
        }

        function populateParentSelect(selected) {
            const sel = document.getElementById('agent-parent');
            sel.innerHTML = '<option value="">None (top-level agent)</option>' +
                allAgents.filter(a => (a.id || a) !== editingAgentId)
                    .map(a => `<option value="${a.id || a}" ${(a.id || a) === selected ? 'selected' : ''}>${a.id || a}</option>`)
                    .join('');
        }

        async function saveAgent() {
            const id = document.getElementById('agent-id').value.trim();
            if (!id) { toast('Agent ID is required', true); return; }
            if (/[\s\/]/.test(id)) { toast('Agent ID cannot contain spaces or slashes', true); return; }

            const data = {
                id,
                model: document.getElementById('agent-model').value.trim() || null,
                parent_id: document.getElementById('agent-parent').value || null,
                workspace: document.getElementById('agent-workspace').value.trim() || null,
                max_tool_calls: parseInt(document.getElementById('agent-max-tools').value) || null,
                temperature: parseFloat(document.getElementById('agent-temperature').value) || 0.3,
                max_tokens: parseInt(document.getElementById('agent-max-tokens').value) || 16000,
                system_prompt: document.getElementById('agent-system-prompt').value.trim() || null,
            };

            try {
                if (editingAgentId) {
                    await api(`/api/agents/${editingAgentId}`, 'PUT', data);
                    toast('Agent updated');
                } else {
                    await api('/api/agents', 'POST', data);
                    toast('Agent created');
                }
                closeModal('agent');
                await loadAgentsPage();
            } catch (e) { toast(e.message, true); }
        }

        async function deleteAgent(id) {
            if (!confirm(`Delete agent "${id}"?`)) return;
            try {
                await api(`/api/agents/${id}`, 'DELETE');
                toast('Agent deleted');
                selectedAgentIdx = null;
                document.getElementById('agent-details').innerHTML = '<p class="text-dim">Select an agent to view details</p>';
                await loadAgentsPage();
            } catch (e) { toast(e.message, true); }
        }

        function refreshAgents() { loadAgentsPage(); }

        // ========== Sessions Page ==========
        async function loadSessionsPage() {
            try {
                const data = await api('/api/sessions');
                sessions = data.sessions || data || [];
                if (!Array.isArray(sessions)) sessions = [];
                renderSessionList();
                // Populate agent dropdown
                const select = document.getElementById('chat-agent');
                select.innerHTML = '<option value="">New Chat...</option>' +
                    allAgents.map(a => `<option value="${a.id || a}">${a.id || a}</option>`).join('');
            } catch (e) {
                sessions = [];
                renderSessionList();
            }
        }

        function renderSessionList() {
            const container = document.getElementById('session-list');
            if (sessions.length === 0) {
                container.innerHTML = '<p class="text-dim" style="padding:1rem">No active sessions. Select an agent and start chatting.</p>';
                return;
            }
            container.innerHTML = sessions.map((s, i) => `
                <div class="list-item ${s.key === selectedSession ? 'active' : ''}" onclick="selectSession('${escapeHtml(s.key)}')">
                    <div class="title">${escapeHtml(s.agent_id || '-')}</div>
                    <div class="meta"><span style="color:var(--accent)">[${escapeHtml(s.channel || 'web')}]</span> ${s.message_count || 0} msgs</div>
                </div>
            `).join('');
        }

        function selectSession(key) {
            selectedSession = key;
            sessionKey = key;
            renderSessionList();
            loadChatHistory(key);
        }

        async function loadChatHistory(key) {
            const messages = document.getElementById('chat-messages');
            messages.innerHTML = '<div class="message assistant"><p class="text-dim">Loading...</p></div>';
            try {
                const data = await api(`/api/sessions/${key}/history`);
                const msgs = data.messages || [];
                messages.innerHTML = msgs.length === 0
                    ? '<div class="message assistant"><p>No messages yet. Type below to start.</p></div>'
                    : msgs.map(m => `<div class="message ${m.role}">${formatMessage(m.content)}</div>`).join('');
                messages.scrollTop = messages.scrollHeight;
            } catch (e) {
                messages.innerHTML = '<div class="message assistant"><p class="text-dim">Start typing to begin a conversation.</p></div>';
            }
        }

        async function sendMessage() {
            const input = document.getElementById('chat-input');
            const agentSelect = document.getElementById('chat-agent');
            const message = input.value.trim();
            if (!message) return;

            const agent = agentSelect.value || allAgents[0]?.id || 'default';
            const messages = document.getElementById('chat-messages');

            messages.innerHTML += `<div class="message user"><p>${escapeHtml(message)}</p></div>`;
            input.value = '';
            messages.scrollTop = messages.scrollHeight;

            // Show thinking indicator
            const thinkingId = 'thinking-' + Date.now();
            messages.innerHTML += `<div class="message assistant" id="${thinkingId}"><p class="text-dim">Thinking...</p></div>`;
            messages.scrollTop = messages.scrollHeight;

            try {
                const res = await api(`/api/agents/${agent}/message`, 'POST', { session_key: sessionKey, message });
                document.getElementById(thinkingId)?.remove();
                const content = res.message?.content || res.content || res.message || JSON.stringify(res);
                messages.innerHTML += `<div class="message assistant">${formatMessage(typeof content === 'string' ? content : JSON.stringify(content))}</div>`;
                if (res.tokens_used) {
                    setStatus(`Tokens: ${res.tokens_used.total_tokens || 0} | ${res.processing_time_ms || 0}ms`);
                }
            } catch (e) {
                document.getElementById(thinkingId)?.remove();
                messages.innerHTML += `<div class="message assistant"><p class="text-error">Error: ${escapeHtml(e.message)}</p></div>`;
            }
            messages.scrollTop = messages.scrollHeight;
        }

        // ========== Models Page ==========
        async function loadModelsPage() {
            try {
                const res = await api('/api/providers');
                providers = res.providers || [];
                renderProviderList();
                if (providers.length > 0 && selectedProviderIdx === null) {
                    selectProvider(0);
                }
            } catch (e) {
                providers = [];
                renderProviderList();
                document.getElementById('provider-details').innerHTML = '<p class="text-dim">Failed to load providers. Is the gateway running?</p>';
            }
        }

        function renderProviderList() {
            const container = document.getElementById('provider-list');
            if (providers.length === 0) {
                container.innerHTML = '<p class="text-dim" style="padding:1rem">No providers loaded. Make sure the gateway is running.</p>';
                return;
            }
            container.innerHTML = providers.map((p, i) => `
                <div class="list-item ${i === selectedProviderIdx ? 'active' : ''}" onclick="selectProvider(${i})">
                    <div class="title"><span class="cred-dot ${p.available ? 'configured' : 'unconfigured'}"></span>${escapeHtml(p.name)}</div>
                    <div class="meta">${escapeHtml(p.id)} &bull; ${(p.models || []).length} models</div>
                </div>
            `).join('');
        }

        function selectProvider(idx) {
            selectedProviderIdx = idx;
            renderProviderList();
            const p = providers[idx];
            if (!p) return;

            // Capabilities
            const cap = (name, supported) =>
                `<span class="capability ${supported ? 'yes' : 'no'}">${supported ? '&#x2713;' : '&#x2717;'} ${name}</span>`;

            // Auth type display
            const authLabels = {
                'aws_credentials': 'AWS Credentials',
                'oauth': 'OAuth (Claude Code)',
                'api_key': 'API Key',
                'none': 'None required'
            };
            const authLabel = authLabels[p.auth_type] || p.auth_type || 'Unknown';

            // Models list (up to 8)
            const models = (p.models || []).slice(0, 8);
            const moreCount = (p.models || []).length - 8;
            const modelsList = models.map(m => {
                const ctx = m.context_window ? ` &bull; ${Math.round(m.context_window / 1000)}k ctx` : '';
                const out = m.max_output_tokens ? ` &bull; ${m.max_output_tokens} max out` : '';
                return `<li style="padding:0.25rem 0;font-size:0.85rem">${escapeHtml(m.name || m.id)}${ctx}${out}</li>`;
            }).join('');

            // Configuration hints
            let configHint = '';
            if (p.auth_type === 'aws_credentials') {
                configHint = `<div class="config-hint"><h4>Configuration</h4>
                    <p class="text-dim mb-1">Set AWS credentials via environment variables or AWS config:</p>
                    <pre>export AWS_ACCESS_KEY_ID=your_key\nexport AWS_SECRET_ACCESS_KEY=your_secret\nexport AWS_REGION=us-east-1</pre>
                    <p class="text-dim mt-1">Or use IAM roles, <code>~/.aws/credentials</code>, or SSO.</p>
                </div>`;
            } else if (p.auth_type === 'oauth') {
                configHint = `<div class="config-hint"><h4>Configuration</h4>
                    <p class="text-dim mb-1">Authenticate via Claude Code OAuth:</p>
                    <pre>npm install -g @anthropic-ai/claude-code\nclaude login</pre>
                    <p class="text-dim mt-1">The gateway will use the cached OAuth session automatically.</p>
                </div>`;
            } else if (p.auth_type === 'api_key') {
                const envVar = p.id === 'openai' ? 'OPENAI_API_KEY' : `${p.id.toUpperCase()}_API_KEY`;
                configHint = `<div class="config-hint"><h4>Configuration</h4>
                    <p class="text-dim mb-1">Set your API key:</p>
                    <pre>export ${envVar}=your_api_key</pre>
                    <p class="text-dim mt-1">Or configure via the Credentials &rarr; Providers tab.</p>
                </div>`;
            } else if (p.auth_type === 'none') {
                configHint = `<div class="config-hint"><h4>Configuration</h4><p class="text-dim">No configuration needed.</p></div>`;
            }

            document.getElementById('provider-details').innerHTML = `
                <div class="detail-field"><label>Provider</label><div class="val" style="font-size:1.1rem;font-weight:600">${escapeHtml(p.name)}</div></div>
                <div class="detail-field"><label>ID</label><div class="val text-dim">${escapeHtml(p.id)}</div></div>
                <div class="detail-field"><label>Status</label><div class="val">${p.available ? '<span class="badge badge-success">&#x2713; Available</span>' : '<span class="badge badge-error">&#x25cb; Not Available</span>'}</div></div>
                <div class="detail-field"><label>Auth Type</label><div class="val">${escapeHtml(authLabel)}</div></div>
                <div class="detail-field"><label>Capabilities</label><div class="val">
                    ${cap('Streaming', p.supports_streaming)}
                    ${cap('Tool Use', p.supports_tools)}
                    ${cap('Vision', p.supports_vision)}
                </div></div>
                <div class="detail-field"><label>Models</label>
                    ${models.length > 0 ? `<ul style="list-style:none;padding:0;margin:0">${modelsList}${moreCount > 0 ? `<li class="text-dim" style="font-size:0.8rem">...and ${moreCount} more</li>` : ''}</ul>` : '<div class="text-dim">No models listed</div>'}
                </div>
                ${configHint}
                <div class="flex gap-1 mt-2">
                    <button class="btn btn-primary btn-sm" onclick="testProvider('${escapeHtml(p.id)}')">Test Connection</button>
                    <button class="btn btn-secondary btn-sm" onclick="showProviderEdit('${escapeHtml(p.id)}')">Configure</button>
                </div>
            `;
        }

        function showProviderEdit(id) {
            // Check if we have a schema for this provider
            const schema = credentialSchemas.find(s => s.provider_id === id);
            if (schema) {
                showSchemaProviderConfig(id);
            } else {
                // Simple fallback config
                document.getElementById('provider-modal-title').textContent = 'Configure ' + id;
                const p = providers.find(x => x.id === id);
                const authType = p?.auth_type || 'api_key';
                let html = '';
                if (authType === 'api_key') {
                    html = `<div class="form-group"><label>API Key</label><input type="password" id="schema-field-api_key" placeholder="Enter API key"></div>
                            <div class="form-group"><label>Base URL (optional)</label><input type="text" id="schema-field-base_url" placeholder="Leave empty for default"></div>`;
                } else if (authType === 'aws_credentials') {
                    html = `<div class="config-hint"><h4>AWS Credentials</h4><p class="text-dim">Configure via environment variables or <code>~/.aws/credentials</code>.</p>
                            <pre>export AWS_ACCESS_KEY_ID=your_key\nexport AWS_SECRET_ACCESS_KEY=your_secret\nexport AWS_REGION=us-east-1</pre></div>`;
                } else if (authType === 'oauth') {
                    html = `<div class="config-hint"><h4>OAuth</h4><p class="text-dim">Authenticate via Claude Code: <code>claude login</code></p></div>`;
                } else {
                    html = '<p class="text-dim">No additional configuration needed.</p>';
                }
                document.getElementById('provider-modal-content').innerHTML = html;
                document.getElementById('provider-modal-content').dataset.providerId = id;
                document.getElementById('modal-provider').classList.remove('hidden');
            }
        }

        async function testProvider(id) {
            try {
                const res = await api(`/api/providers/${id}/test`, 'POST');
                if (res.status === 'ok') {
                    toast(`${id}: connection OK (${res.models_found} models)`);
                } else {
                    toast(`${id}: ${res.error}`, true);
                }
            } catch (e) { toast(e.message, true); }
        }

        // ========== Settings ==========
        async function restartGateway() {
            if (!confirm('Restart the gateway?')) return;
            try {
                await api('/api/gateway/restart', 'POST');
                toast('Gateway restarting...');
            } catch (e) { toast(e.message, true); }
        }

        async function stopGateway() {
            if (!confirm('Stop the gateway?')) return;
            try {
                await api('/api/gateway/stop', 'POST');
                toast('Gateway stopped');
                loadDashboard();
            } catch (e) { toast(e.message, true); }
        }

        // ========== Init ==========
        loadDashboard();
        setInterval(loadDashboard, 15000);
        // Pre-load schemas for provider config
        api('/api/credentials/schemas').then(res => { credentialSchemas = res.schemas || []; }).catch(() => {});
    </script>
</body>
</html>"##
}
