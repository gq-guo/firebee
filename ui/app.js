// Firebee 前端：全部 UI 状态在此，后端（Rust）只负责存储 / HTTP / 变量替换 / 导出。
// 数据结构与 core/models.rs 的 serde 形态一致（Auth 外部标签枚举、方法名 "Get" 等）。
const invoke = window.__TAURI__.core.invoke;
const dialog = window.__TAURI__.dialog;
const $ = (s) => document.querySelector(s);
const MAC = navigator.platform.startsWith('Mac');
const MOD = MAC ? '⌘' : 'Ctrl+';

const METHODS = ['Get', 'Post', 'Put', 'Delete', 'Patch', 'Head', 'Options'];
const HISTORY_LIMIT = 500;
const DYNAMIC_VARS = [['$uuid', 'random UUID v4'], ['$timestamp', 'unix seconds'], ['$isoTimestamp', 'ISO 8601 UTC'], ['$randomInt', '0–1000']];
const TRUNCATE_AT = 300_000; // 超过就先显示前 300KB，点 Show all 再全量
const REASON = { 200: 'OK', 201: 'Created', 204: 'No Content', 301: 'Moved Permanently', 302: 'Found', 304: 'Not Modified',
  400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden', 404: 'Not Found', 405: 'Method Not Allowed', 408: 'Timeout',
  409: 'Conflict', 422: 'Unprocessable', 429: 'Too Many Requests', 500: 'Server Error', 502: 'Bad Gateway', 503: 'Unavailable', 504: 'Gateway Timeout' };

// ---------- 状态 ----------
let data = { collections: [], environments: [], history: [] };
let activeEnvId = null;
let current = newRequest();
// 响应按请求 id 保留在内存里（切换请求不丢），最多 50 条；进行中的请求按 id 记 job_id，互不阻塞
const responses = new Map(); // request.id → { ok: dto } | { error: string } | { cancelled: true }
const pendings = new Map();  // request.id → job_id
const resp = () => responses.get(current.id);
const isPending = () => pendings.has(current.id);
function setResponse(rid, r) {
  responses.delete(rid);
  if (responses.size >= 50) responses.delete(responses.keys().next().value); // ponytail: 简单 FIFO，够用
  responses.set(rid, r);
}
let jobSeq = 0;
let missing = [];
let reqTab = 'params', respTab = 'body', sideTab = 'collections';
let renaming = null;   // 正在重命名的对象（collection / folder / request）
let envSel = null;     // env 对话框中选中的环境
let saveTimer = null;
let filter = '';       // 侧栏搜索关键字（匹配集合/文件夹/请求名、URL）
let jsonPath = '';     // 响应 JSONPath 过滤，跨请求保留
let treeView = false;  // 响应 JSON 以可折叠树显示
let findText = '';     // 响应体内查找
const showAll = new Set();     // 已点过 Show all 的 request.id
const selected = new Set();    // 侧栏多选（⌘点击）的请求
let dragging = null;           // { arr, r } 正在拖动的请求
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
  items = items.filter(Boolean);
  menu.replaceChildren(...items.map(([label, fn, opt = {}]) =>
    h('button', { class: 'menu-item' + (opt.danger ? ' danger-item' : ''), role: 'menuitem',
      onclick: (ev) => { ev.stopPropagation(); menu.classList.add('hidden'); fn(ev); } },
      label, opt.kbd && h('span', { class: 'kbd' }, opt.kbd))));
  // 鼠标事件在光标处打开；按钮/键盘触发时贴按钮下方并右对齐
  const rect = e.currentTarget?.getBoundingClientRect?.() || e.target.getBoundingClientRect();
  const x = e.clientX || Math.max(8, rect.right - 170), y = e.clientY || rect.bottom + 4;
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
function kvTable(rows, keyHint, valHint, rerender, { keyList = null, valList = null } = {}) {
  const table = h('table', { class: 'kv' });
  const listFor = (key) => (valList && /^(content-type|accept)$/i.test(key.trim()) ? valList : null);
  rows.forEach((r, i) => {
    const val = h('input', { placeholder: valHint, value: r.value, 'aria-label': valHint, spellcheck: 'false', list: listFor(r.key),
      oninput: (e) => { r.value = e.target.value; dirty(); } });
    const tr = h('tr', { class: r.enabled ? '' : 'off' },
      h('td', { class: 'ctl' }, h('input', { type: 'checkbox', checked: r.enabled, 'aria-label': 'Enabled',
        onchange: (e) => { r.enabled = e.target.checked; tr.classList.toggle('off', !r.enabled); dirty(); renderTabCounts(); } })),
      h('td', { class: 'key' }, h('input', { placeholder: keyHint, value: r.key, 'aria-label': keyHint, spellcheck: 'false', list: keyList,
        oninput: (e) => { r.key = e.target.value; const l = listFor(r.key); l ? val.setAttribute('list', l) : val.removeAttribute('list'); dirty(); renderTabCounts(); } })),
      h('td', { class: 'val' }, val),
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
        onclick: () => openFromHistory(hist), onkeydown: activate },
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
  body.append(h('div', { class: 'row' }, btn('+ New collection', addCol, 'small'), btn('Import…', importFile, 'small ghost')));
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
  arr.splice(idx, 1); dirty(); ensureCurrent(); renderSidebar(); renderRequestHeader();
  toast(`Deleted ${what} “${obj.name}”.`, { action: ['Undo', () => { arr.splice(idx, 0, obj); dirty(); renderSidebar(); renderRequestHeader(); }] });
};

/** 集合或文件夹节点（结构相同：name / folders / requests） */
function containerNode(c, parentArr, isFolder = false) {
  const menu = (e) => openMenu(e, [
    ['New request', () => { const r = newRequest(); c.requests.push(r); collapsed.delete(c.id); dirty(); selectRequest(r); }],
    ['New folder', () => { c.folders.push(newContainer('New folder')); collapsed.delete(c.id); dirty(); renderSidebar(); }],
    ['Import from curl…', () => openImport(c)],
    !isFolder && ['Export collection…', () => exportCollection(c)],
    ['Rename', startRename(c)],
    ['Delete', remove(parentArr, c, isFolder ? 'folder' : 'collection'), { danger: true }],
  ]);
  const view = filterView(c);
  // 拖到容器标题上 → 追加到该容器末尾
  const dropOnContainer = (e) => { e.preventDefault(); e.currentTarget.classList.remove('dropping'); moveDragged(c.requests, c.requests.length); collapsed.delete(c.id); };
  return h('details', { open: q() ? true : !collapsed.has(c.id), ontoggle: (e) => { if (!q()) e.target.open ? collapsed.delete(c.id) : collapsed.add(c.id); } },
    h('summary', { oncontextmenu: menu, onclick: (e) => { if (renaming === c) e.preventDefault(); },
      ondragover: (e) => { if (dragging) { e.preventDefault(); e.currentTarget.classList.add('dropping'); } },
      ondragleave: (e) => e.currentTarget.classList.remove('dropping'), ondrop: dropOnContainer },
      nameNode(c), btn('⋯', menu, 'small more').withAttr('aria-label', `${isFolder ? 'Folder' : 'Collection'} actions`)),
    ...view.folders.map((f) => containerNode(f, c.folders, true)),
    ...view.requests.map((r) => {
      const many = selected.has(r) && selected.size > 1;
      const rmenu = (e) => openMenu(e, [
        ...exportItems(r),
        ['Duplicate', () => { const d = structuredClone(r); d.id = crypto.randomUUID(); d.name += ' copy'; c.requests.splice(c.requests.indexOf(r) + 1, 0, d); dirty(); selectRequest(d); }],
        ['Rename', startRename(r)],
        many ? [`Delete ${selected.size} selected`, deleteSelected, { danger: true, kbd: '⌫' }] : ['Delete', remove(c.requests, r, 'request'), { danger: true }],
      ]);
      return h('div', { class: 'req' + (r === current ? ' active' : '') + (selected.has(r) ? ' sel' : ''), role: 'button', tabindex: 0,
        'aria-current': r === current || undefined, 'aria-selected': selected.has(r) || undefined, draggable: 'true',
        onclick: (e) => { if (e.metaKey || e.ctrlKey) { selected.has(r) ? selected.delete(r) : selected.add(r); renderSidebar(); } else { selected.clear(); selectRequest(r); } },
        onkeydown: activate, oncontextmenu: rmenu,
        ondragstart: (e) => { dragging = { arr: c.requests, r }; e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', r.name); },
        ondragend: () => { dragging = null; document.querySelectorAll('.dropping').forEach((x) => x.classList.remove('dropping')); },
        ondragover: (e) => { if (dragging && dragging.r !== r) { e.preventDefault(); e.currentTarget.classList.add('dropping'); } },
        ondragleave: (e) => e.currentTarget.classList.remove('dropping'),
        ondrop: (e) => { e.preventDefault(); e.stopPropagation(); moveDragged(c.requests, c.requests.indexOf(r)); } },
        methodTag(r.method), nameNode(r), pendings.has(r.id) && h('span', { class: 'spinner', title: 'Sending…' }),
        btn('⋯', rmenu, 'small more').withAttr('aria-label', 'Request actions'));
    }),
  );
}
Element.prototype.withAttr = function (k, v) { if (v !== undefined) this.setAttribute(k, v); return this; };

/** 把正在拖动的请求放到 targetArr 的 index 之前（同数组内移动会先移除再插入） */
function moveDragged(targetArr, index) {
  if (!dragging) return;
  const { arr, r } = dragging;
  dragging = null;
  const from = arr.indexOf(r);
  if (from < 0) return;
  arr.splice(from, 1);
  if (arr === targetArr && from < index) index -= 1;
  targetArr.splice(index, 0, r);
  dirty(); renderSidebar();
}

/** 当前请求所在的 requests 数组；不在集合里返回 null */
function locateArr(r, nodes = data.collections) {
  for (const n of nodes) {
    if (n.requests.includes(r)) return n.requests;
    const deep = locateArr(r, n.folders);
    if (deep) return deep;
  }
  return null;
}

/** 批量删除多选的请求，一次 Undo 全部恢复 */
function deleteSelected() {
  const removed = [...selected].map((r) => { const arr = locateArr(r); return arr && { arr, idx: arr.indexOf(r), r }; }).filter(Boolean)
    .sort((a, b) => b.idx - a.idx);
  removed.forEach(({ arr, idx }) => arr.splice(idx, 1));
  selected.clear(); dirty(); ensureCurrent(); renderSidebar(); renderRequestHeader();
  toast(`Deleted ${removed.length} requests.`, { action: ['Undo', () => {
    removed.sort((a, b) => a.idx - b.idx).forEach(({ arr, idx, r }) => arr.splice(idx, 0, r));
    dirty(); renderSidebar(); renderRequestHeader();
  }] });
}

/** 选中请求：来自集合时直接引用（编辑即写回集合并保存），来自历史时为副本。 */
function selectRequest(r) {
  current = r; missing = [];
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

/** 所有请求都在集合里、全部自动保存。没有集合时自动建一个。 */
function homeCollection() {
  if (!data.collections.length) { data.collections.push(newContainer('My requests')); dirty(); }
  return data.collections[0];
}
/** 当前请求所在的容器 requests 数组（用于"在旁边新建"），否则首个集合 */
const homeArr = () => locateArr(current) || homeCollection().requests;
function createRequest() {
  const r = newRequest();
  homeArr().push(r); dirty(); selectRequest(r);
  return r;
}
function firstRequest(nodes = data.collections) {
  for (const n of nodes) { if (n.requests[0]) return n.requests[0]; const d = firstRequest(n.folders); if (d) return d; }
  return null;
}
/** 当前请求被删掉后，换到一个仍存在的请求 */
function ensureCurrent() {
  if (locateArr(current)) return;
  const r = firstRequest();
  r ? selectRequest(r) : createRequest();
}
/** 历史：原请求还在就直接打开它，否则按快照在首个集合里新建一个 */
function openFromHistory(hist) {
  const found = findById(hist.request.id);
  if (found) return selectRequest(found);
  const r = structuredClone(hist.request);
  homeCollection().requests.push(r); dirty(); selectRequest(r);
}
function findById(id, nodes = data.collections) {
  for (const n of nodes) { const r = n.requests.find((x) => x.id === id); if (r) return r; const d = findById(id, n.folders); if (d) return d; }
  return null;
}

/** 导出 curl / Python 到对话框 */
async function exportCode(req, kind) {
  $('#export-text').textContent = await invoke('export_code', { request: req, env: activeEnv(), kind });
  $('#export-dialog').showModal();
}
const exportItems = (req) => [['Export as curl', () => exportCode(req, 'curl')], ['Export as Python', () => exportCode(req, 'python')]];

/** curl 导入：into 给定时作为新请求存入该集合/文件夹；否则覆盖到当前请求上（保留 id 与自定义名字）。失败把原因交给 onError */
async function importCurl(text, onError, into = null) {
  try {
    const r = await invoke('import_curl', { text });
    if (into) { into.requests.push(r); collapsed.delete(into.id); dirty(); selectRequest(r); return true; }
    const keepName = !/^(Untitled request|New Request)/i.test(current.name);
    Object.assign(current, r, { id: current.id, name: keepName ? current.name : r.name });
    dirty(); selectRequest(current);
    return true;
  } catch (e) { onError(String(e)); return false; }
}
let importInto = null;
function openImport(into = null) {
  importInto = into;
  $('#import-error').textContent = '';
  $('#import-dialog').showModal();
  $('#import-text').focus();
}

/** 集合导出为 Firebee JSON 文件 */
async function exportCollection(c) {
  const path = await dialog.save({ defaultPath: `${c.name}.firebee.json`, filters: [{ name: 'JSON', extensions: ['json'] }] });
  if (!path) return;
  try { await invoke('export_collection', { collection: c, path }); toast(`Exported “${c.name}” to ${path}`); }
  catch (e) { toast(String(e), { error: true }); }
}
/** 导入 Firebee 导出 / Postman collection / Postman environment */
async function importFile() {
  const path = await dialog.open({ multiple: false, filters: [{ name: 'JSON', extensions: ['json'] }] });
  if (!path) return;
  try {
    const r = await invoke('import_file', { path });
    if (r.collection) { data.collections.push(r.collection); dirty(); renderSidebar(); toast(`Imported collection “${r.collection.name}”.`); }
    if (r.environment) { data.environments.push(r.environment); dirty(); renderTopbar(); toast(`Imported environment “${r.environment.name}” — pick it in the Environment menu.`); }
  } catch (e) { toast(String(e), { error: true }); }
}

/** 可拖动分栏：写 CSS 变量，宽度记在 localStorage（仅本机偏好）；双击恢复默认 */
function splitter(el, cssVar, measure, min, max, key) {
  const root = document.documentElement.style;
  try { const saved = localStorage.getItem(key); if (saved) root.setProperty(cssVar, saved); } catch { /* private mode */ }
  el.onpointerdown = (e) => {
    e.preventDefault(); el.setPointerCapture(e.pointerId); el.classList.add('drag'); document.body.classList.add('dragging');
    el.onpointermove = (ev) => root.setProperty(cssVar, `${Math.round(Math.min(max(), Math.max(min, measure(ev))))}px`);
    el.onpointerup = el.onpointercancel = () => {
      el.onpointermove = el.onpointerup = el.onpointercancel = null;
      el.classList.remove('drag'); document.body.classList.remove('dragging');
      try { localStorage.setItem(key, root.getPropertyValue(cssVar)); } catch { /* ignore */ }
    };
  };
  el.ondblclick = () => { root.removeProperty(cssVar); try { localStorage.removeItem(key); } catch { /* ignore */ } };
}

// ---------- 请求面板 ----------
function renderRequestHeader() {
  $('#req-name').value = current.name;
  const where = locate(current);
  $('#req-where').textContent = where ? `in ${where.join(' / ')}` : '';
  $('#method').value = current.method;
  $('#url').value = current.url;
  $('#send').classList.toggle('hidden', isPending());
  $('#cancel').classList.toggle('hidden', !isPending());
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
  else if (reqTab === 'headers') body.append(kvTable(current.headers, 'Header', 'Value', renderRequest, { keyList: 'hdr-names', valList: 'ct-values' }));
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
  if (isPending()) return;
  if (!current.url.trim()) { $('#url').focus(); return; }
  const m = await invoke('missing_vars', { request: current, env: activeEnv() });
  // 有未定义变量：第一次点击只提示，第二次（列表未变）强制发送
  if (m.length && JSON.stringify(m) !== JSON.stringify(missing)) { missing = m; renderRequestHeader(); return; }
  missing = [];
  const id = ++jobSeq, rid = current.id, req = structuredClone(current);
  pendings.set(rid, id); responses.delete(rid);
  renderRequestHeader(); renderResponse(); renderSidebar();
  let status = null, duration_ms = null, result;
  try {
    const r = await invoke('send_request', { job_id: id, request: req, env: activeEnv(), timeout_secs: Number($('#timeout').value) || 30 });
    result = { ok: r }; status = r.status; duration_ms = r.duration_ms;
  } catch (e) {
    result = /cancelled/i.test(String(e)) ? { cancelled: true } : { error: String(e) };
  }
  pendings.delete(rid); setResponse(rid, result);
  data.history.push({ timestamp: new Date().toISOString(), request: req, status, duration_ms });
  if (data.history.length > HISTORY_LIMIT) data.history.splice(0, data.history.length - HISTORY_LIMIT);
  invoke('save_history', { history: data.history }).catch((e) => toast(`Couldn't save history. ${e}`, { error: true }));
  if (current.id === rid) { renderRequestHeader(); renderResponse(); }
  renderSidebar();
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

const contentType = (r) => (r.headers.find(([k]) => k.toLowerCase() === 'content-type')?.[1] || '').toLowerCase();

function renderResponse() {
  const meta = $('#resp-meta'), body = $('#resp-body'), tabs = $('#resp-tabs');
  const response = resp(), pending = isPending();
  meta.replaceChildren(); body.replaceChildren();
  tabs.classList.toggle('hidden', !response?.ok);
  if (!response) {
    if (pending) meta.append(h('span', { class: 'spinner' }), h('span', { class: 'muted' }, 'Sending…'));
    else body.append(emptyState('Response will show here', `Fill in a URL and press Send, or ${MOD}↩ from anywhere in the editor.`));
    return;
  }
  if (response.cancelled) { meta.append(h('span', { class: 'muted' }, 'Cancelled — nothing was received.')); return; }
  if (response.error) { meta.append(h('span', { class: 'error' }, h('span', {}, '⚠'), h('span', {}, response.error))); return; }
  const r = response.ok, ct = contentType(r);
  const isImage = ct.startsWith('image/'), isHtml = ct.includes('text/html');
  const copy = btn('Copy body', () => copyText(r.body, copy));
  meta.append(
    h('span', { class: `status s${Math.floor(r.status / 100)}` }, `${r.status} ${REASON[r.status] || ''}`.trim()),
    h('span', { class: 'meta' }, `${r.duration_ms} ms`), h('span', { class: 'meta' }, fmtSize(r.size_bytes)),
    h('span', { class: 'spacer' }), r.body_base64 ? null : copy, btn('Save…', () => saveBody(r, ct)),
  );
  const previewTab = document.querySelector('[data-resp=preview]');
  previewTab.classList.toggle('hidden', !isHtml);
  if (respTab === 'preview' && !isHtml) respTab = 'body';
  setTab('resp', respTab);
  if (respTab === 'headers') {
    body.append(h('table', { class: 'hdrs' }, ...r.headers.map(([k, v]) => h('tr', {}, h('td', {}, k), h('td', {}, v)))));
    return;
  }
  if (respTab === 'preview') {
    // 沙箱 iframe：无脚本、无同源、无表单提交
    body.append(h('iframe', { sandbox: '', srcdoc: r.body, title: 'HTML preview' }));
    return;
  }
  if (r.body_base64) {
    if (isImage) body.append(h('img', { src: `data:${ct};base64,${r.body_base64}`, alt: 'Response image' }));
    else body.append(emptyState('Binary body', `${fmtSize(r.size_bytes)} that isn't text. Use Save… to write it to a file.`));
    return;
  }
  if (!r.body) { body.append(h('span', { class: 'muted' }, 'Empty body.')); return; }
  let parsed, isJson = false;
  try { parsed = JSON.parse(r.body); isJson = true; } catch { /* not JSON */ }

  // 工具行：JSONPath（仅 JSON）· Tree 切换（仅 JSON）· 查找 · 计数
  const count = h('span', { class: 'meta' });
  const findCount = h('span', { class: 'meta' });
  const out = h('div', { class: 'out' });
  let marks = [], cur = -1;
  const paint = () => {
    out.replaceChildren();
    let value = parsed, text = r.body, err = null;
    if (isJson && jsonPath.trim()) {
      try { const res = jsonPath_(parsed, jsonPath); value = res.length === 1 ? res[0] : res; count.className = 'meta'; count.textContent = res.length === 1 ? '1 match' : `${res.length} matches`; }
      catch (e) { err = e.message; count.className = 'meta error'; count.textContent = err; }
    } else count.textContent = '';
    if (isJson) text = JSON.stringify(value, null, 2);
    if (isJson && treeView && !err) { out.append(jsonTree(value, null, 0, text.length > 50_000)); }
    else {
      const full = showAll.has(current.id) || text.length <= TRUNCATE_AT;
      const shown = full ? text : text.slice(0, TRUNCATE_AT);
      const pre = h('pre', {});
      if (isJson && shown.length <= 1_000_000) pre.innerHTML = highlightJson(shown); else pre.textContent = shown;
      out.append(pre);
      if (!full) out.append(h('div', { class: 'truncated' }, h('span', { class: 'muted' }, `Showing the first ${fmtSize(TRUNCATE_AT)} of ${fmtSize(text.length)}. `),
        btn(`Show all`, () => { showAll.add(current.id); paint(); })));
    }
    applyFind();
  };
  const applyFind = () => {
    marks.forEach((m) => m.replaceWith(...m.childNodes));
    out.normalize();
    marks = []; cur = -1;
    const q = findText.trim().toLowerCase();
    if (!q) { findCount.textContent = ''; return; }
    marks = markMatches(out, q);
    findCount.textContent = marks.length ? `${marks.length} found` : 'No matches';
    if (marks.length) gotoMark(1);
  };
  const gotoMark = (dir) => {
    if (!marks.length) return;
    marks[cur]?.classList.remove('cur');
    cur = (cur + dir + marks.length) % marks.length;
    marks[cur].classList.add('cur');
    marks[cur].scrollIntoView({ block: 'center' });
    findCount.textContent = `${cur + 1} / ${marks.length}`;
  };
  let t1, t2;
  const treeBtn = btn('Tree', () => { treeView = !treeView; treeBtn.classList.toggle('on', treeView); paint(); }, 'small' + (treeView ? ' on' : ''));
  treeBtn.title = 'Toggle collapsible tree view';
  body.append(h('div', { class: 'jp' },
    isJson && h('input', { value: jsonPath, placeholder: '$.data.orders.*.app.key', 'aria-label': 'JSONPath filter', spellcheck: 'false',
      oninput: (e) => { jsonPath = e.target.value; clearTimeout(t1); t1 = setTimeout(paint, 150); },
      onkeydown: (e) => { if (e.key === 'Escape' && jsonPath) { e.stopPropagation(); jsonPath = e.target.value = ''; paint(); } } }),
    isJson && count, isJson && treeBtn,
    h('input', { id: 'find', class: 'find', value: findText, placeholder: `Find in body (${MOD}F)`, 'aria-label': 'Find in response body', spellcheck: 'false',
      oninput: (e) => { findText = e.target.value; clearTimeout(t2); t2 = setTimeout(applyFind, 150); },
      onkeydown: (e) => { if (e.key === 'Enter') { e.preventDefault(); gotoMark(e.shiftKey ? -1 : 1); } else if (e.key === 'Escape' && findText) { e.stopPropagation(); findText = e.target.value = ''; applyFind(); } } }),
    findCount), out);
  paint();
}

/** 在 root 的文本节点里给 q（已小写）的每次出现包上 <mark>，返回 mark 列表 */
function markMatches(root, q) {
  const marks = [], walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes = [];
  for (let n; (n = walker.nextNode());) if (n.nodeValue.toLowerCase().includes(q)) nodes.push(n);
  for (const n of nodes) {
    const text = n.nodeValue, lower = text.toLowerCase(), frag = document.createDocumentFragment();
    let i = 0, j;
    while ((j = lower.indexOf(q, i)) >= 0) {
      if (j > i) frag.append(text.slice(i, j));
      const m = h('mark', {}, text.slice(j, j + q.length));
      marks.push(m); frag.append(m); i = j + q.length;
      if (marks.length > 2000) break; // ponytail: 上限，避免超大响应卡死
    }
    if (i < text.length) frag.append(text.slice(i));
    n.replaceWith(frag);
  }
  return marks;
}

/** JSON → 可折叠树（原生 <details>）。big 时深度 ≥ 2 默认折叠。 */
function jsonTree(v, key, depth, big) {
  const label = key === null ? null : h('span', { class: Array.isArray(key) ? 'num' : 'key' }, Array.isArray(key) ? `${key[0]}` : `"${key}"`);
  const isObj = v !== null && typeof v === 'object';
  if (!isObj) {
    const cls = v === null ? 'null' : typeof v === 'string' ? 'str' : typeof v === 'boolean' ? 'bool' : 'num';
    return h('div', { class: 'tl' }, label, label && ': ', h('span', { class: cls }, JSON.stringify(v)), ',');
  }
  const entries = Array.isArray(v) ? v.map((x, i) => [[i], x]) : Object.entries(v);
  const open = Array.isArray(v) ? '[' : '{', close = Array.isArray(v) ? ']' : '}';
  const d = h('details', { class: depth === 0 ? 'tree' : null, open: !(big && depth >= 2) },
    h('summary', {}, label, label && ': ', open, h('span', { class: 'n' }, `${entries.length} ${Array.isArray(v) ? 'items' : 'keys'}`)),
    ...entries.map(([k, x]) => jsonTree(x, k, depth + 1, big)),
    h('div', { class: 'tl' }, close, depth ? ',' : ''));
  return d;
}

/** 响应体保存到文件；按 Content-Type 猜扩展名 */
async function saveBody(r, ct) {
  const ext = ct.includes('json') ? 'json' : ct.includes('html') ? 'html' : ct.includes('xml') ? 'xml' : ct.includes('csv') ? 'csv'
    : ct.startsWith('image/') ? ct.split('/')[1].split(';')[0].replace('jpeg', 'jpg').replace('svg+xml', 'svg') : r.body_base64 ? 'bin' : 'txt';
  const path = await dialog.save({ defaultPath: `${(current.name || 'response').replace(/[\\/:*?"<>|]+/g, '_')}.${ext}` });
  if (!path) return;
  try { await invoke('save_file', { path, text: r.body_base64 ? null : r.body, base64_data: r.body_base64 || null }); toast(`Saved to ${path}`); }
  catch (e) { toast(String(e), { error: true }); }
}

/** JSONPath 子集：$ · .key · .N · [N] · [-N] · ['key'] · * · [*] · ..key（递归）。返回匹配数组。 */
function jsonPath_(root, path) {
  let p = path.trim();
  if (!p.startsWith('$')) throw new Error('Path must start with $');
  p = p.slice(1);
  const re = /\.\.([\w$-]+|\*)|\.([\w$-]+|\*)|\[(\*|-?\d+|'([^']*)'|"([^"]*)")\]/y;
  const tokens = [];
  for (let i = 0; i < p.length;) {
    re.lastIndex = i;
    const m = re.exec(p);
    if (!m) throw new Error(`Can't read “${p.slice(i, i + 10)}” at position ${i + 1}`);
    i = re.lastIndex;
    if (m[1] !== undefined) tokens.push({ deep: m[1] });
    else tokens.push({ key: m[2] ?? m[4] ?? m[5] ?? m[3] });
  }
  const isObj = (v) => v !== null && typeof v === 'object';
  const children = (v) => Array.isArray(v) ? v : isObj(v) ? Object.values(v) : [];
  const get = (v, k) => {
    if (k === '*') return children(v);
    if (Array.isArray(v)) { const n = Number(k); if (!Number.isInteger(n)) return []; const i = n < 0 ? v.length + n : n; return i in v ? [v[i]] : []; }
    return isObj(v) && k in v ? [v[k]] : [];
  };
  const descend = (v, acc = []) => { acc.push(v); children(v).forEach((c) => descend(c, acc)); return acc; };
  let cur = [root];
  for (const tk of tokens) cur = cur.flatMap((v) => tk.deep !== undefined ? descend(v).flatMap((d) => get(d, tk.deep)) : get(v, tk.key));
  return cur;
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

// ---------- {{变量}} 自动补全 ----------
const ac = { el: null, field: null, items: [], cur: 0, start: 0 };
const acEligible = (el) => (el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && (el.type === 'text' || !el.getAttribute('type'))))
  && el.closest('#request, #env-editor') && !el.matches('#req-name, .find, .jp input');
function acUpdate(el) {
  const before = el.value.slice(0, el.selectionStart);
  const m = /\{\{([\w$-]*)$/.exec(before);
  if (!m) return acHide();
  const prefix = m[1].toLowerCase();
  const env = activeEnv();
  const names = [...(env ? env.variables.filter((v) => v.enabled && v.key).map((v) => [v.key, v.value]) : []), ...DYNAMIC_VARS]
    .filter(([k]) => k.toLowerCase().startsWith(prefix));
  if (!names.length) return acHide();
  Object.assign(ac, { field: el, items: names, cur: 0, start: el.selectionStart - m[1].length });
  const rect = el.getBoundingClientRect();
  ac.el.style.left = `${Math.min(rect.left, window.innerWidth - 260)}px`;
  ac.el.style.top = `${rect.bottom + 4}px`;
  ac.el.replaceChildren(...names.map(([k, v], i) => h('div', { class: 'item' + (i === 0 ? ' cur' : ''), role: 'option',
    onmousedown: (e) => { e.preventDefault(); acAccept(i); } }, `{{${k}}}`, h('span', { class: 'muted' }, v))));
  ac.el.classList.remove('hidden');
}
function acAccept(i = ac.cur) {
  const el = ac.field, name = ac.items[i][0];
  const after = el.value.slice(el.selectionStart);
  const tail = after.startsWith('}}') ? after.slice(2) : after;
  el.value = `${el.value.slice(0, ac.start)}${name}}}${tail}`;
  const pos = ac.start + name.length + 2;
  el.setSelectionRange(pos, pos);
  acHide();
  el.dispatchEvent(new Event('input', { bubbles: true }));
}
function acHide() { ac.el.classList.add('hidden'); ac.field = null; }
function acBind() {
  ac.el = $('#ac');
  document.addEventListener('input', (e) => { if (acEligible(e.target)) acUpdate(e.target); }, true);
  document.addEventListener('keydown', (e) => {
    if (!ac.field) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      ac.cur = (ac.cur + (e.key === 'ArrowDown' ? 1 : -1) + ac.items.length) % ac.items.length;
      [...ac.el.children].forEach((c, i) => c.classList.toggle('cur', i === ac.cur));
    } else if (e.key === 'Enter' || e.key === 'Tab') { e.preventDefault(); acAccept(); }
    else if (e.key === 'Escape') { e.stopPropagation(); acHide(); }
  }, true);
  document.addEventListener('focusout', (e) => { if (e.target === ac.field) setTimeout(() => { if (document.activeElement !== ac.field) acHide(); }, 0); });
}

// ---------- 事件绑定 ----------
function bind() {
  acBind();
  $('#clear-cookies').onclick = async () => { await invoke('clear_cookies'); toast('Cookies cleared for this session.'); };
  $('#method').replaceChildren(...METHODS.map((m) => h('option', { value: m }, m.toUpperCase())));
  $('#method').onchange = (e) => { current.method = e.target.value; dirty(); renderSidebar(); };
  $('#url').oninput = (e) => { current.url = e.target.value; dirty(); };
  $('#url').onkeydown = (e) => { if (e.key === 'Enter') send(); };
  $('#url').onpaste = (e) => {
    const t = e.clipboardData?.getData('text') || '';
    if (/^\s*curl(\.exe)?\s/.test(t)) { e.preventDefault(); importCurl(t, (m) => toast(m, { error: true })); }
  };
  $('#import-close').onclick = () => $('#import-dialog').close();
  $('#import-run').onclick = async () => {
    if (await importCurl($('#import-text').value, (m) => { $('#import-error').textContent = m; }, importInto)) { $('#import-text').value = ''; $('#import-dialog').close(); }
  };
  $('#import-text').onkeydown = (e) => { if ((MAC ? e.metaKey : e.ctrlKey) && e.key === 'Enter') $('#import-run').click(); };
  $('#req-name').oninput = (e) => { current.name = e.target.value; dirty(); renderSidebar(); };
  $('#req-name').onkeydown = (e) => { if (e.key === 'Enter') e.target.blur(); };
  $('#send').onclick = send;
  $('#cancel').onclick = () => invoke('cancel_request', { job_id: pendings.get(current.id) });
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
    else if (e.key === 'n') { e.preventDefault(); createRequest(); $('#url').focus(); }
    else if (e.key === 'f') { e.preventDefault(); const f = $('#find') || $('#search'); f.focus(); f.select(); }
  });
  document.addEventListener('keydown', (e) => {
    if ((e.key === 'Backspace' || e.key === 'Delete') && selected.size > 1 && document.activeElement?.closest('#sidebar')) { e.preventDefault(); deleteSelected(); }
    else if (e.key === 'Escape' && selected.size) { selected.clear(); renderSidebar(); }
  });
  splitter($('#split-side'), '--side-w', (ev) => ev.clientX, 180, () => window.innerWidth * 0.5, 'firebee.sideW');
  splitter($('#split-main'), '--req-w', (ev) => ev.clientX - $('#request').getBoundingClientRect().left, 360,
    () => $('main').getBoundingClientRect().width - 326, 'firebee.reqW');
  $('#send').title = `Send (${MOD}↩)`;
}

async function main() {
  bind();
  data = await invoke('load_data');
  current = firstRequest() || (homeCollection().requests.push(current), dirty(), current);
  renderTopbar(); renderSidebar(); renderRequest(); renderResponse();
  $('#url').focus();
}
main();
