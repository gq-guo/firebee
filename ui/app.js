// Firebee 前端：全部 UI 状态在此，后端（Rust）只负责存储 / HTTP / 变量替换 / 导出。
// 数据结构与 core/models.rs 的 serde 形态一致（Auth 外部标签枚举、方法名 "Get" 等）。
const invoke = window.__TAURI__.core.invoke;
const $ = (s) => document.querySelector(s);
const MAC = navigator.platform.startsWith('Mac');
const MOD = MAC ? '⌘' : 'Ctrl+';

const METHODS = ['Get', 'Post', 'Put', 'Delete', 'Patch', 'Head', 'Options'];
const HISTORY_LIMIT = 500;
const REASON = { 200: 'OK', 201: 'Created', 204: 'No Content', 301: 'Moved Permanently', 302: 'Found', 304: 'Not Modified',
  400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden', 404: 'Not Found', 405: 'Method Not Allowed', 408: 'Timeout',
  409: 'Conflict', 422: 'Unprocessable', 429: 'Too Many Requests', 500: 'Server Error', 502: 'Bad Gateway', 503: 'Unavailable', 504: 'Gateway Timeout' };

// ---------- 状态 ----------
let data = { collections: [], environments: [], history: [] };
let activeEnvId = null;
let current = newRequest();
let response = null;   // { ok: dto } | { error: string } | { cancelled: true } | null
let pending = null;    // 进行中的 job_id
let jobSeq = 0;
let missing = [];
let reqTab = 'params', respTab = 'body', sideTab = 'collections';
let renaming = null;   // 正在重命名的对象（collection / folder / request）
let envSel = null;     // env 对话框中选中的环境
let saveTimer = null;
let filter = '';       // 侧栏搜索关键字（匹配集合/文件夹/请求名、URL）
const collapsed = new Set(); // 用户折叠过的 collection/folder id（重绘时保持）

function newRequest(name = 'Untitled request') {
  return { id: crypto.randomUUID(), name, method: 'Get', url: '', params: [], headers: [],
           body_type: 'None', body: '', form: [], auth: 'None' };
}
const newContainer = (name) => ({ id: crypto.randomUUID(), name, folders: [], requests: [] });
const kv = () => ({ enabled: true, key: '', value: '' });
const activeEnv = () => data.environments.find((e) => e.id === activeEnvId) || null;

// 防抖保存集合与环境（500ms）。成功无提示；失败必须让用户知道。
function dirty() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      await invoke('save_collections', { collections: data.collections });
      await invoke('save_environments', { environments: data.environments });
    } catch (e) { toast(`Couldn't save your changes. ${e}`, { error: true, action: ['Retry', dirty] }); }
  }, 500);
}

// ---------- DOM 助手 ----------
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') el.className = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (v !== null && v !== undefined && v !== false) el.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}
/** replaceChildren 会把 null 变成 "null" 文本，这里过滤掉 */
const put = (el, ...kids) => el.replaceChildren(...kids.filter((k) => k !== null && k !== undefined && k !== false));
const btn = (label, onclick, cls = 'small') => h('button', { class: cls, onclick }, label);
const methodTag = (m) => h('span', { class: `method m-${m.toLowerCase()}` }, m.toUpperCase());

/** 复制：按钮文字换成 Copied ✓ 两秒半，不弹 toast */
function copyText(text, button) {
  const write = navigator.clipboard?.writeText(text) ?? Promise.reject();
  write.catch(() => {
    const ta = h('textarea', {}, text);
    document.body.append(ta); ta.select(); document.execCommand('copy'); ta.remove();
  }).finally(() => {
    if (!button) return;
    const label = button.textContent;
    button.dataset.state = 'copied'; button.textContent = 'Copied ✓';
    setTimeout(() => { delete button.dataset.state; button.textContent = label; }, 2500);
  });
}

/** 右下角 toast：错误、或带 Undo 的可撤销操作。悬停时不消失。 */
let toastTimer = null;
function toast(message, { error = false, action = null, ms = 6000 } = {}) {
  const el = $('#toast');
  clearTimeout(toastTimer);
  el.className = error ? 'error' : '';
  put(el, h('span', {}, message),
    action && btn(action[0], () => { hideToast(); action[1](); }),
    btn('×', hideToast, 'small ghost'));
  const arm = () => { toastTimer = setTimeout(hideToast, ms); };
  el.onmouseenter = () => clearTimeout(toastTimer); el.onmouseleave = arm;
  arm();
}
function hideToast() { clearTimeout(toastTimer); $('#toast').classList.add('hidden'); }

/** 右键 / ⋯ 菜单：items = [[label, fn, {danger, kbd}], ...]，点击任意处或 Esc 关闭 */
function openMenu(e, items) {
  e.preventDefault(); e.stopPropagation();
  const menu = $('#menu');
  menu.replaceChildren(...items.map(([label, fn, opt = {}]) =>
    h('button', { class: 'menu-item' + (opt.danger ? ' danger-item' : ''), role: 'menuitem',
      onclick: (ev) => { ev.stopPropagation(); menu.classList.add('hidden'); fn(ev); } },
      label, opt.kbd && h('span', { class: 'kbd' }, opt.kbd))));
  const x = e.clientX || e.target.getBoundingClientRect().left, y = e.clientY || e.target.getBoundingClientRect().bottom;
  menu.style.left = `${Math.min(x, window.innerWidth - 180)}px`;
  menu.style.top = `${Math.min(y, window.innerHeight - items.length * 30 - 12)}px`;
  menu.classList.remove('hidden');
  menu.firstElementChild?.focus();
}
document.addEventListener('click', () => $('#menu').classList.add('hidden'));
document.addEventListener('keydown', (e) => {
  const menu = $('#menu');
  if (e.key === 'Escape') menu.classList.add('hidden');
  if (!menu.classList.contains('hidden') && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
    e.preventDefault();
    const items = [...menu.children], i = items.indexOf(document.activeElement);
    items[(i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length].focus();
  }
});

/** 通用 key-value 表格；增删行时调 rerender 重建，输入只写模型不重绘（保焦点）。 */
function kvTable(rows, keyHint, valHint, rerender) {
  const table = h('table', { class: 'kv' });
  rows.forEach((r, i) => {
    const tr = h('tr', { class: r.enabled ? '' : 'off' },
      h('td', { class: 'ctl' }, h('input', { type: 'checkbox', checked: r.enabled, 'aria-label': 'Enabled',
        onchange: (e) => { r.enabled = e.target.checked; tr.classList.toggle('off', !r.enabled); dirty(); renderTabCounts(); } })),
      h('td', { class: 'key' }, h('input', { placeholder: keyHint, value: r.key, 'aria-label': keyHint, spellcheck: 'false',
        oninput: (e) => { r.key = e.target.value; dirty(); renderTabCounts(); } })),
      h('td', { class: 'val' }, h('input', { placeholder: valHint, value: r.value, 'aria-label': valHint, spellcheck: 'false',
        oninput: (e) => { r.value = e.target.value; dirty(); } })),
      h('td', { class: 'ctl' }, btn('×', () => { rows.splice(i, 1); dirty(); rerender(); }, 'small ghost')),
    );
    tr.lastChild.firstChild.setAttribute('aria-label', 'Remove row');
    table.append(tr);
  });
  const add = btn('+ Add row', () => { rows.push(kv()); dirty(); rerender(); focusLastKey(); }, 'small kv-add');
  const focusLastKey = () => setTimeout(() => document.querySelector('table.kv tr:last-child td.key input')?.focus(), 0);
  return h('div', {}, table, add);
}

// ---------- 顶栏 ----------
function renderTopbar() {
  const sel = $('#env-select');
  sel.replaceChildren(
    h('option', { value: '' }, 'No environment'),
    ...data.environments.map((e) => h('option', { value: e.id }, e.name)),
  );
  sel.value = activeEnvId || '';
}

// ---------- 侧边栏 ----------
function setTab(attr, value) {
  document.querySelectorAll(`[data-${attr}]`).forEach((b) => {
    const on = b.dataset[attr] === value;
    b.classList.toggle('active', on); b.setAttribute('aria-selected', on);
  });
}

function emptyState(title, why, action) {
  return h('div', { class: 'empty' }, h('strong', {}, title), h('span', {}, why), action && btn(action[0], action[1], ''));
}

function renderSidebar() {
  setTab('side', sideTab);
  const body = $('#side-body');
  body.replaceChildren();
  if (sideTab === 'history') {
    if (!data.history.length) { body.append(emptyState('No requests sent yet', 'Every request you send is kept here, so you can reopen it later.')); return; }
    const shown = data.history.filter((x) => !q() || hit(x.request.name) || hit(x.request.url));
    if (!shown.length) { body.append(emptyState(`No history matches “${filter.trim()}”`, 'Try part of a URL or a request name.')); return; }
    for (const hist of shown.reverse()) {
      const t = new Date(hist.timestamp);
      const hhmm = `${String(t.getHours()).padStart(2, '0')}:${String(t.getMinutes()).padStart(2, '0')}`;
      body.append(h('div', { class: 'hist', role: 'button', tabindex: 0, title: `${hist.request.method.toUpperCase()} ${hist.request.url}`,
        onclick: () => selectRequest(structuredClone(hist.request)), onkeydown: activate },
        h('span', { class: 'muted' }, hhmm), methodTag(hist.request.method),
        h('span', { class: hist.status ? `s${Math.floor(hist.status / 100)}` : 's0' }, hist.status ?? 'ERR'),
        h('span', {}, hist.request.url || '(no URL)')));
    }
    return;
  }
  const addCol = () => { data.collections.push(newContainer(`Collection ${data.collections.length + 1}`)); dirty(); renderSidebar(); };
  if (!data.collections.length) {
    body.append(emptyState('No collections yet', 'A collection keeps the requests you want to reuse, in folders if you like.', ['Create a collection', addCol]));
    return;
  }
  const shown = data.collections.filter((c) => filterView(c).show);
  if (q() && !shown.length) { body.append(emptyState(`Nothing matches “${filter.trim()}”`, 'Names of collections, folders and requests are searched, and request URLs.')); return; }
  body.append(btn('+ New collection', addCol, 'small'));
  shown.forEach((c) => body.append(containerNode(c, data.collections)));
  body.querySelector('input.rename')?.focus();
}

// ---------- 搜索过滤 ----------
const q = () => filter.trim().toLowerCase();
const hit = (s) => !!q() && (s || '').toLowerCase().includes(q());
/** 容器名命中 → 整个子树都显示；否则只显示命中的后代，且至少有一个才显示容器本身 */
function filterView(c) {
  if (!q() || hit(c.name)) return { folders: c.folders, requests: c.requests, show: true };
  const folders = c.folders.filter((f) => filterView(f).show);
  const requests = c.requests.filter((r) => hit(r.name) || hit(r.url));
  return { folders, requests, show: folders.length + requests.length > 0 };
}

/** Enter / Space 触发点击（给 role=button 的 div 用） */
function activate(e) { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.currentTarget.click(); } }

/** 名称：正在重命名则显示输入框（Enter 提交 / Esc 取消 / 失焦提交）；双击也可重命名 */
function nameNode(obj) {
  if (renaming !== obj) return h('span', { class: 'name', title: obj.name, ondblclick: startRename(obj) }, obj.name);
  const commit = (e) => {
    if (renaming !== obj) return; // Enter 后 blur 会再触发一次
    const v = e.target.value.trim();
    if (v) { obj.name = v; dirty(); }
    renaming = null; renderSidebar(); renderRequestHeader();
  };
  return h('input', { class: 'rename', value: obj.name, 'aria-label': 'New name',
    onclick: (e) => e.stopPropagation(),
    onblur: commit,
    onkeydown: (e) => {
      e.stopPropagation();
      if (e.key === 'Enter') commit(e);
      else if (e.key === 'Escape') { renaming = null; renderSidebar(); }
    } });
}

const startRename = (obj) => (e) => { e.stopPropagation(); renaming = obj; renderSidebar(); };
/** 乐观删除 + Undo toast，不弹确认框 */
const remove = (arr, obj, what) => () => {
  const idx = arr.indexOf(obj);
  arr.splice(idx, 1); dirty(); renderSidebar(); renderRequestHeader();
  toast(`Deleted ${what} “${obj.name}”.`, { action: ['Undo', () => { arr.splice(idx, 0, obj); dirty(); renderSidebar(); renderRequestHeader(); }] });
};

/** 集合或文件夹节点（结构相同：name / folders / requests） */
function containerNode(c, parentArr, isFolder = false) {
  const menu = (e) => openMenu(e, [
    ['New request', () => { const r = newRequest(); c.requests.push(r); collapsed.delete(c.id); dirty(); selectRequest(r); }],
    ['New folder', () => { c.folders.push(newContainer('New folder')); collapsed.delete(c.id); dirty(); renderSidebar(); }],
    ['Rename', startRename(c)],
    ['Delete', remove(parentArr, c, isFolder ? 'folder' : 'collection'), { danger: true }],
  ]);
  const view = filterView(c);
  return h('details', { open: q() ? true : !collapsed.has(c.id), ontoggle: (e) => { if (!q()) e.target.open ? collapsed.delete(c.id) : collapsed.add(c.id); } },
    h('summary', { oncontextmenu: menu, onclick: (e) => { if (renaming === c) e.preventDefault(); } },
      nameNode(c), btn('⋯', menu, 'small more').withAttr('aria-label', `${isFolder ? 'Folder' : 'Collection'} actions`)),
    ...view.folders.map((f) => containerNode(f, c.folders, true)),
    ...view.requests.map((r) => {
      const rmenu = (e) => openMenu(e, [
        ['Duplicate', () => { const d = structuredClone(r); d.id = crypto.randomUUID(); d.name += ' copy'; c.requests.splice(c.requests.indexOf(r) + 1, 0, d); dirty(); selectRequest(d); }],
        ['Rename', startRename(r)],
        ['Delete', remove(c.requests, r, 'request'), { danger: true }],
      ]);
      return h('div', { class: 'req' + (r === current ? ' active' : ''), role: 'button', tabindex: 0, 'aria-current': r === current || undefined,
        onclick: () => selectRequest(r), onkeydown: activate, oncontextmenu: rmenu },
        methodTag(r.method), nameNode(r), btn('⋯', rmenu, 'small more').withAttr('aria-label', 'Request actions'));
    }),
  );
}
Element.prototype.withAttr = function (k, v) { if (v !== undefined) this.setAttribute(k, v); return this; };

/** 选中请求：来自集合时直接引用（编辑即写回集合并保存），来自历史时为副本。 */
function selectRequest(r) {
  current = r; response = null; missing = [];
  renderRequest(); renderResponse(); renderSidebar();
}

/** 当前请求在集合树中的位置（面包屑）；不在任何集合里返回 null */
function locate(r, nodes = data.collections, path = []) {
  for (const n of nodes) {
    if (n.requests.includes(r)) return [...path, n.name];
    const deep = locate(r, n.folders, [...path, n.name]);
    if (deep) return deep;
  }
  return null;
}

/** 把未保存的请求存进某个集合（引用进树，从此自动保存） */
function saveMenu(e) {
  const into = (c) => { c.requests.push(current); collapsed.delete(c.id); dirty(); selectRequest(current); };
  openMenu(e, [
    ...data.collections.map((c) => [c.name, () => into(c)]),
    ['+ New collection', () => { const c = newContainer(`Collection ${data.collections.length + 1}`); data.collections.push(c); into(c); }],
  ]);
}

// ---------- 请求面板 ----------
function renderRequestHeader() {
  $('#req-name').value = current.name;
  const where = locate(current);
  $('#req-where').textContent = where ? `in ${where.join(' / ')}` : 'not saved to a collection';
  $('#save').classList.toggle('hidden', !!where);
  $('#method').value = current.method;
  $('#url').value = current.url;
  $('#send').classList.toggle('hidden', pending !== null);
  $('#cancel').classList.toggle('hidden', pending === null);
  const m = $('#missing');
  m.classList.toggle('hidden', missing.length === 0);
  m.replaceChildren(missing.length ? h('span', {}, `⚠ ${missing.join(', ')} ${missing.length > 1 ? 'are' : 'is'} not defined in the active environment. `,
    h('strong', {}, 'Send again to send with the placeholders as-is.')) : '');
  renderTabCounts();
}

function renderTabCounts() {
  const count = (rows) => rows.filter((r) => r.enabled && r.key).length;
  const n = { params: count(current.params), headers: count(current.headers),
    body: current.body_type === 'None' ? 0 : current.body_type === 'Form' ? count(current.form) : (current.body.trim() ? 1 : 0),
    auth: current.auth === 'None' ? 0 : 1 };
  document.querySelectorAll('[data-req]').forEach((b) => {
    const key = b.dataset.req;
    put(b, key[0].toUpperCase() + key.slice(1), n[key] ? h('span', { class: 'n' }, n[key]) : null);
  });
}

function renderRequest() {
  renderRequestHeader();
  setTab('req', reqTab);
  const body = $('#req-body');
  body.replaceChildren();
  if (reqTab === 'params') body.append(kvTable(current.params, 'Key', 'Value', renderRequest));
  else if (reqTab === 'headers') body.append(kvTable(current.headers, 'Header', 'Value', renderRequest));
  else if (reqTab === 'body') body.append(bodyEditor());
  else body.append(authEditor());
}

function radios(name, options, value, onchange) {
  return h('div', { class: 'row', role: 'radiogroup' }, ...options.map(([v, label]) =>
    h('label', {}, h('input', { type: 'radio', name, value: v, checked: v === value, onchange: () => onchange(v) }), label)));
}

function bodyEditor() {
  const wrap = h('div', { class: 'fill' }, radios('body_type', [['None', 'None'], ['Json', 'JSON'], ['Text', 'Text'], ['Form', 'Form']],
    current.body_type, (v) => { current.body_type = v; dirty(); renderRequest(); }));
  if (current.body_type === 'Json' || current.body_type === 'Text') {
    const helper = h('div', { class: 'helper' });
    const ta = h('textarea', { spellcheck: 'false', 'aria-label': 'Request body', placeholder: current.body_type === 'Json' ? '{ "key": "value" }' : 'Raw text body',
      oninput: (e) => { current.body = e.target.value; ta.removeAttribute('aria-invalid'); helper.className = 'helper'; helper.textContent = ''; dirty(); renderTabCounts(); } }, current.body);
    wrap.append(ta, helper);
    if (current.body_type === 'Json') {
      wrap.firstChild.append(h('span', { class: 'spacer' }), btn('Format JSON', () => {
        try { current.body = ta.value = JSON.stringify(JSON.parse(ta.value), null, 2); dirty(); helper.className = 'helper'; helper.textContent = ''; ta.removeAttribute('aria-invalid'); }
        catch (err) { ta.setAttribute('aria-invalid', 'true'); helper.className = 'helper error'; helper.textContent = `Not valid JSON — ${err.message}. Fix it, then format again.`; }
      }));
    }
  } else if (current.body_type === 'Form') {
    wrap.append(kvTable(current.form, 'Field', 'Value', renderRequest));
  }
  return wrap;
}

const authKind = (a) => (a === 'None' ? 'None' : Object.keys(a)[0]);
function authEditor() {
  const kind = authKind(current.auth);
  const wrap = h('div', {}, radios('auth', [['None', 'None'], ['Bearer', 'Bearer token'], ['Basic', 'Basic auth'], ['ApiKey', 'API key']], kind, (v) => {
    current.auth = { None: 'None', Bearer: { Bearer: { token: '' } }, Basic: { Basic: { username: '', password: '' } },
                     ApiKey: { ApiKey: { key: '', value: '', in_query: false } } }[v];
    dirty(); renderRequest();
  }));
  const field = (obj, key, label, type = 'text') => h('label', {}, `${label}`,
    h('input', { type, value: obj[key], spellcheck: 'false', 'aria-label': label, oninput: (e) => { obj[key] = e.target.value; dirty(); } }));
  const a = current.auth[kind];
  if (kind === 'Bearer') wrap.append(h('div', { class: 'row' }, field(a, 'token', 'Token')));
  else if (kind === 'Basic') wrap.append(h('div', { class: 'row' }, field(a, 'username', 'Username'), field(a, 'password', 'Password', 'password')));
  else if (kind === 'ApiKey') wrap.append(h('div', { class: 'row' }, field(a, 'key', 'Header or param name'), field(a, 'value', 'Value'),
    h('label', {}, h('input', { type: 'checkbox', checked: a.in_query, onchange: (e) => { a.in_query = e.target.checked; dirty(); } }), 'Send as query param instead of header')));
  return wrap;
}

// ---------- 发送 / 取消 ----------
async function send() {
  if (pending !== null) return;
  if (!current.url.trim()) { $('#url').focus(); return; }
  const m = await invoke('missing_vars', { request: current, env: activeEnv() });
  // 有未定义变量：第一次点击只提示，第二次（列表未变）强制发送
  if (m.length && JSON.stringify(m) !== JSON.stringify(missing)) { missing = m; renderRequestHeader(); return; }
  missing = [];
  const id = ++jobSeq;
  pending = id; response = null;
  renderRequestHeader(); renderResponse();
  const req = structuredClone(current);
  let status = null, duration_ms = null;
  try {
    const r = await invoke('send_request', { job_id: id, request: req, env: activeEnv(), timeout_secs: Number($('#timeout').value) || 30 });
    response = { ok: r }; status = r.status; duration_ms = r.duration_ms;
  } catch (e) {
    response = /cancelled/i.test(String(e)) ? { cancelled: true } : { error: String(e) };
  }
  if (pending === id) pending = null;
  data.history.push({ timestamp: new Date().toISOString(), request: req, status, duration_ms });
  if (data.history.length > HISTORY_LIMIT) data.history.splice(0, data.history.length - HISTORY_LIMIT);
  invoke('save_history', { history: data.history }).catch((e) => toast(`Couldn't save history. ${e}`, { error: true }));
  renderRequestHeader(); renderResponse();
  if (sideTab === 'history') renderSidebar();
}

// ---------- 响应面板 ----------
const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
// ponytail: 正则着色，>1MB 直接纯文本
function highlightJson(s) {
  if (s.length > 1_000_000) return esc(s);
  return esc(s).replace(/("(?:\\.|[^"\\])*"\s*:?|\b(?:true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g, (m) => {
    const cls = m[0] === '"' ? (m.endsWith(':') ? 'key' : 'str') : m === 'null' ? 'null' : /^(true|false)$/.test(m) ? 'bool' : 'num';
    return `<span class="${cls}">${m}</span>`;
  });
}
const fmtSize = (n) => n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : n >= 1024 ? `${(n / 1024).toFixed(1)} KB` : `${n} B`;

function renderResponse() {
  const meta = $('#resp-meta'), body = $('#resp-body'), tabs = $('#resp-tabs');
  meta.replaceChildren(); body.replaceChildren();
  tabs.classList.toggle('hidden', !response?.ok);
  if (!response) {
    if (pending !== null) meta.append(h('span', { class: 'spinner' }), h('span', { class: 'muted' }, 'Sending…'));
    else body.append(emptyState('Response will show here', `Fill in a URL and press Send, or ${MOD}↩ from anywhere in the editor.`));
    return;
  }
  if (response.cancelled) { meta.append(h('span', { class: 'muted' }, 'Cancelled — nothing was received.')); return; }
  if (response.error) { meta.append(h('span', { class: 'error' }, h('span', {}, '⚠'), h('span', {}, response.error))); return; }
  const r = response.ok;
  const copy = btn('Copy body', () => copyText(r.body, copy));
  meta.append(
    h('span', { class: `status s${Math.floor(r.status / 100)}` }, `${r.status} ${REASON[r.status] || ''}`.trim()),
    h('span', { class: 'meta' }, `${r.duration_ms} ms`), h('span', { class: 'meta' }, fmtSize(r.size_bytes)),
    h('span', { class: 'spacer' }), copy,
  );
  setTab('resp', respTab);
  if (respTab === 'headers') {
    body.append(h('table', { class: 'hdrs' }, ...r.headers.map(([k, v]) => h('tr', {}, h('td', {}, k), h('td', {}, v)))));
    return;
  }
  if (!r.body) { body.append(h('span', { class: 'muted' }, 'Empty body.')); return; }
  let text = r.body, isJson = false;
  try { text = JSON.stringify(JSON.parse(r.body), null, 2); isJson = true; } catch { /* not JSON */ }
  const pre = h('pre', {});
  if (isJson) pre.innerHTML = highlightJson(text); else pre.textContent = text;
  body.append(pre);
}

// ---------- 环境管理对话框 ----------
function renderEnvDialog() {
  const list = $('#env-list'), editor = $('#env-editor');
  list.replaceChildren(...[
    ...data.environments.map((e) => btn(e.name, () => { envSel = e; renderEnvDialog(); }, 'item' + (e === envSel ? ' active' : ''))),
    btn('+ New environment', () => {
      envSel = { id: crypto.randomUUID(), name: `Environment ${data.environments.length + 1}`, variables: [] };
      data.environments.push(envSel); dirty(); renderEnvDialog(); renderTopbar();
      setTimeout(() => $('#env-editor input')?.select(), 0);
    }, 'small'),
    envSel && btn('Delete', () => {
      const env = envSel, idx = data.environments.indexOf(env);
      data.environments.splice(idx, 1);
      const wasActive = activeEnvId === env.id;
      if (wasActive) activeEnvId = null;
      envSel = null; dirty(); renderEnvDialog(); renderTopbar();
      toast(`Deleted environment “${env.name}”.`, { action: ['Undo', () => { data.environments.splice(idx, 0, env); if (wasActive) activeEnvId = env.id; envSel = env; dirty(); renderEnvDialog(); renderTopbar(); }] });
    }, 'small'),
  ].filter(Boolean));
  editor.replaceChildren();
  if (!envSel) { editor.append(emptyState('Pick an environment', 'Variables here replace {{name}} in URLs, headers, bodies and auth.')); return; }
  editor.append(
    h('input', { value: envSel.name, 'aria-label': 'Environment name', oninput: (e) => { envSel.name = e.target.value; dirty(); renderTopbar(); $('#env-list .item.active').textContent = envSel.name; } }),
    h('p', { class: 'muted' }, 'Variables — use them as {{name}} anywhere in a request.'),
    kvTable(envSel.variables, 'Name', 'Value', renderEnvDialog),
  );
}

// ---------- 事件绑定 ----------
function bind() {
  $('#method').replaceChildren(...METHODS.map((m) => h('option', { value: m }, m.toUpperCase())));
  $('#method').onchange = (e) => { current.method = e.target.value; dirty(); renderSidebar(); };
  $('#url').oninput = (e) => { current.url = e.target.value; dirty(); };
  $('#url').onkeydown = (e) => { if (e.key === 'Enter') send(); };
  $('#req-name').oninput = (e) => { current.name = e.target.value; dirty(); renderSidebar(); };
  $('#req-name').onkeydown = (e) => { if (e.key === 'Enter') e.target.blur(); };
  $('#save').onclick = saveMenu;
  $('#send').onclick = send;
  $('#cancel').onclick = () => invoke('cancel_request', { job_id: pending });
  $('#export').onchange = async (e) => {
    const kind = e.target.value; e.target.value = '';
    if (!kind) return;
    $('#export-text').textContent = await invoke('export_code', { request: current, env: activeEnv(), kind });
    $('#export-dialog').showModal();
  };
  $('#export-copy').onclick = (e) => copyText($('#export-text').textContent, e.currentTarget);
  $('#export-close').onclick = () => $('#export-dialog').close();
  $('#env-select').onchange = (e) => { activeEnvId = e.target.value || null; missing = []; renderRequestHeader(); };
  $('#env-manage').onclick = () => { renderEnvDialog(); $('#env-dialog').showModal(); };
  $('#env-close').onclick = () => $('#env-dialog').close();
  document.querySelectorAll('[data-side]').forEach((b) => b.onclick = () => { sideTab = b.dataset.side; renderSidebar(); });
  $('#search').oninput = (e) => { filter = e.target.value; renderSidebar(); };
  $('#search').onkeydown = (e) => { if (e.key === 'Escape' && filter) { e.stopPropagation(); filter = e.target.value = ''; renderSidebar(); } };
  document.querySelectorAll('[data-req]').forEach((b) => b.onclick = () => { reqTab = b.dataset.req; renderRequest(); });
  document.querySelectorAll('[data-resp]').forEach((b) => b.onclick = () => { respTab = b.dataset.resp; renderResponse(); });
  // 全局快捷键：⌘↩ 发送 · ⌘S 保存到集合 · ⌘N 新请求
  document.addEventListener('keydown', (e) => {
    const mod = MAC ? e.metaKey : e.ctrlKey;
    if (!mod) return;
    if (e.key === 'Enter') { e.preventDefault(); send(); }
    else if (e.key === 's') { e.preventDefault(); if (!locate(current)) saveMenu({ preventDefault() {}, stopPropagation() {}, target: $('#save') }); }
    else if (e.key === 'n') { e.preventDefault(); selectRequest(newRequest()); $('#url').focus(); }
    else if (e.key === 'f') { e.preventDefault(); $('#search').focus(); $('#search').select(); }
  });
  $('#send').title = `Send (${MOD}↩)`; $('#save').title = `Save to a collection (${MOD}S)`;
}

async function main() {
  bind();
  data = await invoke('load_data');
  renderTopbar(); renderSidebar(); renderRequest(); renderResponse();
  $('#url').focus();
}
main();
