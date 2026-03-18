const DB_NAME = 'rockbot-web-bootstrap';
const STORE_NAME = 'identity';
const ID_KEY = 'client-identity';
let activeSocket = null;

function el(id) {
  return document.getElementById(id);
}

function setPill(id, cls, text) {
  const node = el(id);
  if (!node) return;
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
    `Private key: ${identity.privateKey instanceof CryptoKey ? 'stored as non-extractable CryptoKey' : 'legacy PEM'}`,
  ].join('\n');
}

function updateWsAuth(text, cls) {
  const node = el('ws-auth-text');
  if (node) node.textContent = text;
  setPill('identity-pill', cls, cls === 'pill-ok' ? 'Identity ready' : el('identity-pill')?.textContent || text);
}

async function refreshIdentity() {
  const identity = await idbGet(ID_KEY);
  setPill('identity-pill', identity ? 'pill-ok' : 'pill-idle', identity ? 'Identity ready' : 'No key imported');
  const summary = el('identity-summary');
  if (summary) {
    summary.textContent = summarizeIdentity(identity);
  }
  if (identity) {
    void ensureAuthenticatedSocket(identity);
  } else {
    updateWsAuth('No stored identity', 'pill-idle');
  }
}

async function refreshHealth() {
  try {
    const response = await fetch('/health');
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    const health = await response.json();
    setPill('gateway-status', 'pill-ok', 'Healthy');
    const node = el('health-text');
    if (node) {
      node.textContent = `${health.status || 'ok'} · v${health.version || 'unknown'}`;
    }
  } catch (error) {
    setPill('gateway-status', 'pill-warn', 'Unavailable');
    const node = el('health-text');
    if (node) {
      node.textContent = String(error);
    }
  }
}

async function readFileInput(input) {
  const file = input.files && input.files[0];
  if (!file) return null;
  return file.text();
}

function pemBody(pem) {
  return pem
    .replace(/-----BEGIN [^-]+-----/g, '')
    .replace(/-----END [^-]+-----/g, '')
    .replace(/\s+/g, '');
}

function base64ToArrayBuffer(base64) {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

function arrayBufferToBase64(buffer) {
  const bytes = new Uint8Array(buffer);
  let binary = '';
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

async function importPrivateKey(privateKeyPem) {
  return window.crypto.subtle.importKey(
    'pkcs8',
    base64ToArrayBuffer(pemBody(privateKeyPem)),
    { name: 'ECDSA', namedCurve: 'P-256' },
    false,
    ['sign'],
  );
}

async function signChallengeWithKey(privateKey, challengeBase64) {
  const signature = await window.crypto.subtle.sign(
    { name: 'ECDSA', hash: 'SHA-256' },
    privateKey,
    base64ToArrayBuffer(challengeBase64),
  );
  return arrayBufferToBase64(signature);
}

async function saveIdentity() {
  const certPem = await readFileInput(el('cert-file'));
  const keyPem = await readFileInput(el('key-file'));
  if (!certPem || !keyPem) {
    setPill('identity-pill', 'pill-warn', 'Need cert + key');
    return;
  }

  const privateKey = await importPrivateKey(keyPem);
  await idbSet(ID_KEY, {
    certificate: certPem,
    privateKey,
    savedAt: Date.now(),
  });
  await refreshIdentity();
}

async function clearIdentity() {
  if (activeSocket) {
    activeSocket.close();
    activeSocket = null;
  }
  await idbDelete(ID_KEY);
  const certInput = el('cert-file');
  const keyInput = el('key-file');
  if (certInput) certInput.value = '';
  if (keyInput) keyInput.value = '';
  updateWsAuth('Not connected', 'pill-idle');
  await refreshIdentity();
}

async function ensureAuthenticatedSocket(identity) {
  if (activeSocket && activeSocket.readyState === WebSocket.OPEN) {
    return;
  }

  const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws';
  const socket = new WebSocket(`${scheme}://${window.location.host}/ws`);
  activeSocket = socket;
  updateWsAuth('Connecting...', 'pill-idle');

  socket.addEventListener('open', () => {
    socket.send(JSON.stringify({
      type: 'web_auth_begin',
      certificate_pem: identity.certificate,
    }));
  });

  socket.addEventListener('message', async (event) => {
    const payload = JSON.parse(event.data);
    if (payload.type === 'web_auth_challenge') {
      try {
        const signature = await signChallengeWithKey(identity.privateKey, payload.challenge);
        socket.send(JSON.stringify({
          type: 'web_auth_complete',
          signature,
        }));
      } catch (error) {
        updateWsAuth(`Key import failed: ${error}`, 'pill-warn');
      }
    } else if (payload.type === 'web_auth_result') {
      if (payload.authenticated) {
        updateWsAuth(`Authenticated as ${payload.cert_name} (${payload.cert_role})`, 'pill-ok');
      } else {
        updateWsAuth(payload.message || 'Authentication failed', 'pill-warn');
      }
    }
  });

  socket.addEventListener('close', () => {
    if (activeSocket === socket) {
      activeSocket = null;
      updateWsAuth('Disconnected', 'pill-warn');
    }
  });

  socket.addEventListener('error', () => {
    updateWsAuth('WebSocket error', 'pill-warn');
  });
}

function bindDropzone() {
  const dropzone = el('dropzone');
  if (!dropzone) return;

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
    const existing = await idbGet(ID_KEY) || {};
    let updated = { ...existing, savedAt: Date.now() };
    for (const file of files) {
      const text = await file.text();
      if (text.includes('BEGIN CERTIFICATE')) {
        updated.certificate = text;
      } else if (text.includes('BEGIN') && text.includes('PRIVATE KEY')) {
        updated.privateKey = await importPrivateKey(text);
      }
    }
    await idbSet(ID_KEY, updated);
    await refreshIdentity();
  });
}

window.addEventListener('DOMContentLoaded', async () => {
  bindDropzone();
  el('save-btn')?.addEventListener('click', () => { void saveIdentity(); });
  el('clear-btn')?.addEventListener('click', () => { void clearIdentity(); });
  await refreshHealth();
  await refreshIdentity();
});
