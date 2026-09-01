import { DockClient } from '../../dist/client.js';

const applicationInput = document.querySelector('#application');
const gatewayInput = document.querySelector('#gateway-url');
const pairingIdInput = document.querySelector('#pairing-id');
const pairingHelp = document.querySelector('#pairing-help');
const pairingCommand = document.querySelector('#pairing-command');
const result = document.querySelector('#result');
const workspaceSelect = document.querySelector('#workspace');
const threadSelect = document.querySelector('#thread');
const messageInput = document.querySelector('#message');
const sessionResult = document.querySelector('#session');
const contextResult = document.querySelector('#context-result');

let client;
let removeSessionListener;
let selectedObject;

document.querySelector('#select-object-a').addEventListener('click', () => run(() => selectObject({
  id: 'object-a',
  name: 'Object A',
  status: 'reviewing'
})));

document.querySelector('#select-object-b').addEventListener('click', () => run(() => selectObject({
  id: 'object-b',
  name: 'Object B',
  status: 'approved'
})));

document.querySelector('#clear-object').addEventListener('click', () => run(async () => {
  selectedObject = undefined;
  contextResult.textContent = '当前对象：无';
  if (client) await client.clearContext({ source: 'fixture.selection' });
}));

document.querySelector('#connect').addEventListener('click', () => run(async () => {
  client = createClient();
  await showConnection(await client.connect());
}));

document.querySelector('#request-pairing').addEventListener('click', () => run(async () => {
  client = createClient();
  const pairing = await client.requestPairing();
  pairingIdInput.value = pairing.pairingRequestId;
  pairingHelp.hidden = false;
  pairingCommand.textContent = '请在 Dock 终端批准该来源';
  result.textContent = `配对申请已创建，有效期至 ${new Date(pairing.expiresAt).toLocaleTimeString()}。确认后会自动拿到 ticket。`;
}));

document.querySelector('#complete-pairing').addEventListener('click', () => run(async () => {
  client ||= createClient();
  const pairingRequestId = pairingIdInput.value.trim();
  await showConnection(await client.completePairing(pairingRequestId));
}));

document.querySelector('#list-threads').addEventListener('click', () => run(refreshThreads));

document.querySelector('#create-thread').addEventListener('click', () => run(async () => {
  requireClient();
  bindSession(await client.createThread({
    workspaceId: workspaceSelect.value,
    title: `HTTP Host ${new Date().toLocaleTimeString()}`
  }));
  await refreshThreads();
}));

document.querySelector('#open-thread').addEventListener('click', () => run(async () => {
  requireClient();
  bindSession(await client.switchThread(threadSelect.value, workspaceSelect.value));
}));

document.querySelector('#send-message').addEventListener('click', () => run(async () => {
  requireClient();
  if (!client.activeSession) throw new Error('请先创建或选择 Thread');
  await client.activeSession.startTurn(messageInput.value);
}));

workspaceSelect.addEventListener('change', () => run(async () => {
  requireClient();
  bindSession(await client.switchWorkspace(workspaceSelect.value));
  await refreshThreads();
}));

function createClient() {
  client?.disconnect();
  const next = new DockClient({
    application: applicationInput.value.trim(),
    gatewayUrl: gatewayInput.value.trim(),
    contextProvider: () => selectedObject ? [selectedContextItem()] : []
  });
  return next;
}

async function selectObject(object) {
  selectedObject = object;
  contextResult.textContent = `当前对象：${object.name}（${object.status}）`;
  if (client) await client.setContext([selectedContextItem()]);
}

function selectedContextItem() {
  return {
    id: 'current-object',
    type: 'selected_resource',
    title: selectedObject.name,
    summary: `宿主页面当前选择了 ${selectedObject.name}`,
    source: 'fixture.selection',
    entityRef: { kind: 'fixture-object', id: selectedObject.id },
    data: { name: selectedObject.name, status: selectedObject.status },
    priority: 90,
    timestamp: Date.now()
  };
}

async function showConnection(snapshot) {
  result.textContent = JSON.stringify({
    connected: true,
    server: snapshot.initialize.serverInfo,
    protocolVersion: snapshot.initialize.protocolVersion,
    application: snapshot.initialize.connection.application,
    origin: snapshot.initialize.connection.origin,
    defaultWorkspaceId: snapshot.defaultWorkspaceId,
    workspaces: snapshot.workspaces
  }, null, 2);
  replaceOptions(workspaceSelect, snapshot.workspaces.map((workspace) => ({
    value: workspace.id,
    label: workspace.default ? `${workspace.id}（默认）` : workspace.id
  })));
  workspaceSelect.value = client.activeWorkspaceId || snapshot.defaultWorkspaceId;
  bindSession(client.activeSession);
  await refreshThreads();
}

async function refreshThreads() {
  requireClient();
  const threads = await client.listThreads(workspaceSelect.value);
  replaceOptions(threadSelect, threads.map((thread) => ({ value: thread.id, label: thread.title })));
  if (client.activeSession?.workspaceId === workspaceSelect.value) {
    threadSelect.value = client.activeSession.id;
  }
}

function bindSession(session) {
  removeSessionListener?.();
  removeSessionListener = undefined;
  if (!session) {
    sessionResult.textContent = '此 Workspace 尚未选择 Thread';
    return;
  }
  removeSessionListener = session.onChange((state) => {
    sessionResult.textContent = JSON.stringify({
      thread: state.thread,
      connection: state.connection,
      lastSeq: state.lastSeq,
      turns: state.turns,
      messages: state.messages.map(({ role, content, status, turnId }) => ({ role, content, status, turnId }))
    }, null, 2);
  });
}

function replaceOptions(select, items) {
  select.replaceChildren(...items.map((item) => {
    const option = document.createElement('option');
    option.value = item.value;
    option.textContent = item.label;
    return option;
  }));
}

function requireClient() {
  if (!client) throw new Error('请先连接 Gateway');
}

async function run(operation) {
  try {
    await operation();
  } catch (error) {
    result.textContent = JSON.stringify({
      connected: false,
      code: error?.code || 'unexpected_error',
      message: error instanceof Error ? error.message : String(error)
    }, null, 2);
  }
}
