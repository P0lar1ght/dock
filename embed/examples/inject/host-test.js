const logEl = document.querySelector('#log');
const statusEl = document.querySelector('#mount-status');
const lines = [];

function log(label, detail) {
  const time = new Date().toLocaleTimeString();
  const body = detail === undefined ? '' : ` ${JSON.stringify(detail)}`;
  lines.unshift(`${time} ${label}${body}`);
  logEl.textContent = lines.slice(0, 40).join('\n');
}

function agent() {
  return document.querySelector('dock-agent');
}

function refreshMountStatus() {
  const el = agent();
  if (el) {
    bindAgentEvents(el);
    statusEl.className = 'ok';
    statusEl.textContent = `已挂载 <dock-agent application="${el.getAttribute('application') || ''}" gateway-url="${el.getAttribute('gateway-url') || ''}">`;
    return el;
  }
  statusEl.className = 'warn';
  statusEl.textContent = 'dock-embed.js 已执行，但还没有 <dock-agent>';
  return null;
}

function bindAgentEvents(el) {
  if (!el || el.dataset.hostTestBound === 'true') return;
  el.dataset.hostTestBound = 'true';
  ['dock:ready', 'dock:state', 'dock:toggle', 'dock:error'].forEach((name) => {
    el.addEventListener(name, (event) => {
      refreshMountStatus();
      log(name, event.detail);
    });
  });
}

document.querySelector('#open-chat').addEventListener('click', async () => {
  const el = refreshMountStatus();
  if (!el) return log('host', { error: '没有 dock-agent' });
  try {
    await el.openChat();
    log('host.openChat', { open: el.open });
  } catch (error) {
    log('host.openChat.error', { message: error instanceof Error ? error.message : String(error) });
  }
});

document.querySelector('#close-chat').addEventListener('click', () => {
  const el = refreshMountStatus();
  if (!el) return log('host', { error: '没有 dock-agent' });
  el.closeChat();
  log('host.closeChat', { open: el.open });
});

document.querySelector('#connect').addEventListener('click', async () => {
  const el = refreshMountStatus();
  if (!el) return log('host', { error: '没有 dock-agent' });
  try {
    await el.connect();
    log('host.connect', { open: el.open });
  } catch (error) {
    log('host.connect.error', { message: error instanceof Error ? error.message : String(error) });
  }
});

window.addEventListener('error', (event) => {
  log('window.error', { message: event.message, file: event.filename });
});

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', refreshMountStatus, { once: true });
} else {
  refreshMountStatus();
}

queueMicrotask(refreshMountStatus);
setTimeout(refreshMountStatus, 250);
