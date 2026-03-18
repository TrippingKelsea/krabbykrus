//! Embedded browser bootstrap UI for RockBot.
//!
//! The public HTTPS listener intentionally exposes only a minimal shell:
//! static assets, health, trust bootstrap, and optional enrollment.

/// Return the browser bootstrap shell.
pub fn get_dashboard_html() -> &'static str {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>RockBot</title>
  <link rel="stylesheet" href="/static/app.css">
</head>
<body>
  <main class="shell">
    <section class="hero">
      <p class="eyebrow">RockBot Web Bootstrap</p>
      <h1>Import your client identity, then connect over the authenticated control plane.</h1>
      <p class="lede">
        The public HTTPS listener only serves this bootstrap shell, static assets,
        trust material, and optionally enrollment. Stateful application traffic
        belongs on the authenticated WebSocket control plane.
      </p>
    </section>

    <section class="panel">
      <div class="panel-header">
        <h2>Gateway</h2>
        <span id="health-pill" class="pill pill-idle">Checking</span>
      </div>
      <dl class="facts">
        <div><dt>Health</dt><dd id="health-text">Loading...</dd></div>
        <div><dt>CA Bundle</dt><dd><a href="/api/cert/ca" target="_blank" rel="noreferrer">Download public CA</a></dd></div>
      </dl>
    </section>

    <section class="panel">
      <div class="panel-header">
        <h2>Browser Identity</h2>
        <span id="identity-pill" class="pill pill-idle">No key imported</span>
      </div>
      <p class="help">
        Import a PEM client certificate and PEM private key. RockBot stores them in
        IndexedDB so you do not need to re-import them every time you open the app.
      </p>

      <div id="dropzone" class="dropzone" tabindex="0">
        <strong>Drop PEM files here</strong>
        <span>or choose them manually below</span>
      </div>

      <div class="form-grid">
        <label>
          <span>Client Certificate (.crt/.pem)</span>
          <input id="cert-file" type="file" accept=".crt,.pem">
        </label>
        <label>
          <span>Private Key (.key/.pem)</span>
          <input id="key-file" type="file" accept=".key,.pem">
        </label>
      </div>

      <div class="actions">
        <button id="save-btn" class="btn btn-primary" type="button">Save Identity</button>
        <button id="clear-btn" class="btn btn-secondary" type="button">Forget Identity</button>
      </div>

      <pre id="identity-summary" class="summary">No client identity stored.</pre>
    </section>
  </main>

  <script src="/static/app.js" defer></script>
</body>
</html>
"#
}

/// Return a static asset by request path.
pub fn get_static_asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path {
        "/static/app.css" => Some(("text/css; charset=utf-8", APP_CSS)),
        "/static/app.js" => Some(("application/javascript; charset=utf-8", APP_JS)),
        _ => None,
    }
}

const APP_CSS: &str = r#"
:root {
  --bg: #0b1220;
  --panel: rgba(17, 24, 39, 0.92);
  --panel-strong: rgba(15, 23, 42, 0.98);
  --text: #e5edf9;
  --dim: #94a3b8;
  --line: rgba(148, 163, 184, 0.18);
  --accent: #2dd4bf;
  --accent-strong: #0f766e;
  --warn: #f59e0b;
  --danger: #ef4444;
  --ok: #22c55e;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  min-height: 100vh;
  font-family: "IBM Plex Sans", "Segoe UI", sans-serif;
  color: var(--text);
  background:
    radial-gradient(circle at top, rgba(45, 212, 191, 0.18), transparent 32rem),
    linear-gradient(180deg, #08111f 0%, #020617 100%);
}
a { color: var(--accent); }
.shell {
  width: min(920px, calc(100vw - 2rem));
  margin: 0 auto;
  padding: 3rem 0 4rem;
}
.hero {
  margin-bottom: 2rem;
}
.eyebrow {
  margin: 0 0 0.75rem;
  font-size: 0.8rem;
  text-transform: uppercase;
  letter-spacing: 0.14em;
  color: var(--accent);
}
h1 {
  margin: 0;
  font-size: clamp(2rem, 4vw, 3.2rem);
  line-height: 1.04;
  max-width: 14ch;
}
.lede {
  margin: 1rem 0 0;
  max-width: 62ch;
  color: var(--dim);
  line-height: 1.6;
}
.panel {
  margin-top: 1.25rem;
  padding: 1.25rem;
  border: 1px solid var(--line);
  border-radius: 18px;
  background: var(--panel);
  backdrop-filter: blur(18px);
}
.panel-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1rem;
}
.panel-header h2 {
  margin: 0;
  font-size: 1.05rem;
}
.pill {
  display: inline-flex;
  align-items: center;
  border-radius: 999px;
  padding: 0.28rem 0.7rem;
  font-size: 0.78rem;
  font-weight: 600;
}
.pill-idle { background: rgba(148, 163, 184, 0.15); color: var(--dim); }
.pill-ok { background: rgba(34, 197, 94, 0.16); color: #86efac; }
.pill-warn { background: rgba(245, 158, 11, 0.16); color: #fcd34d; }
.facts {
  margin: 1rem 0 0;
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 0.9rem;
}
.facts div {
  padding: 0.9rem 1rem;
  border-radius: 14px;
  background: var(--panel-strong);
}
.facts dt {
  margin-bottom: 0.35rem;
  color: var(--dim);
  font-size: 0.8rem;
  text-transform: uppercase;
  letter-spacing: 0.08em;
}
.facts dd {
  margin: 0;
}
.help {
  margin: 1rem 0 0;
  color: var(--dim);
  line-height: 1.6;
}
.dropzone {
  margin-top: 1rem;
  border: 1px dashed rgba(45, 212, 191, 0.5);
  border-radius: 16px;
  padding: 1.5rem;
  background: rgba(13, 20, 33, 0.72);
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  align-items: center;
  justify-content: center;
  text-align: center;
  color: var(--dim);
}
.dropzone.active {
  border-color: var(--accent);
  background: rgba(15, 118, 110, 0.18);
  color: var(--text);
}
.form-grid {
  margin-top: 1rem;
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: 0.9rem;
}
label span {
  display: block;
  margin-bottom: 0.45rem;
  color: var(--dim);
  font-size: 0.85rem;
}
input[type=file] {
  width: 100%;
  padding: 0.8rem;
  border-radius: 12px;
  border: 1px solid var(--line);
  background: var(--panel-strong);
  color: var(--text);
}
.actions {
  margin-top: 1rem;
  display: flex;
  gap: 0.75rem;
  flex-wrap: wrap;
}
.btn {
  border: none;
  border-radius: 999px;
  padding: 0.8rem 1.1rem;
  font: inherit;
  font-weight: 600;
  cursor: pointer;
}
.btn-primary {
  background: linear-gradient(135deg, var(--accent) 0%, #14b8a6 100%);
  color: #05231f;
}
.btn-secondary {
  background: rgba(148, 163, 184, 0.15);
  color: var(--text);
}
.summary {
  margin: 1rem 0 0;
  padding: 1rem;
  border-radius: 14px;
  background: var(--panel-strong);
  border: 1px solid var(--line);
  color: var(--dim);
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-word;
}
"#;

const APP_JS: &str = r#"
const DB_NAME = 'rockbot-web-bootstrap';
const STORE_NAME = 'identity';
const ID_KEY = 'client-identity';

function el(id) { return document.getElementById(id); }

function setPill(id, cls, text) {
  const node = el(id);
  node.className = `pill ${cls}`;
  node.textContent = text;
}

function openDb() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME);
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function idbGet(key) {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readonly');
    const store = tx.objectStore(STORE_NAME);
    const request = store.get(key);
    request.onsuccess = () => resolve(request.result ?? null);
    request.onerror = () => reject(request.error);
  });
}

async function idbSet(key, value) {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readwrite');
    tx.objectStore(STORE_NAME).put(value, key);
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
}

async function idbDelete(key) {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readwrite');
    tx.objectStore(STORE_NAME).delete(key);
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
}

function summarizeIdentity(identity) {
  if (!identity) {
    return 'No client identity stored.';
  }
  const certPreview = identity.certificate.split('\n').slice(0, 3).join('\n');
  return [
    `Stored at: ${new Date(identity.savedAt).toLocaleString()}`,
    '',
    'Certificate preview:',
    certPreview,
    '',
    `Private key bytes: ${identity.privateKey.length}`,
  ].join('\n');
}

async function refreshIdentity() {
  const identity = await idbGet(ID_KEY);
  setPill('identity-pill', identity ? 'pill-ok' : 'pill-idle', identity ? 'Identity ready' : 'No key imported');
  el('identity-summary').textContent = summarizeIdentity(identity);
}

async function refreshHealth() {
  try {
    const response = await fetch('/health');
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    const health = await response.json();
    setPill('health-pill', 'pill-ok', 'Healthy');
    el('health-text').textContent = `${health.status || 'ok'} · v${health.version || 'unknown'}`;
  } catch (error) {
    setPill('health-pill', 'pill-warn', 'Unavailable');
    el('health-text').textContent = String(error);
  }
}

async function readFileInput(input) {
  const file = input.files && input.files[0];
  if (!file) return null;
  return file.text();
}

async function saveIdentity() {
  const certPem = await readFileInput(el('cert-file'));
  const keyPem = await readFileInput(el('key-file'));
  if (!certPem || !keyPem) {
    setPill('identity-pill', 'pill-warn', 'Need cert + key');
    return;
  }
  await idbSet(ID_KEY, {
    certificate: certPem,
    privateKey: keyPem,
    savedAt: Date.now(),
  });
  await refreshIdentity();
}

async function clearIdentity() {
  await idbDelete(ID_KEY);
  el('cert-file').value = '';
  el('key-file').value = '';
  await refreshIdentity();
}

function bindDropzone() {
  const dropzone = el('dropzone');
  const prevent = (event) => {
    event.preventDefault();
    event.stopPropagation();
  };
  ['dragenter', 'dragover'].forEach((name) => {
    dropzone.addEventListener(name, (event) => {
      prevent(event);
      dropzone.classList.add('active');
    });
  });
  ['dragleave', 'drop'].forEach((name) => {
    dropzone.addEventListener(name, (event) => {
      prevent(event);
      dropzone.classList.remove('active');
    });
  });
  dropzone.addEventListener('drop', async (event) => {
    const files = Array.from(event.dataTransfer?.files || []);
    for (const file of files) {
      const text = await file.text();
      if (text.includes('BEGIN CERTIFICATE')) {
        await idbSet(ID_KEY, {
          ...(await idbGet(ID_KEY) || {}),
          certificate: text,
          savedAt: Date.now(),
        });
      } else if (text.includes('BEGIN') && text.includes('PRIVATE KEY')) {
        await idbSet(ID_KEY, {
          ...(await idbGet(ID_KEY) || {}),
          privateKey: text,
          savedAt: Date.now(),
        });
      }
    }
    await refreshIdentity();
  });
}

window.addEventListener('DOMContentLoaded', async () => {
  bindDropzone();
  el('save-btn').addEventListener('click', () => { void saveIdentity(); });
  el('clear-btn').addEventListener('click', () => { void clearIdentity(); });
  await refreshHealth();
  await refreshIdentity();
});
"#;
