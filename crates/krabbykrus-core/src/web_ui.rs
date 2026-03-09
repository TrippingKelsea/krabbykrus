//! Embedded Web UI for Krabbykrus Gateway
//!
//! Navigation synchronized with TUI:
//! - Dashboard (status overview)
//! - Credentials (vault management)
//! - Agents (configuration)
//! - Sessions (active sessions + chat)
//! - Models (provider config)
//! - Settings (gateway config)

/// Returns the main web UI HTML
pub fn get_dashboard_html() -> &'static str {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Krabbykrus Gateway</title>
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
        .main { flex: 1; overflow-y: auto; }
        .content { padding: 2rem; max-width: 1400px; margin: 0 auto; }
        
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
        }
        input:focus, select:focus, textarea:focus { outline: none; border-color: var(--primary); }
        .form-row { display: flex; gap: 1rem; }
        .form-row .form-group { flex: 1; }
        
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
        .badge { padding: 0.25rem 0.75rem; border-radius: 999px; font-size: 0.75rem; font-weight: 500; }
        .badge-success { background: rgba(16,185,129,0.2); color: var(--success); }
        .badge-warning { background: rgba(245,158,11,0.2); color: var(--warning); }
        .badge-error { background: rgba(239,68,68,0.2); color: var(--error); }
        .badge-info { background: rgba(124,58,237,0.2); color: var(--accent); }
        
        /* Page header */
        .page-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 2rem; }
        .page-header h1 { font-size: 1.5rem; }
        
        /* Split layout */
        .split { display: grid; grid-template-columns: 1fr 1fr; gap: 1.5rem; }
        .split-40-60 { grid-template-columns: 40% 60%; }
        .split-35-65 { grid-template-columns: 35% 65%; }
        
        /* Session list */
        .session-list { max-height: 400px; overflow-y: auto; }
        .session-item { padding: 1rem; border-bottom: 1px solid var(--border); cursor: pointer; }
        .session-item:hover { background: var(--surface-2); }
        .session-item.active { background: var(--primary); color: white; }
        .session-item .agent { font-weight: 500; }
        .session-item .meta { font-size: 0.75rem; color: var(--text-dim); margin-top: 0.25rem; }
        .session-item.active .meta { color: rgba(255,255,255,0.7); }
        
        /* Utilities */
        .hidden { display: none !important; }
        .text-dim { color: var(--text-dim); }
        .text-success { color: var(--success); }
        .text-error { color: var(--error); }
        .mt-1 { margin-top: 0.5rem; }
        .mt-2 { margin-top: 1rem; }
        .mb-2 { margin-bottom: 1rem; }
        .flex { display: flex; }
        .gap-2 { gap: 0.75rem; }
        .items-center { align-items: center; }
        .justify-between { justify-content: space-between; }
        
        /* Keyboard shortcuts hint */
        .shortcuts { position: fixed; bottom: 1rem; right: 1rem; background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 0.5rem 1rem; font-size: 0.75rem; color: var(--text-dim); }
        .shortcuts kbd { background: var(--bg); padding: 0.125rem 0.375rem; border-radius: 4px; margin: 0 0.125rem; }
    </style>
</head>
<body>
    <div class="app">
        <aside class="sidebar">
            <div class="logo">🦀 Krabbykrus</div>
            <ul class="nav">
                <li class="nav-item active" data-page="dashboard">
                    <span class="icon">📊</span> Dashboard
                    <span class="status-dot online" id="status-dot"></span>
                </li>
                <li class="nav-item" data-page="credentials">
                    <span class="icon">🔐</span> Credentials
                </li>
                <li class="nav-item" data-page="agents">
                    <span class="icon">🤖</span> Agents
                </li>
                <li class="nav-item" data-page="sessions">
                    <span class="icon">💬</span> Sessions
                </li>
                <li class="nav-item" data-page="models">
                    <span class="icon">🧠</span> Models
                </li>
                <li class="nav-item" data-page="settings">
                    <span class="icon">⚙️</span> Settings
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
                    <div class="card"><h3>Gateway</h3><div class="value" id="stat-gateway">●</div><div class="sub" id="stat-version">-</div></div>
                    <div class="card"><h3>Agents</h3><div class="value"><span id="stat-agents">0</span></div><div class="sub" id="stat-pending"></div></div>
                    <div class="card"><h3>Sessions</h3><div class="value" id="stat-sessions">0</div><div class="sub">active</div></div>
                    <div class="card"><h3>Vault</h3><div class="value" id="stat-vault">-</div><div class="sub" id="stat-vault-info"></div></div>
                </div>
                <div class="card">
                    <div class="card-header"><h3>Configured Agents</h3><button class="btn btn-primary btn-sm" onclick="reloadAgents()">↻ Reload</button></div>
                    <table><thead><tr><th>Agent ID</th><th>Model</th><th>Sessions</th><th>Status</th></tr></thead><tbody id="agents-table"></tbody></table>
                </div>
            </div>
            
            <!-- Credentials Page -->
            <div id="page-credentials" class="content page hidden">
                <div class="page-header">
                    <h1>Credential Vault</h1>
                    <button class="btn btn-primary" onclick="showAddEndpoint()" id="btn-add-endpoint">+ Add Endpoint</button>
                </div>
                <div id="vault-init-section" class="card mb-2 hidden">
                    <h3 class="mb-2">Initialize Vault</h3>
                    <p class="text-dim mb-2">Set up the credential vault to securely store API keys and secrets.</p>
                    <div class="form-group"><label>Password (min 8 characters)</label><input type="password" id="init-password" placeholder="Enter password"></div>
                    <div class="form-group"><label>Confirm Password</label><input type="password" id="init-password-confirm" placeholder="Confirm password"></div>
                    <button class="btn btn-primary" onclick="initializeVault()">Initialize Vault</button>
                </div>
                <div id="vault-unlock-section" class="card mb-2 hidden">
                    <h3 class="mb-2">🔒 Vault Locked</h3>
                    <p class="text-dim mb-2">Enter your password to unlock the vault.</p>
                    <div class="flex gap-2"><input type="password" id="unlock-password" placeholder="Password" style="flex:1" onkeypress="if(event.key==='Enter')unlockVault()"><button class="btn btn-primary" onclick="unlockVault()">Unlock</button></div>
                </div>
                <div id="vault-content" class="hidden">
                    <div class="card mb-2">
                        <div class="flex items-center gap-2">
                            <span class="badge badge-success">🔓 Unlocked</span>
                            <button class="btn btn-secondary btn-sm" onclick="lockVault()">Lock Vault</button>
                        </div>
                    </div>
                    <div class="card">
                        <h3 class="mb-2">Service Endpoints</h3>
                        <table><thead><tr><th>Name</th><th>Type</th><th>URL</th><th>Status</th><th>Actions</th></tr></thead><tbody id="endpoints-table"></tbody></table>
                    </div>
                </div>
            </div>
            
            <!-- Agents Page -->
            <div id="page-agents" class="content page hidden">
                <div class="page-header">
                    <h1>Agent Configuration</h1>
                    <div style="display:flex;gap:0.5rem">
                        <button class="btn btn-primary" onclick="showCreateAgent()">+ Create Agent</button>
                        <button class="btn btn-secondary" onclick="refreshAgents()">↻ Refresh</button>
                    </div>
                </div>
                <div class="split split-40-60">
                    <div class="card">
                        <h3 class="mb-2">Agents</h3>
                        <div id="agent-list" class="session-list"></div>
                    </div>
                    <div class="card">
                        <h3 class="mb-2">Details</h3>
                        <div id="agent-details">
                            <p class="text-dim">Select an agent to view details</p>
                        </div>
                    </div>
                </div>
            </div>

            <!-- Agent Create/Edit Modal -->
            <div id="modal-agent" class="modal hidden">
                <div class="modal-content" style="max-width:560px">
                    <h2 id="agent-modal-title">Create Agent</h2>
                    <div class="form-group">
                        <label>Agent ID</label>
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
                        <select id="agent-parent">
                            <option value="">None (top-level agent)</option>
                        </select>
                        <div class="hint">Set a parent to create a subagent</div>
                    </div>
                    <div class="form-group">
                        <label>Workspace</label>
                        <input type="text" id="agent-workspace" placeholder="uses default if empty">
                    </div>
                    <div class="form-group">
                        <label>Max Tool Calls</label>
                        <input type="number" id="agent-max-tools" value="10" min="1" max="100">
                    </div>
                    <div class="form-group">
                        <label>System Prompt</label>
                        <textarea id="agent-system-prompt" rows="3" placeholder="Optional system prompt override" style="width:100%;background:var(--surface-2);color:var(--text);border:1px solid var(--border);border-radius:6px;padding:0.5rem;resize:vertical"></textarea>
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
            
            <!-- Sessions Page -->
            <div id="page-sessions" class="content page hidden">
                <div class="page-header">
                    <h1>Active Sessions</h1>
                    <select id="chat-agent" class="btn btn-secondary"><option value="">New Chat...</option></select>
                </div>
                <div class="split split-35-65">
                    <div class="card">
                        <h3 class="mb-2">Sessions</h3>
                        <div id="session-list" class="session-list"></div>
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
                <div class="grid">
                    <div class="card" id="provider-anthropic">
                        <div class="card-header"><h3>Anthropic</h3><span class="badge badge-error" id="anthropic-status">Not Configured</span></div>
                        <p class="text-dim mb-2">Claude models (Opus 4, Sonnet 4, Haiku 3.5)</p>
                        <div class="form-group">
                            <label>Authentication Type</label>
                            <select id="anthropic-auth-type" onchange="toggleAnthropicAuthFields()">
                                <option value="session_key">Session Key (Claude Code)</option>
                                <option value="api_key">API Key</option>
                            </select>
                            <p class="hint" id="anthropic-auth-hint">Uses Claude Code credentials (~/.claude/.credentials.json)</p>
                        </div>
                        <div class="form-group" id="anthropic-key-group" style="display: none;">
                            <label>API Key</label>
                            <input type="password" id="anthropic-key" placeholder="sk-ant-...">
                        </div>
                        <div id="anthropic-session-status" class="form-group">
                            <p id="anthropic-session-info" class="text-dim">Checking Claude Code credentials...</p>
                        </div>
                        <button class="btn btn-primary btn-sm" onclick="saveProvider('anthropic')">Save</button>
                    </div>
                    <div class="card" id="provider-openai">
                        <div class="card-header"><h3>OpenAI</h3><span class="badge badge-error" id="openai-status">Not Configured</span></div>
                        <p class="text-dim mb-2">GPT-4, GPT-3.5 models</p>
                        <div class="form-group"><label>API Key</label><input type="password" id="openai-key" placeholder="sk-..."></div>
                        <button class="btn btn-primary btn-sm" onclick="saveProvider('openai')">Save</button>
                    </div>
                    <div class="card" id="provider-ollama">
                        <div class="card-header"><h3>Ollama</h3><span class="badge badge-info" id="ollama-status">Local</span></div>
                        <p class="text-dim mb-2">Local models (Llama, Mistral, etc.)</p>
                        <div class="form-group"><label>Base URL</label><input type="text" id="ollama-url" value="http://localhost:11434"></div>
                        <button class="btn btn-primary btn-sm" onclick="saveProvider('ollama')">Save</button>
                    </div>
                    <div class="card" id="provider-bedrock">
                        <div class="card-header"><h3>AWS Bedrock</h3><span class="badge badge-error" id="bedrock-status">Not Configured</span></div>
                        <p class="text-dim mb-2">AWS-hosted models</p>
                        <div class="form-group"><label>Region</label><input type="text" id="bedrock-region" placeholder="us-west-2"></div>
                        <button class="btn btn-primary btn-sm" onclick="saveProvider('bedrock')">Save</button>
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
                            <span>Port</span>
                            <span>18080</span>
                        </div>
                        <div class="flex gap-2 mt-2">
                            <button class="btn btn-secondary btn-sm" onclick="restartGateway()">Restart</button>
                            <button class="btn btn-danger btn-sm" onclick="stopGateway()">Stop</button>
                        </div>
                    </div>
                    <div class="card">
                        <h3 class="mb-2">Paths</h3>
                        <div class="form-group">
                            <label>Config File</label>
                            <input type="text" id="config-path" readonly value="~/.config/krabbykrus/krabbykrus.toml">
                        </div>
                        <div class="form-group">
                            <label>Vault Path</label>
                            <input type="text" id="vault-path" readonly value="~/.config/krabbykrus/vault.db">
                        </div>
                    </div>
                </div>
                <div class="card mt-2">
                    <h3 class="mb-2">About</h3>
                    <p>Krabbykrus - A Rust-native AI agent framework</p>
                    <p class="text-dim mt-1">https://github.com/openclaw/krabbykrus</p>
                </div>
            </div>
        </main>
    </div>
    
    <!-- Add Endpoint Modal -->
    <div id="modal-endpoint" class="modal-overlay hidden">
        <div class="modal">
            <h2>Add Service Endpoint</h2>
            <div class="form-group">
                <label>Endpoint Name</label>
                <input type="text" id="endpoint-name" placeholder="e.g., My Home Assistant">
                <div class="hint">A friendly name to identify this endpoint</div>
            </div>
            <div class="form-group">
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
                <button class="btn btn-primary" onclick="saveEndpoint()">Save</button>
            </div>
        </div>
    </div>
    
    <div class="shortcuts">Press <kbd>1</kbd>-<kbd>6</kbd> for quick nav</div>
    
    <script>
        // State
        let currentPage = 'dashboard';
        let sessionKey = 'web-' + Date.now();
        let selectedSession = null;
        let selectedAgent = null;
        let agents = [];
        let sessions = [];
        
        // Navigation
        document.querySelectorAll('.nav-item').forEach(item => {
            item.addEventListener('click', () => showPage(item.dataset.page));
        });
        
        // Keyboard shortcuts
        document.addEventListener('keydown', e => {
            if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
            const pages = ['dashboard', 'credentials', 'agents', 'sessions', 'models', 'settings'];
            if (e.key >= '1' && e.key <= '6') {
                showPage(pages[parseInt(e.key) - 1]);
            }
        });
        
        function showPage(page) {
            document.querySelectorAll('.nav-item').forEach(i => i.classList.remove('active'));
            document.querySelector(`[data-page="${page}"]`)?.classList.add('active');
            document.querySelectorAll('.page').forEach(p => p.classList.add('hidden'));
            document.getElementById(`page-${page}`)?.classList.remove('hidden');
            currentPage = page;
            
            // Load page-specific data
            if (page === 'credentials') loadCredentials();
            if (page === 'agents') loadAgentsPage();
            if (page === 'sessions') loadSessionsPage();
            if (page === 'models') loadModels();
        }
        
        // API helpers
        async function api(url, method = 'GET', data = null) {
            const opts = { method, headers: { 'Content-Type': 'application/json' } };
            if (data) opts.body = JSON.stringify(data);
            const res = await fetch(url, opts);
            const json = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(json.error || `HTTP ${res.status}`);
            return json;
        }
        
        function showError(msg) { alert('Error: ' + msg); }
        function showSuccess(msg) { alert(msg); }
        
        // Dashboard
        async function loadDashboard() {
            try {
                const health = await api('/health');
                document.getElementById('stat-version').textContent = health.version || '-';
                document.getElementById('stat-gateway').textContent = '●';
                document.getElementById('stat-gateway').style.color = 'var(--success)';
                document.getElementById('stat-agents').textContent = health.agents?.length || 0;
                document.getElementById('stat-sessions').textContent = health.active_sessions || 0;
                document.getElementById('status-dot').className = 'status-dot online';
                document.getElementById('gateway-status').textContent = 'Online';
                document.getElementById('gateway-status').className = 'badge badge-success';
                document.getElementById('version-info').textContent = 'v' + (health.version || '0.1.0');
                
                agents = health.agents || [];
                renderAgentsTable();
                
                const pending = await api('/api/gateway/pending').catch(() => ({ count: 0 }));
                if (pending.count > 0) {
                    document.getElementById('stat-pending').textContent = `+${pending.count} pending`;
                }
                
                const vault = await api('/api/credentials/status').catch(() => ({}));
                document.getElementById('stat-vault').textContent = !vault.initialized ? 'Init' : (vault.locked ? '🔒' : '🔓');
                document.getElementById('stat-vault-info').textContent = vault.initialized ? `${vault.endpoint_count || 0} endpoints` : 'Not initialized';
            } catch (e) {
                document.getElementById('stat-gateway').textContent = '○';
                document.getElementById('stat-gateway').style.color = 'var(--error)';
                document.getElementById('status-dot').className = 'status-dot offline';
                document.getElementById('gateway-status').textContent = 'Offline';
                document.getElementById('gateway-status').className = 'badge badge-error';
            }
        }
        
        function renderAgentsTable() {
            const tbody = document.getElementById('agents-table');
            tbody.innerHTML = agents.length === 0 
                ? '<tr><td colspan="4" class="text-dim">No agents configured</td></tr>'
                : agents.map(a => `<tr>
                    <td>${a.id || a}</td>
                    <td class="text-dim">${a.model || '-'}</td>
                    <td>${a.session_count || 0}</td>
                    <td><span class="badge badge-success">Active</span></td>
                </tr>`).join('');
        }
        
        async function reloadAgents() {
            try {
                const res = await api('/api/gateway/reload', 'POST');
                alert(`Reloaded: ${res.agents_created || 0} created, ${res.agents_pending || 0} pending`);
                loadDashboard();
            } catch (e) { showError(e.message); }
        }
        
        // Credentials
        async function loadCredentials() {
            try {
                const status = await api('/api/credentials/status');
                document.getElementById('vault-init-section').classList.add('hidden');
                document.getElementById('vault-unlock-section').classList.add('hidden');
                document.getElementById('vault-content').classList.add('hidden');
                document.getElementById('btn-add-endpoint').classList.add('hidden');
                
                if (!status.enabled) {
                    document.getElementById('vault-init-section').classList.remove('hidden');
                    document.getElementById('vault-init-section').innerHTML = '<p class="text-dim">Credential management is disabled.</p>';
                    return;
                }
                if (!status.initialized) { document.getElementById('vault-init-section').classList.remove('hidden'); return; }
                if (status.locked) { document.getElementById('vault-unlock-section').classList.remove('hidden'); return; }
                
                document.getElementById('vault-content').classList.remove('hidden');
                document.getElementById('btn-add-endpoint').classList.remove('hidden');
                
                const endpoints = await api('/api/credentials/endpoints');
                const tbody = document.getElementById('endpoints-table');
                tbody.innerHTML = endpoints.length === 0 
                    ? '<tr><td colspan="5" class="text-dim">No endpoints. Click "Add Endpoint" to get started.</td></tr>'
                    : endpoints.map(ep => `<tr>
                        <td>${ep.name}</td>
                        <td><span class="badge badge-info">${ep.endpoint_type}</span></td>
                        <td class="text-dim">${ep.base_url}</td>
                        <td><span class="badge badge-success">✓</span></td>
                        <td><button class="btn btn-danger btn-sm" onclick="deleteEndpoint('${ep.id}')">Delete</button></td>
                    </tr>`).join('');
            } catch (e) { showError(e.message); }
        }
        
        async function initializeVault() {
            const pw = document.getElementById('init-password').value;
            const confirm = document.getElementById('init-password-confirm').value;
            if (pw.length < 8) { showError('Password must be at least 8 characters'); return; }
            if (pw !== confirm) { showError('Passwords do not match'); return; }
            try { await api('/api/credentials/init', 'POST', { method: 'password', password: pw }); loadCredentials(); }
            catch (e) { showError(e.message); }
        }
        
        async function unlockVault() {
            try { await api('/api/credentials/unlock', 'POST', { password: document.getElementById('unlock-password').value }); loadCredentials(); }
            catch (e) { showError(e.message); }
        }
        
        async function lockVault() {
            try { await api('/api/credentials/lock', 'POST'); loadCredentials(); }
            catch (e) { showError(e.message); }
        }
        
        function showAddEndpoint() {
            document.getElementById('endpoint-name').value = '';
            document.getElementById('endpoint-type').value = 'home_assistant';
            updateEndpointForm();
            document.getElementById('modal-endpoint').classList.remove('hidden');
        }
        
        function closeModal(name) { document.getElementById(`modal-${name}`).classList.add('hidden'); }
        
        function updateEndpointForm() {
            const type = document.getElementById('endpoint-type').value;
            const container = document.getElementById('endpoint-form-dynamic');
            
            const configs = {
                home_assistant: [
                    { id: 'url', label: 'Home Assistant URL', placeholder: 'http://homeassistant.local:8123' },
                    { id: 'token', label: 'Long-Lived Access Token', type: 'password', placeholder: 'eyJ0eXAi...' }
                ],
                generic_rest: [
                    { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com' },
                    { id: 'token', label: 'Bearer Token', type: 'password', placeholder: 'Your token' }
                ],
                generic_oauth2: [
                    { id: 'url', label: 'API Base URL', placeholder: 'https://api.example.com' },
                    { id: 'auth_url', label: 'Authorization URL', placeholder: 'https://auth.example.com/authorize' },
                    { id: 'token_url', label: 'Token URL', placeholder: 'https://auth.example.com/token' },
                    { id: 'client_id', label: 'Client ID' },
                    { id: 'client_secret', label: 'Client Secret', type: 'password' },
                    { id: 'scopes', label: 'Scopes', placeholder: 'read write offline_access' },
                    { id: 'redirect_uri', label: 'Redirect URI', placeholder: 'http://localhost:18080/oauth/callback' }
                ],
                api_key_service: [
                    { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com' },
                    { id: 'api_key', label: 'API Key', type: 'password' },
                    { id: 'header_name', label: 'Header Name', placeholder: 'X-API-Key' }
                ],
                basic_auth_service: [
                    { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com' },
                    { id: 'username', label: 'Username' },
                    { id: 'password', label: 'Password', type: 'password' }
                ],
                bearer_token: [
                    { id: 'url', label: 'Base URL', placeholder: 'https://api.example.com' },
                    { id: 'token', label: 'Token', type: 'password' }
                ]
            };
            
            container.innerHTML = (configs[type] || []).map(f => `
                <div class="form-group">
                    <label>${f.label}</label>
                    <input type="${f.type || 'text'}" id="field-${f.id}" placeholder="${f.placeholder || ''}">
                </div>
            `).join('');
        }
        
        async function saveEndpoint() {
            const name = document.getElementById('endpoint-name').value;
            const type = document.getElementById('endpoint-type').value;
            if (!name) { showError('Name is required'); return; }
            
            const url = document.getElementById('field-url')?.value;
            if (!url) { showError('URL is required'); return; }
            
            try {
                await api('/api/credentials/endpoints', 'POST', { name, endpoint_type: type, base_url: url });
                closeModal('endpoint');
                loadCredentials();
            } catch (e) { showError(e.message); }
        }
        
        async function deleteEndpoint(id) {
            if (!confirm('Delete this endpoint?')) return;
            try { await api(`/api/credentials/endpoints/${id}`, 'DELETE'); loadCredentials(); }
            catch (e) { showError(e.message); }
        }
        
        // Agents Page
        let editingAgentId = null;

        async function loadAgentsPage() {
            await loadDashboard();
            renderAgentList();
        }

        function renderAgentList() {
            const container = document.getElementById('agent-list');
            // Sort: top-level agents first, then subagents grouped under parents
            const topLevel = agents.filter(a => !a.parent_id);
            const subOf = pid => agents.filter(a => a.parent_id === pid);

            let html = '';
            if (agents.length === 0) {
                html = '<p class="text-dim" style="padding:1rem">No agents configured. Click "Create Agent" to get started.</p>';
            } else {
                const renderItem = (a, i, indent) => {
                    const id = a.id || a;
                    const realIdx = agents.indexOf(a);
                    return `<div class="session-item ${realIdx === selectedAgent ? 'active' : ''}" onclick="selectAgent(${realIdx})" style="padding-left:${indent?'2rem':'1rem'}">
                        <div class="agent">${indent ? '└ ' : ''}${id}</div>
                        <div class="meta">${a.model || 'default'} ${a.parent_id ? '(subagent)' : ''}</div>
                    </div>`;
                };
                topLevel.forEach((a, i) => {
                    html += renderItem(a, i, false);
                    subOf(a.id || a).forEach(sub => { html += renderItem(sub, 0, true); });
                });
                // Show orphan subagents (parent not found)
                agents.filter(a => a.parent_id && !topLevel.some(t => (t.id||t) === a.parent_id)).forEach(a => {
                    html += renderItem(a, 0, true);
                });
            }
            container.innerHTML = html;
        }

        function selectAgent(idx) {
            selectedAgent = idx;
            renderAgentList();
            const agent = agents[idx];
            if (!agent) { document.getElementById('agent-details').innerHTML = '<p class="text-dim">Select an agent</p>'; return; }
            const id = agent.id || agent;
            const subs = agents.filter(a => a.parent_id === id).map(a => a.id || a);
            const statusBadge = agent.status === 'pending'
                ? `<span class="badge badge-warning">Pending</span><div class="hint">${agent.reason || ''}</div>`
                : (agent.enabled === false ? '<span class="badge badge-error">Disabled</span>' : '<span class="badge badge-success">Active</span>');

            document.getElementById('agent-details').innerHTML = `
                <div class="form-group"><label>ID</label><input type="text" readonly value="${id}"></div>
                <div class="form-group"><label>Model</label><input type="text" readonly value="${agent.model || 'default'}"></div>
                ${agent.parent_id ? `<div class="form-group"><label>Parent</label><input type="text" readonly value="${agent.parent_id}"><div class="hint">This is a subagent</div></div>` : ''}
                ${subs.length > 0 ? `<div class="form-group"><label>Subagents</label><div>${subs.map(s=>'<span class="badge badge-info" style="margin-right:4px">'+s+'</span>').join('')}</div></div>` : ''}
                ${agent.workspace ? `<div class="form-group"><label>Workspace</label><input type="text" readonly value="${agent.workspace}"></div>` : ''}
                <div class="form-group"><label>Max Tool Calls</label><input type="text" readonly value="${agent.max_tool_calls || 10}"></div>
                <div class="form-group"><label>Status</label>${statusBadge}</div>
                ${agent.system_prompt ? `<div class="form-group"><label>System Prompt</label><div style="background:var(--surface-2);padding:0.5rem;border-radius:6px;font-size:0.85rem;max-height:100px;overflow-y:auto">${escapeHtml(agent.system_prompt)}</div></div>` : ''}
                <div style="display:flex;gap:0.5rem;margin-top:1rem">
                    <button class="btn btn-primary btn-sm" onclick="showEditAgent('${id}')">Edit</button>
                    <button class="btn btn-secondary btn-sm" onclick="showCreateSubagent('${id}')">+ Subagent</button>
                    <button class="btn btn-danger btn-sm" onclick="deleteAgent('${id}')">Delete</button>
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
            document.getElementById('agent-max-tools').value = '10';
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
            const agent = agents.find(a => (a.id || a) === id);
            if (!agent) return;
            editingAgentId = id;
            document.getElementById('agent-modal-title').textContent = 'Edit Agent: ' + id;
            document.getElementById('agent-save-btn').textContent = 'Save';
            document.getElementById('agent-id').value = id;
            document.getElementById('agent-id').readOnly = true;
            document.getElementById('agent-model').value = agent.model || '';
            document.getElementById('agent-workspace').value = agent.workspace || '';
            document.getElementById('agent-max-tools').value = agent.max_tool_calls || 10;
            document.getElementById('agent-system-prompt').value = agent.system_prompt || '';
            populateParentSelect(agent.parent_id || '');
            // Show subagents
            const subs = agents.filter(a => a.parent_id === id);
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
                agents.filter(a => (a.id || a) !== editingAgentId)
                    .map(a => `<option value="${a.id || a}" ${(a.id || a) === selected ? 'selected' : ''}>${a.id || a}</option>`)
                    .join('');
        }

        async function saveAgent() {
            const id = document.getElementById('agent-id').value.trim();
            if (!id) { showError('Agent ID is required'); return; }
            if (/[\s\/]/.test(id)) { showError('Agent ID cannot contain spaces or slashes'); return; }

            const data = {
                id,
                model: document.getElementById('agent-model').value.trim() || null,
                parent_id: document.getElementById('agent-parent').value || null,
                workspace: document.getElementById('agent-workspace').value.trim() || null,
                max_tool_calls: parseInt(document.getElementById('agent-max-tools').value) || 10,
                system_prompt: document.getElementById('agent-system-prompt').value.trim() || null,
            };

            try {
                if (editingAgentId) {
                    await api(`/api/agents/${editingAgentId}`, 'PUT', data);
                } else {
                    await api('/api/agents', 'POST', data);
                }
                closeModal('agent');
                await loadAgentsPage();
                const msg = editingAgentId ? 'Agent updated' : 'Agent created';
                showSuccess(msg + '. Reload gateway to apply changes.');
            } catch (e) { showError(e.message); }
        }

        async function deleteAgent(id) {
            if (!confirm(`Delete agent "${id}"? This will also remove it from running agents.`)) return;
            try {
                await api(`/api/agents/${id}`, 'DELETE');
                await loadAgentsPage();
            } catch (e) { showError(e.message); }
        }

        function refreshAgents() { loadAgentsPage(); }
        
        // Sessions Page
        async function loadSessionsPage() {
            try {
                const data = await api('/api/sessions');
                sessions = data.sessions || [];
                renderSessionList();
                
                // Populate agent dropdown
                const select = document.getElementById('chat-agent');
                select.innerHTML = '<option value="">New Chat...</option>' + 
                    agents.map(a => `<option value="${a.id || a}">${a.id || a}</option>`).join('');
            } catch (e) {
                sessions = [];
                renderSessionList();
            }
        }
        
        function renderSessionList() {
            const container = document.getElementById('session-list');
            container.innerHTML = sessions.length === 0
                ? '<p class="text-dim" style="padding:1rem">No active sessions</p>'
                : sessions.map((s, i) => `
                    <div class="session-item ${s.key === selectedSession ? 'active' : ''}" onclick="selectSession('${s.key}')">
                        <div class="agent">${s.agent_id}</div>
                        <div class="meta">${s.channel || 'web'} • ${s.message_count || 0} msgs</div>
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
            messages.innerHTML = '<div class="message assistant"><p>Loading...</p></div>';
            try {
                const data = await api(`/api/sessions/${key}/history`);
                messages.innerHTML = (data.messages || []).map(m => `
                    <div class="message ${m.role}">${formatMessage(m.content)}</div>
                `).join('') || '<div class="message assistant"><p>No messages yet.</p></div>';
                messages.scrollTop = messages.scrollHeight;
            } catch (e) {
                messages.innerHTML = '<div class="message assistant"><p>Failed to load history.</p></div>';
            }
        }
        
        async function sendMessage() {
            const input = document.getElementById('chat-input');
            const agentSelect = document.getElementById('chat-agent');
            const message = input.value.trim();
            if (!message) return;
            
            const agent = agentSelect.value || agents[0]?.id || 'default';
            
            const messages = document.getElementById('chat-messages');
            messages.innerHTML += `<div class="message user"><p>${escapeHtml(message)}</p></div>`;
            input.value = '';
            messages.scrollTop = messages.scrollHeight;
            
            try {
                const res = await api(`/api/agents/${agent}/message`, 'POST', { session_key: sessionKey, message });
                messages.innerHTML += `<div class="message assistant">${formatMessage(res.content || res.message || JSON.stringify(res))}</div>`;
            } catch (e) {
                messages.innerHTML += `<div class="message assistant"><p class="text-error">Error: ${e.message}</p></div>`;
            }
            messages.scrollTop = messages.scrollHeight;
        }
        
        function escapeHtml(t) { return t.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;'); }
        function formatMessage(t) { return '<p>' + escapeHtml(t).replace(/```([\s\S]*?)```/g,'</p><pre>$1</pre><p>').replace(/`([^`]+)`/g,'<code>$1</code>').replace(/\n/g,'<br>') + '</p>'; }
        
        // Models
        async function loadModels() {
            // Check Anthropic auth status
            checkAnthropicAuth();
        }
        
        function toggleAnthropicAuthFields() {
            const authType = document.getElementById('anthropic-auth-type').value;
            const keyGroup = document.getElementById('anthropic-key-group');
            const sessionStatus = document.getElementById('anthropic-session-status');
            const hint = document.getElementById('anthropic-auth-hint');
            
            if (authType === 'api_key') {
                keyGroup.style.display = 'block';
                sessionStatus.style.display = 'none';
                hint.textContent = 'Get your API key from console.anthropic.com';
            } else {
                keyGroup.style.display = 'none';
                sessionStatus.style.display = 'block';
                hint.textContent = 'Uses Claude Code credentials (~/.claude/.credentials.json)';
                checkAnthropicAuth();
            }
        }
        
        async function checkAnthropicAuth() {
            const infoEl = document.getElementById('anthropic-session-info');
            const statusEl = document.getElementById('anthropic-status');
            
            try {
                const res = await api('/api/providers/anthropic/status');
                if (res.configured) {
                    statusEl.className = 'badge badge-success';
                    statusEl.textContent = 'Configured';
                    if (res.auth_type === 'session_key') {
                        infoEl.innerHTML = '<span style="color: var(--success)">✓ Claude Code credentials detected</span>' +
                            (res.expires_at ? `<br><small>Token expires: ${new Date(res.expires_at).toLocaleString()}</small>` : '');
                    } else {
                        infoEl.innerHTML = '<span style="color: var(--success)">✓ API key configured</span>';
                    }
                } else {
                    statusEl.className = 'badge badge-error';
                    statusEl.textContent = 'Not Configured';
                    infoEl.innerHTML = '<span style="color: var(--text-dim)">Run <code>claude</code> CLI to authenticate, or switch to API Key</span>';
                }
            } catch (e) {
                infoEl.innerHTML = '<span style="color: var(--error)">Error checking status</span>';
            }
        }
        
        async function saveProvider(provider) {
            if (provider === 'anthropic') {
                const authType = document.getElementById('anthropic-auth-type').value;
                if (authType === 'session_key') {
                    // Just verify credentials exist
                    await checkAnthropicAuth();
                    return;
                }
            }
            
            const key = document.getElementById(`${provider}-key`)?.value || document.getElementById(`${provider}-url`)?.value || document.getElementById(`${provider}-region`)?.value;
            if (!key) { showError('Please enter a value'); return; }
            
            try {
                await api(`/api/providers/${provider}/configure`, 'POST', { key });
                showSuccess(`Provider ${provider} saved. Restart gateway to apply.`);
            } catch (e) {
                showError(e.message);
            }
        }
        
        // Settings
        async function restartGateway() {
            if (!confirm('Restart the gateway?')) return;
            try {
                await api('/api/gateway/restart', 'POST');
                alert('Gateway restarting...');
            } catch (e) { showError(e.message); }
        }
        
        async function stopGateway() {
            if (!confirm('Stop the gateway?')) return;
            try {
                await api('/api/gateway/stop', 'POST');
                alert('Gateway stopped');
                loadDashboard();
            } catch (e) { showError(e.message); }
        }
        
        // Init
        loadDashboard();
        updateEndpointForm();
        setInterval(loadDashboard, 15000);
    </script>
</body>
</html>"##
}
