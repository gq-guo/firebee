// Firebee 前端：全部 UI 状态在此，后端（Rust）只负责存储 / HTTP / 变量替换 / 导出。
// 数据结构与 core/models.rs 的 serde 形态一致（Auth 外部标签枚举、方法名 "Get" 等）。
const invoke = window.__TAURI__.core.invoke;
const dialog = window.__TAURI__.dialog;
const $ = (s) => document.querySelector(s);
const MAC = navigator.platform.startsWith('Mac');
const MOD = MAC ? '⌘' : 'Ctrl+';

const METHODS = ['Get', 'Post', 'Put', 'Delete', 'Patch', 'Head', 'Options'];
const HISTORY_LIMIT = 500;
let DYNAMIC_VARS = []; // 启动时从后端拉（core/vars.rs 是唯一来源）
const TRUNCATE_AT = 300_000;
const HISTORY_BODY_MAX = 64_000;   // 单条历史最多留 64KB 响应体
const HISTORY_BODIES = 100;
const SECRET_RESP_HEADERS = new Set(['set-cookie', 'set-cookie2']); // 不写进 history.json        // 只有最近 100 条留响应体，避免 history.json 无限长 // 超过就先显示前 300KB，点 Show all 再全量
const REASON = { 200: 'OK', 201: 'Created', 204: 'No Content', 301: 'Moved Permanently', 302: 'Found', 304: 'Not Modified',
  400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden', 404: 'Not Found', 405: 'Method Not Allowed', 408: 'Timeout',
  409: 'Conflict', 422: 'Unprocessable', 429: 'Too Many Requests', 500: 'Server Error', 502: 'Bad Gateway', 503: 'Unavailable', 504: 'Gateway Timeout' };

// ---------- 状态 ----------
let data = { collections: [], environments: [], history: [] };
let activeEnvId = (() => { try { return localStorage.getItem('firebee.env'); } catch { return null; } })();
// activeEnvId 只能通过这里改，否则容易漏掉持久化（曾经漏过对话框里的 Set active）
function setActiveEnv(id) {
  activeEnvId = id || null;
  try { activeEnvId ? localStorage.setItem('firebee.env', activeEnvId) : localStorage.removeItem('firebee.env'); } catch { /* private mode */ }
}
const pref = (k, d) => { try { const v = localStorage.getItem(k); return v === null ? d : v === '1'; } catch { return d; } };
let followRedirects = pref('firebee.follow', true);
let insecure = pref('firebee.insecure', false);
// 网络层设置（代理 / CA / 客户端证书）：一份 JSON 存本机，和 follow / insecure 一起随每次发送传给后端
const NET_DEFAULT = { proxy: 'system', proxy_url: '', no_proxy: '', ca_path: '', cert_path: '' };
let net = (() => { try { return { ...NET_DEFAULT, ...JSON.parse(localStorage.getItem('firebee.net') || '{}') }; } catch { return { ...NET_DEFAULT }; } })();
const saveNet = () => { try { localStorage.setItem('firebee.net', JSON.stringify(net)); } catch { /* private mode */ } };
// 主题：'light' | 'dark' | 'system'。<html data-theme> 永远是解析后的 light / dark
let theme = (() => { try { return localStorage.getItem('firebee.theme') || 'system'; } catch { return 'system'; } })();
const lightMq = matchMedia('(prefers-color-scheme: light)');
const applyTheme = () => {
  document.documentElement.dataset.theme = theme === 'system' ? (lightMq.matches ? 'light' : 'dark') : theme;
  // 原生标题栏跟着走；null = 交还给系统。失败（没权限 / 非 Tauri）不影响页面主题
  window.__TAURI__.window?.getCurrentWindow().setTheme(theme === 'system' ? null : theme).catch(() => {});
};
function setTheme(v) {
  theme = v; applyTheme();
  try { localStorage.setItem('firebee.theme', v); } catch { /* private mode */ }
}
lightMq.addEventListener('change', applyTheme);
applyTheme();
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
let saving = Promise.resolve(); // 已经在写盘的那次保存；更新后重启前要等它写完
let filter = '';       // 侧栏搜索关键字（匹配集合/文件夹/请求名、URL）
let respQuery = '';    // 响应区的查询框，跨请求保留：以 $ 开头是 JSONPath 过滤，否则是正文查找
const showAll = new Set();     // 已点过 Show all 的 request.id
const selected = new Set();    // 侧栏多选（⌘点击）的请求
let dragging = null;           // { arr, r } 正在拖动的请求
const collapsed = new Set(); // 用户折叠过的 collection/folder id（重绘时保持）

/** kind: 'http' | 'graphql'。GraphQL 请求固定 POST，body 存 query，变量在 graphql_variables */
function newRequest(name = 'Untitled request', kind = 'http') {
  const gql = kind === 'graphql';
  return { id: crypto.randomUUID(), name, method: gql ? 'Post' : 'Get', url: '', params: [], headers: [],
           body_type: gql ? 'GraphQL' : 'None', body: '', form: [], auth: 'None', pinned: false, graphql_variables: '', captures: [] };
}
const isGql = (r) => r.body_type === 'GraphQL';
const newContainer = (name) => ({ id: crypto.randomUUID(), name, folders: [], requests: [], headers: [], auth: 'None' });
const kv = () => ({ enabled: true, key: '', value: '' });
const activeEnv = () => data.environments.find((e) => e.id === activeEnvId) || null;

// 防抖保存集合与环境（500ms）。成功无提示；失败必须让用户知道。
function dirty() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    saving = (async () => {
      await invoke('save_collections', { collections: data.collections });
      await invoke('save_environments', { environments: data.environments });
    })();
    saving.catch((e) => toast(`Couldn't save your changes. ${e}`, { error: true, action: ['Retry', dirty] }));
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
const reqTag = (r) => (isGql(r) ? h('span', { class: 'method m-gql' }, 'GQL') : methodTag(r.method));

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
function toast(text, { error = false, action = null } = {}) {
  // 一次只显示一个 toast。前一个还带着没用过的 Undo 时不能直接盖掉——
  // 删了 A 再删 B、或者删完 A 又发了个带 capture 的请求，A 就永久没了
  // （删除是乐观的，500ms 后就落盘）。有待撤销动作时一律排队。
  if (pendingUndo) { toastQueue.push([text, { error, action }]); return; }
  showToast(text, { error, action });
}
const toastQueue = [];
let pendingUndo = null;

function showToast(text, { error = false, action = null } = {}) {
  const el = $('#toast');
  clearTimeout(toastTimer);
  pendingUndo = action || null;
  const done = () => {
    el.classList.add('hidden');
    pendingUndo = null;
    const next = toastQueue.shift();
    if (next) showToast(next[0], next[1]);
  };
  put(el, h('span', {}, text),
    action && btn(action[0], () => { action[1](); done(); }, 'small'),
    btn('×', done, 'small ghost').withAttr('aria-label', 'Dismiss'));
  el.className = 'toast' + (error ? ' error' : '');
  el.onmouseenter = () => clearTimeout(toastTimer);
  el.onmouseleave = () => { toastTimer = setTimeout(done, 3000); };
  toastTimer = setTimeout(done, action ? 8000 : 3000);
}
function hideToast() { clearTimeout(toastTimer); pendingUndo = null; toastQueue.length = 0; $('#toast').classList.add('hidden'); }

/** 右键 / ⋯ 菜单：items = [[label, fn, {danger, kbd}], ...]，点击任意处或 Esc 关闭 */
function openMenu(e, items) {
  e.preventDefault(); e.stopPropagation();
  // 这里 stopPropagation 了，document 上关设置弹窗的监听收不到，手动关
  $('#req-settings').classList.add('hidden'); $('#settings-btn').setAttribute('aria-expanded', 'false');
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
// 正式版不弹 WebView 自带的右键菜单（Reload / Inspect Element），开发版保留方便调试。
// 自定义菜单已经 preventDefault；输入框和选中的文字保留原生菜单，复制粘贴还要用
let debugBuild = false; // 查询结果回来前按正式版处理
invoke('debug_build').then((v) => { debugBuild = v; }, () => {});
document.addEventListener('contextmenu', (e) => {
  if (debugBuild || e.defaultPrevented || e.target.closest('input, textarea, [contenteditable]') || String(getSelection())) return;
  e.preventDefault();
});
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
function kvTable(rows, keyHint, valHint, rerender, { keyList = null, valList = null, files = false, secrets = false } = {}) {
  const table = h('table', { class: 'kv' });
  const listFor = (key) => (valList && /^(content-type|accept)$/i.test(key.trim()) ? valList : null);
  // 最后一行永远是空白"幽灵行"：一敲字就变成真实行并追加新的幽灵行，不用点 Add
  const all = [...rows, null];
  all.forEach((r, i) => {
    const ghost = r === null;
    const promote = (field, value) => {
      const nr = kv(); nr[field] = value; rows.push(nr); dirty(); rerender(); renderTabCounts();
      const table = document.querySelector('dialog[open] table.kv') || $('#req-body table.kv');
      // 必须在刚重建的这张表里找：#req-body 的表在 DOM 里排在对话框前面，
      // 全文档选择器会选中它，而模态框打开时它是 inert 的，focus() 静默失效
      const cell = table.closest('body') ? table.querySelector(`tr:nth-last-child(2) td.${field === 'key' ? 'key' : 'val'} input`) : null;
      if (cell) { cell.focus(); cell.setSelectionRange(cell.value.length, cell.value.length); }
    };
    const val = h('input', { placeholder: valHint, value: ghost ? '' : r.value, 'aria-label': valHint, spellcheck: 'false', list: ghost ? null : listFor(r.key),
      type: !ghost && r.secret ? 'password' : 'text', autocomplete: 'off',
      oninput: (e) => { if (ghost) return promote('value', e.target.value); r.value = e.target.value; if (!files) r.is_file = false; dirty(); } });
    // 文件行：值是本机路径，用系统文件框选；Text/File 按钮切换
    const fileCell = () => h('span', { class: 'row' },
      btn(r.value ? r.value.split('/').pop() : 'Choose file…', async () => {
        const path = await dialog.open({ multiple: false });
        if (path) { r.value = path; dirty(); rerender(); }
      }, 'small').withAttr('title', r.value || ''),
      r.value ? h('span', { class: 'muted' }, r.value) : null);
    // Secret：值进钥匙串不进 JSON，输入框遮罩
    const secretBtn = !secrets || ghost ? null : btn(r.secret ? '🔒' : '🔓', () => { r.secret = !r.secret; dirty(); rerender(); }, 'small ghost')
      .withAttr('title', r.secret ? 'Secret — stored in the keychain, masked, never exported. Click to make plain.' : 'Plain — click to make secret')
      .withAttr('aria-label', 'Toggle secret');
    const typeBtn = !files || ghost ? null : btn(r.is_file ? 'File' : 'Text', () => {
      r.is_file = !r.is_file; r.value = ''; dirty(); rerender();
    }, 'small ghost').withAttr('aria-label', 'Field type');
    const tr = h('tr', { class: ghost ? 'ghost' : (r.enabled ? '' : 'off') },
      h('td', { class: 'ctl' }, h('input', { type: 'checkbox', checked: ghost ? false : r.enabled, 'aria-label': 'Enabled', tabindex: ghost ? -1 : null,
        onchange: (e) => { if (ghost) return; r.enabled = e.target.checked; tr.classList.toggle('off', !r.enabled); dirty(); renderTabCounts(); } })),
      h('td', { class: 'key' }, h('input', { placeholder: keyHint, value: ghost ? '' : r.key, 'aria-label': keyHint, spellcheck: 'false', list: keyList,
        oninput: (e) => { if (ghost) return promote('key', e.target.value); r.key = e.target.value; const l = listFor(r.key); l ? val.setAttribute('list', l) : val.removeAttribute('list'); dirty(); renderTabCounts(); } })),
      files ? h('td', { class: 'ctl type' }, typeBtn) : null,
      secrets ? h('td', { class: 'ctl type' }, secretBtn) : null,
      h('td', { class: 'val' }, !ghost && r.is_file && files ? fileCell() : val),
      h('td', { class: 'ctl' }, ghost ? null : btn('×', () => { rows.splice(i, 1); dirty(); rerender(); renderTabCounts(); }, 'small ghost').withAttr('aria-label', 'Remove row')),
    );
    table.append(tr);
  });
  return table;
}

// ---------- 变量解析（与 core/vars.rs 同规则：{{name}}，环境变量优先，其次 $动态变量）----------
const VAR_RE = /\{\{\s*([^{}]*?)\s*\}\}/g;
function varMap() {
  const env = activeEnv();
  return new Set(env ? env.variables.filter((v) => v.enabled && v.key).map((v) => v.key) : []);
}
/** `$randomInt(1,100)` → `$randomInt`，带参数的动态变量按基名判断 */
const baseVarName = (name) => name.replace(/\(.*\)$/, '').trimEnd();
const isResolved = (name, vars) => vars.has(name) || DYNAMIC_VARS.some(([k]) => k === baseVarName(name));
/** 当前请求里所有未解析的变量名（去重、按出现顺序） */
function unresolvedVars(req = current) {
  const vars = varMap(), out = [];
  const scan = (str) => { for (const m of (str || '').matchAll(VAR_RE)) { const n = m[1]; if (!isResolved(n, vars) && !out.includes(n)) out.push(n); } };
  scan(req.url); scan(req.body); scan(req.graphql_variables);
  for (const kvr of [...req.params, ...req.headers, ...req.form]) { scan(kvr.key); scan(kvr.value); }
  const off = req.auth === 'Off';
  for (const kvr of inheritedHeaders(req)) {
    if (off && kvr.key.trim().toLowerCase() === 'authorization') continue;
    scan(kvr.key); scan(kvr.value);
  }
  const a = off ? null : req.auth !== 'None' ? req.auth : inheritedAuth(req);
  if (a && typeof a !== 'string') Object.values(Object.values(a)[0]).forEach((v) => typeof v === 'string' && scan(v));
  return out;
}
/** URL 镜像：把 {{var}} 按可解析与否着色，其余原样 */
function renderUrlMirror() {
  const el = $('#url-mirror'), vars = varMap(), url = $('#url').value;
  el.replaceChildren();
  let last = 0;
  for (const m of url.matchAll(VAR_RE)) {
    if (m.index > last) el.append(url.slice(last, m.index));
    el.append(h('span', { class: isResolved(m[1], vars) ? 'v-ok' : 'v-bad' }, m[0]));
    last = m.index + m[0].length;
  }
  el.append(url.slice(last), '\u200b');
  el.scrollLeft = $('#url').scrollLeft;
  renderResolved();
}
/** URL 下方的"解析后"预览：环境变量换成值，$动态变量和未定义的原样（后者标红） */
function renderResolved() {
  const env = activeEnv(), url = current.url;
  const vals = new Map(env ? env.variables.filter((v) => v.enabled && v.key).map((v) => [v.key, v.secret ? '••••••' : v.value]) : []);
  const out = $('#resolved-url');
  out.replaceChildren();
  // enc：query 里的键值要按 URL 编码显示，复制出来才是能直接用的地址
  const add = (str, enc = (x) => x) => {
    let last = 0;
    for (const m of str.matchAll(VAR_RE)) {
      if (m.index > last) out.append(enc(str.slice(last, m.index)));
      out.append(vals.has(m[1]) ? h('span', { class: 'rv' }, enc(vals.get(m[1])))
        : h('span', { class: isResolved(m[1], vals) ? '' : 'rbad' }, m[0]));
      last = m.index + m[0].length;
    }
    out.append(enc(str.slice(last)));
  };
  // 与 core/http.rs 的 build_url 一致：启用且有键名的 param + query 型 ApiKey，
  // 按 form 编码（空格是 +）追加在已有 query 之后、#fragment 之前
  const sub = (str) => str.replace(VAR_RE, (m, n) => (vals.has(n) ? vals.get(n) : m));
  const pairs = current.params.filter((p) => p.enabled && p.key).map((p) => [sub(p.key), sub(p.value)]);
  const auth = current.auth === 'Off' ? null : current.auth !== 'None' ? current.auth : inheritedAuth(current);
  const ak = auth?.ApiKey;
  if (ak?.in_query && ak.key) pairs.push([sub(ak.key), sub(ak.value)]);
  const extra = new URLSearchParams(pairs).toString();
  const hash = url.indexOf('#'), head = hash < 0 ? url : url.slice(0, hash);
  add(head);
  if (extra) out.append(head.includes('?') ? (/[?&]$/.test(head) ? '' : '&') : '?', extra);
  if (hash >= 0) add(url.slice(hash));
  // 复制用真正解析过的 URL；解析不了（比如 host 里还有未定义变量）就用显示的文本
  let exact = null;
  try {
    const u = new URL(sub(url));
    if (extra) u.search = u.search ? `${u.search.slice(1)}&${extra}` : extra;
    if (!u.search) u.search = ''; // 去掉孤零零的 "?"，同 build_url
    exact = u.href;
  } catch { /* 不是合法 URL */ }
  out.dataset.exact = exact ?? '';
  $('#resolved').classList.toggle('empty', !url.trim());
  $('#resolved').title = env ? `Resolved with ${env.name}` : 'No environment active';
}
/** 未解析变量提示：常驻在 Send 左侧的固定槽位，不撑开布局 */
function renderMissing() {
  const m = $('#missing'), list = unresolvedVars();
  m.classList.toggle('hidden', list.length === 0);
  if (!list.length) { missing = []; return; }
  const armed = JSON.stringify(missing) === JSON.stringify(list);
  m.textContent = armed ? 'Send again to send with placeholders as-is' : `${list.join(', ')} not defined${activeEnv() ? ` in ${activeEnv().name}` : ''}`;
  m.title = armed ? '' : `Not defined in the active environment: ${list.join(', ')}`;
}

// ---------- 顶栏 ----------
function renderTopbar() {
  const sel = $('#env-select');
  // 管理入口放在列表最下方，选中即打开对话框并恢复原选择
  sel.replaceChildren(
    h('option', { value: '' }, 'No environment'),
    ...data.environments.map((e) => h('option', { value: e.id }, e.name)),
    h('option', { disabled: true }, '──────────'),
    h('option', { value: '__manage' }, 'Manage environments…'),
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
    body.append(h('div', { class: 'row' }, h('span', { class: 'muted' }, `${data.history.length} entries`), h('span', { class: 'spacer' }),
      btn('Clear history', () => {
        const old = data.history; data.history = []; saveHistory(); renderSidebar();
        // Undo 时把清空之后新产生的记录接在后面，别把它们一起丢了
        toast(`Cleared ${old.length} history entries.`, { action: ['Undo', () => { data.history = [...old, ...data.history]; saveHistory(); renderSidebar(); }] });
      }, 'small ghost')));
    for (const hist of shown.reverse()) {
      const hmenu = (e) => openMenu(e, [['Delete entry', () => {
        const idx = data.history.indexOf(hist);
        if (idx < 0) return; // splice(-1,1) 会删掉最新那条
        data.history.splice(idx, 1); saveHistory(); renderSidebar();
        toast('Deleted history entry.', { action: ['Undo', () => { data.history.splice(idx, 0, hist); saveHistory(); renderSidebar(); }] });
      }, { danger: true }]]);
      const t = new Date(hist.timestamp);
      const hhmm = `${String(t.getHours()).padStart(2, '0')}:${String(t.getMinutes()).padStart(2, '0')}`;
      body.append(h('div', { class: 'hist', role: 'button', tabindex: 0, title: `${hist.request.method.toUpperCase()} ${hist.request.url}`,
        onclick: () => openFromHistory(hist), onkeydown: activate, oncontextmenu: hmenu },
        h('span', { class: 'muted' }, hhmm), reqTag(hist.request),
        h('span', { class: hist.status ? `s${Math.floor(hist.status / 100)}` : 's0' }, hist.status ?? 'ERR'),
        h('span', {}, hist.request.url || '(no URL)'), btn('⋯', hmenu, 'small more').withAttr('aria-label', 'History entry actions')));
    }
    return;
  }
  const addCol = addCollection;
  if (!data.collections.length) {
    body.append(emptyState('No collections yet', 'A collection keeps the requests you want to reuse, in folders if you like.', ['Create a collection', addCol]));
    return;
  }
  const shown = data.collections.filter((c) => filterView(c).show);
  if (q() && !shown.length) { body.append(emptyState(`Nothing matches “${filter.trim()}”`, 'Names of collections, folders and requests are searched, and request URLs.')); return; }
  const pins = pinnedNode();
  if (pins) body.append(pins);
  shown.forEach((c) => body.append(containerNode(c, data.collections)));
  body.querySelector('input.rename')?.focus();
}

function addCollection() { data.collections.push(newContainer(`Collection ${data.collections.length + 1}`)); sideTab = 'collections'; dirty(); renderSidebar(); }

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
    renaming = null; renderSidebar(); renderRequestHeader(); renderTabs();
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
  if (idx < 0) return; // splice(-1,1) 会删掉最后一个，不是这一个
  const tabsBefore = [...openTabs];
  arr.splice(idx, 1); dirty(); ensureCurrent(); renderSidebar(); renderRequestHeader(); renderTabs();
  toast(`Deleted ${what} “${obj.name}”.`, { action: ['Undo', () => {
    arr.splice(idx, 0, obj); dirty();
    if (tabsBefore.includes(obj.id) && !openTabs.includes(obj.id)) { openTabs.push(obj.id); saveTabs(); }
    renderSidebar(); renderRequestHeader(); renderTabs();
  }] });
};

/** 集合或文件夹节点（结构相同：name / folders / requests） */
function containerNode(c, parentArr, isFolder = false) {
  const menu = (e) => openMenu(e, [
    ...[['http', 'New HTTP request'], ['graphql', 'New GraphQL request']].map(([kind, label]) =>
      [label, () => { const r = newRequest(undefined, kind); c.requests.push(r); collapsed.delete(c.id); dirty(); selectRequest(r); }]),
    ['New folder', () => { c.folders.push(newContainer('New folder')); collapsed.delete(c.id); dirty(); renderSidebar(); }],
    ['Shared headers & auth…', () => openSettings(c, isFolder)],
    ['Import from curl…', () => openImport(c)],
    !isFolder && ['Export collection…', () => exportCollection(c)],
    !isFolder && (c.project_dir
      ? ['Unlink project folder', () => { delete c.project_dir; dirty(); renderSidebar(); toast(`“${c.name}” is no longer synced to a folder. The files there are untouched.`); }]
      : ['Save as project folder…', () => linkProject(c)]),
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
      nameNode(c), c.project_dir ? h('span', { class: 'proj', title: `Synced to ${c.project_dir}` }, '⎇') : null, btn('⋯', menu, 'small more').withAttr('aria-label', `${isFolder ? 'Folder' : 'Collection'} actions`)),
    !q() && !c.folders.length && !c.requests.length && h('div', { class: 'empty-hint' }, 'Empty — right-click to add a request, or paste a curl into the URL field'),
    ...view.folders.map((f) => containerNode(f, c.folders, true)),
    ...view.requests.map((r) => requestRow(r, c.requests)),
  );
}

/** 一行请求。arr 是它所属的 requests 数组（Duplicate / Delete / 拖动都作用在上面）；
 *  drag=false 用于 Pinned 区——那里的顺序跟着各自集合走，拖动没有意义。 */
function requestRow(r, arr, drag = true) {
  const many = selected.has(r) && selected.size > 1;
  const rmenu = (e) => openMenu(e, [
    [r.pinned ? 'Unpin' : 'Pin to top', () => { r.pinned = !r.pinned; dirty(); renderSidebar(); }],
    ...exportItems(r),
    ['Duplicate', () => { const d = structuredClone(r); d.id = crypto.randomUUID(); d.name += ' copy'; d.pinned = false; arr.splice(arr.indexOf(r) + 1, 0, d); dirty(); selectRequest(d); }],
    ['Rename', startRename(r)],
    many ? [`Delete ${selected.size} selected`, deleteSelected, { danger: true, kbd: '⌫' }] : ['Delete', remove(arr, r, 'request'), { danger: true }],
  ]);
  const drops = drag ? {
    draggable: 'true',
    ondragstart: (e) => { dragging = { arr, r }; e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', r.name); },
    ondragend: () => { dragging = null; document.querySelectorAll('.dropping').forEach((x) => x.classList.remove('dropping')); },
    ondragover: (e) => { if (dragging && dragging.r !== r) { e.preventDefault(); e.currentTarget.classList.add('dropping'); } },
    ondragleave: (e) => e.currentTarget.classList.remove('dropping'),
    ondrop: (e) => { e.preventDefault(); e.stopPropagation(); moveDragged(arr, arr.indexOf(r)); },
  } : {};
  return h('div', { class: 'req' + (r === current ? ' active' : '') + (selected.has(r) ? ' sel' : ''), role: 'button', tabindex: 0,
    'aria-current': r === current || undefined, 'aria-selected': selected.has(r) || undefined, ...drops,
    onclick: (e) => { if (e.metaKey || e.ctrlKey) { selected.has(r) ? selected.delete(r) : selected.add(r); renderSidebar(); } else { selected.clear(); selectRequest(r); } },
    onkeydown: activate, oncontextmenu: rmenu },
    reqTag(r), nameNode(r), pendings.has(r.id) && h('span', { class: 'spinner', title: 'Sending…' }),
    btn('⋯', rmenu, 'small more').withAttr('aria-label', 'Request actions'));
}

/** 集合树里的所有请求（深度优先） */
function allRequests(nodes = data.collections, out = []) {
  for (const n of nodes) { out.push(...n.requests); allRequests(n.folders, out); }
  return out;
}

/** 侧栏顶部的 Pinned 区；没有置顶请求时不占位 */
function pinnedNode() {
  const pins = allRequests().filter((r) => r.pinned && (!q() || hit(r.name) || hit(r.url)));
  if (!pins.length) return null;
  return h('details', { class: 'pinned', open: q() ? true : !collapsed.has('pinned'),
      ontoggle: (e) => { if (!q()) e.target.open ? collapsed.delete('pinned') : collapsed.add('pinned'); } },
    h('summary', {}, h('span', { class: 'name' }, 'Pinned'), h('span', { class: 'muted' }, String(pins.length))),
    ...pins.map((r) => requestRow(r, locateArr(r) || [], false)));
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
  // 换了父容器 = 换了继承链：面包屑、继承的头 / auth、未解析变量统计全都要重算
  if (r === current) { renderRequestHeader(); renderRequest(); renderMissing(); }
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
  selected.clear(); dirty(); ensureCurrent(); renderSidebar(); renderRequestHeader(); renderTabs();
  const tabsBefore = [...openTabs];
  toast(`Deleted ${removed.length} requests.`, { action: ['Undo', () => {
    removed.sort((a, b) => a.idx - b.idx).forEach(({ arr, idx, r }) => arr.splice(idx, 0, r));
    tabsBefore.forEach((id) => { if (!openTabs.includes(id) && findById(id)) openTabs.push(id); });
    saveTabs(); dirty(); renderSidebar(); renderRequestHeader(); renderTabs();
  }] });
}

/** 选中请求：来自集合时直接引用（编辑即写回集合并保存），来自历史时为副本。 */
function selectRequest(r) {
  current = r; missing = []; flash(null);
  if (!openTabs.includes(r.id)) openTabs.push(r.id);
  saveTabs();
  renderTabs(); renderRequest(); renderResponse(); renderSidebar();
}

// ---------- 标签页 ----------
// 打开的请求 id 列表；请求对象本身住在集合里，这里只记"开着哪些"。
// 编辑本来就直接写回集合对象，所以切标签不会丢任何东西。
let openTabs = (() => {
  try {
    const v = JSON.parse(localStorage.getItem('firebee.tabs') || '[]');
    return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []; // 别让一个坏值把启动打挂
  } catch { return []; }
})();
const saveTabs = () => { try { localStorage.setItem('firebee.tabs', JSON.stringify(openTabs)); } catch { /* private mode */ } };

/** 关掉一个标签；关的是当前标签时，焦点给右边的、没有就给左边的。
 *  全关完也没关系：标签条自己隐藏，请求面板继续显示当前请求（标签只是快捷入口）。 */
function closeTab(id) {
  const i = openTabs.indexOf(id);
  if (i < 0) return;
  openTabs.splice(i, 1); saveTabs();
  if (id !== current.id) return renderTabs();
  const next = findById(openTabs[i] ?? openTabs[i - 1]);
  next ? selectRequest(next) : renderTabs();
}

/** 只留下 keep 里的标签；当前标签被关掉时，焦点给 focusId（右键的那个），没有就给第一个 */
function closeTabsExcept(keep, focusId) {
  openTabs = openTabs.filter((id) => keep.includes(id));
  saveTabs();
  if (openTabs.includes(current.id)) return renderTabs();
  const stay = findById(focusId && openTabs.includes(focusId) ? focusId : openTabs[0]);
  stay ? selectRequest(stay) : renderTabs();
}

function renderTabs() {
  // 集合里已经没有的请求（被删了）顺手清掉
  const live = openTabs.map((id) => [id, findById(id)]).filter(([, r]) => r);
  if (live.length !== openTabs.length) { openTabs = live.map(([id]) => id); saveTabs(); }
  const bar = $('#tabs');
  put(bar, ...live.map(([id, r]) => {
    const on = id === current.id;
    const i = openTabs.indexOf(id);
    const tmenu = (e) => openMenu(e, [
      ['Close', () => closeTab(id), { kbd: `${MOD}W` }],
      live.length > 1 && ['Close others', () => closeTabsExcept([id], id)],
      i < live.length - 1 && ['Close to the right', () => closeTabsExcept(openTabs.slice(0, i + 1), id)],
      ['Close all', () => closeTabsExcept([])],
    ]);
    return h('div', { class: 'tab' + (on ? ' active' : ''), role: 'tab', tabindex: 0, 'aria-selected': on ? 'true' : 'false',
      title: [...(locate(r) || []), r.name].join(' / '),
      onclick: () => !on && selectRequest(r),
      onkeydown: activate,
      oncontextmenu: tmenu,
      onauxclick: (e) => { if (e.button === 1) { e.preventDefault(); closeTab(id); } } },
      reqTag(r), h('span', { class: 'tab-name' }, r.name),
      pendings.has(id) ? h('span', { class: 'spinner' }) : null,
      btn('×', (e) => { e.stopPropagation(); closeTab(id); }, 'small more close')
        .withAttr('aria-label', `Close ${r.name}`));
  }));
  scrollTabIntoView(bar);
}

/** 标签多到溢出时，当前标签可能在可视区外——只滚标签条本身，别动页面 */
function scrollTabIntoView(bar) {
  const act = bar.querySelector('.tab.active');
  if (!act) return;
  const pad = 12, left = act.offsetLeft - bar.offsetLeft, right = left + act.offsetWidth;
  if (left < bar.scrollLeft + pad) bar.scrollLeft = Math.max(0, left - pad);
  else if (right > bar.scrollLeft + bar.clientWidth - pad) bar.scrollLeft = right - bar.clientWidth + pad;
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

/** 集合 / 文件夹的公共 header 与 auth */
function openSettings(c, isFolder) {
  c.headers ||= []; c.auth ||= 'None';
  const dlg = $('#settings-dialog');
  const draw = () => {
    $('#settings-title').textContent = `${isFolder ? 'Folder' : 'Collection'} · ${c.name}`;
    $('#settings-body').replaceChildren(
      h('section', { class: 'settings-block' },
        h('h3', {}, 'Headers'),
        h('p', { class: 'helper' }, 'Sent with every request inside. A request that sets the same header wins.'),
        kvTable(c.headers, 'Header', 'Value', draw, { keyList: 'hdr-names', valList: 'ct-values' })),
      h('section', { class: 'settings-block' },
        h('h3', {}, 'Auth'),
        h('p', { class: 'helper' }, 'Used by requests that leave their own auth on Inherit.'),
        authEditor(c, draw, true)));
  };
  draw();
  if (!dlg.open) dlg.showModal();
  $('#settings-close').onclick = () => { dlg.close(); renderRequest(); renderSidebar(); };
  dlg.onclose = () => renderRequest();
}

/** 请求所在集合 / 文件夹链上的公共配置，外层在前；找不到返回 [] */
function chainFor(r, nodes = data.collections, path = []) {
  for (const n of nodes) {
    const here = [...path, { headers: n.headers || [], auth: n.auth || 'None' }];
    if (n.requests.includes(r)) return here;
    const deep = chainFor(r, n.folders, here);
    if (deep) return deep;
  }
  return path.length ? null : [];
}
/** 链上生效的 header（同名内层覆盖外层），用于在 Headers 标签里只读展示 */
function inheritedHeaders(r) {
  const out = [];
  for (const link of chainFor(r) || []) {
    const on = link.headers.filter((h) => h.enabled && h.key);
    for (let i = out.length - 1; i >= 0; i--) if (on.some((h) => h.key.toLowerCase() === out[i].key.toLowerCase())) out.splice(i, 1);
    out.push(...on);
  }
  return out.filter((h) => !r.headers.some((own) => own.enabled && own.key.toLowerCase() === h.key.toLowerCase()));
}
/** 链上生效的 auth（最内层非 None）；请求自己设了就返回 null */
function inheritedAuth(r) {
  if (r.auth !== 'None' && r.auth !== 'Off') return null; // 自己设了具体认证
  return [...(chainFor(r) || [])].reverse().find((l) => l.auth !== 'None')?.auth || null;
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
  const r = found || structuredClone(hist.request);
  if (!found) { homeCollection().requests.push(r); dirty(); }
  // 当时的响应还留着就放回响应面板，不用重发（重发 POST 是有副作用的）
  if (hist.response) setResponse(r.id, { ok: { ...hist.response, body_base64: null, from_history: hist.timestamp } });
  selectRequest(r);
}
function findById(id, nodes = data.collections) {
  for (const n of nodes) { const r = n.requests.find((x) => x.id === id); if (r) return r; const d = findById(id, n.folders); if (d) return d; }
  return null;
}

/** 导出 curl / Python 到对话框 */
async function exportCode(req, kind) {
  $('#export-text').textContent = await invoke('export_code', { request: req, env: activeEnv(), kind, inherited: chainFor(req) || [] });
  $('#export-dialog').showModal();
}
const exportItems = (req) => [['Export as curl', () => exportCode(req, 'curl')], ['Export as Python', () => exportCode(req, 'python')]];

/** curl 导入：into 给定时作为新请求存入该集合/文件夹；否则覆盖到当前请求上（保留 id 与自定义名字）。失败把原因交给 onError */
async function importCurl(text, onError, into = null) {
  try {
    const r = await invoke('import_curl', { text });
    if (into) { into.requests.push(r); collapsed.delete(into.id); dirty(); selectRequest(r); return true; }
    const keepName = !/^(Untitled request|New Request)/i.test(current.name);
    // import_curl 返回的是全新的 Request，captures / pinned 是空的——
  // 直接 assign 会把用户配好的提取规则和置顶状态静默抹掉
  Object.assign(current, r, { id: current.id, name: keepName ? current.name : r.name,
    captures: current.captures || [], pinned: current.pinned });
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
/** 项目目录：集合与目录双向同步（见 core/project.rs），目录可以进 git */
async function linkProject(c) {
  const dir = await dialog.open({ directory: true, title: 'Choose an empty folder (or the project folder of this collection)' });
  if (!dir) return;
  try {
    const linked = await invoke('link_project', { collection: c, dir });
    Object.assign(c, linked); dirty(); renderSidebar();
    toast(`“${c.name}” now lives in ${dir}. Commit that folder; teammates open it with Open project folder…`);
  } catch (e) { toast(String(e), { error: true }); }
}
async function openProject() {
  const dir = await dialog.open({ directory: true, title: 'Open a folder containing firebee.json' });
  if (!dir) return;
  try {
    const c = await invoke('open_project', { dir });
    const existing = data.collections.find((x) => x.id === c.id);
    if (existing) { Object.assign(existing, c); dirty(); renderSidebar(); toast(`Reloaded “${c.name}” from ${dir}.`); return; }
    data.collections.push(c); dirty(); renderSidebar();
    toast(`Opened “${c.name}”. Changes you make are written back to ${dir}.`);
  } catch (e) { toast(String(e), { error: true }); }
}

async function exportCollection(c) {
  const path = await dialog.save({ defaultPath: `${c.name}.firebee.json`, filters: [{ name: 'JSON', extensions: ['json'] }] });
  if (!path) return;
  try { await invoke('export_collection', { collection: c, path }); toast(`Exported “${c.name}” to ${path}`); }
  catch (e) { toast(String(e), { error: true }); }
}
/** 导入 Firebee 导出 / Postman collection / Postman environment */
async function importFile() {
  const path = await dialog.open({ multiple: false, filters: [{ name: 'JSON / YAML', extensions: ['json', 'yaml', 'yml'] }] });
  if (!path) return;
  try {
    const r = await invoke('import_file', { path });
    if (r.collection) {
      const n = disarmCaptures(r.collection);
      data.collections.push(r.collection); dirty(); renderSidebar();
      toast(`Imported collection “${r.collection.name}”.`
        + (n ? ` ${n} capture ${n === 1 ? 'rule was' : 'rules were'} turned off — they rewrite environment variables, so review them in a request's Capture tab before enabling.` : ''));
    }
    if (r.environment) { data.environments.push(r.environment); dirty(); renderTopbar(); toast(`Imported environment “${r.environment.name}” — pick it in the Environment menu.`); }
  } catch (e) { toast(String(e), { error: true }); }
}

/** 导入进来的 capture 规则一律先关掉，返回关掉的条数。
 *  capture 会改写当前环境的变量：一个别人给的集合里带一条 `base_url ← $.base_url`，
 *  你点一次 Send 就把自己的 base_url 换成了对方的域名，之后自己的请求会把真 token 发过去。
 *  数据保留（自己导出的集合再导回来不丢东西），但必须先看一眼再开。 */
function disarmCaptures(node) {
  let n = 0;
  for (const r of node.requests || []) for (const c of r.captures || []) if (c.enabled) { c.enabled = false; n++; }
  for (const f of node.folders || []) n += disarmCaptures(f);
  return n;
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
  $('#method').value = current.method;
  $('#method').className = `m-${current.method.toLowerCase()}`;
  $('#method').classList.toggle('hidden', isGql(current)); // GraphQL 固定 POST
  $('#url').value = current.url;
  $('#send').classList.toggle('hidden', isPending());
  $('#cancel').classList.toggle('hidden', !isPending());
  renderUrlMirror(); renderMissing();
  renderTabCounts();
}

function renderTabCounts() {
  const count = (rows) => rows.filter((r) => r.enabled && r.key).length;
  const n = { params: count(current.params), headers: count(current.headers),
    body: current.body_type === 'None' ? 0 : ['Form', 'Multipart'].includes(current.body_type) ? count(current.form) : (current.body.trim() ? 1 : 0),
    auth: current.auth === 'None' ? 0 : 1, capture: count(current.captures || []) };
  document.querySelectorAll('[data-req]').forEach((b) => {
    const key = b.dataset.req, countable = key === 'params' || key === 'headers' || key === 'capture' || (key === 'body' && ['Form', 'Multipart'].includes(current.body_type));
    const label = key === 'body' && isGql(current) ? 'Query' : key[0].toUpperCase() + key.slice(1);
    put(b, label, n[key] ? h('span', { class: countable ? 'n' : 'n dot' }, countable ? n[key] : '•') : null);
    if (key === 'capture') b.classList.toggle('hidden', !n.capture && reqTab !== 'capture');
  });
}

function renderRequest() {
  setTab('req', reqTab);
  // Preview 里已经有完整的最终 URL，URL 栏下那行就不重复显示了
  $('#resolved').classList.toggle('hidden', reqTab === 'preview');
  renderRequestHeader();
  const body = $('#req-body');
  body.replaceChildren();
  if (reqTab === 'params') body.append(kvTable(current.params, 'Key', 'Value', renderRequest));
  else if (reqTab === 'headers') {
    const inh = inheritedHeaders(current).filter((hd) => !(current.auth === 'Off' && hd.key.trim().toLowerCase() === 'authorization'));
    if (inh.length) {
      const mark = () => h('td', { class: 'ctl muted', title: 'Inherited from the collection or folder — add the same header below to override it' }, '↑');
      body.append(h('table', { class: 'kv inherited' }, ...inh.map((hd) =>
        h('tr', {}, h('td', { class: 'ctl' }), h('td', { class: 'key' }, hd.key), h('td', { class: 'val', title: hd.value }, hd.value), mark()))));
    }
    body.append(kvTable(current.headers, 'Header', 'Value', renderRequest, { keyList: 'hdr-names', valList: 'ct-values' }));
  }
  else if (reqTab === 'body') body.append(bodyEditor());
  else if (reqTab === 'capture') body.append(captureEditor());
  else if (reqTab === 'preview') body.append(previewEditor());
  else {
    const ia = inheritedAuth(current);
    if (current.auth === 'Off') body.append(h('div', { class: 'helper' }, ia
      ? `${authKind(ia)} auth from the collection or folder is turned off for this request. Inherited Authorization, Cookie and Proxy-Authorization headers are dropped too; other inherited headers still apply.`
      : 'No credentials are sent. Inherited Authorization, Cookie and Proxy-Authorization headers are dropped; other inherited headers still apply.'));
    else if (ia) body.append(h('div', { class: 'helper' }, `Inheriting ${authKind(ia)} auth from the collection or folder. Pick another option to override it, or “No auth” to send nothing.`));
    body.append(authEditor());
  }
}

function radios(name, options, value, onchange) {
  return h('div', { class: 'row', role: 'radiogroup' }, ...options.map(([v, label]) =>
    h('label', {}, h('input', { type: 'radio', name, value: v, checked: v === value, onchange: () => onchange(v) }), label)));
}

/** GraphQL：上面 query，下面 variables（JSON，可格式化） */
function graphqlEditor() {
  const helper = h('div', { class: 'helper' });
  const query = h('textarea', { spellcheck: 'false', 'aria-label': 'GraphQL query', placeholder: 'query {\n  viewer { id }\n}',
    oninput: (e) => { current.body = e.target.value; dirty(); renderTabCounts(); } }, current.body);
  const vars = h('textarea', { class: 'gql-vars', spellcheck: 'false', 'aria-label': 'GraphQL variables', placeholder: '{ "id": 1 }',
    oninput: (e) => { current.graphql_variables = e.target.value; vars.removeAttribute('aria-invalid'); helper.className = 'helper'; helper.textContent = ''; dirty(); } }, current.graphql_variables || '');
  return h('div', { class: 'fill' }, h('div', { class: 'row' }, h('strong', {}, 'Query')), query,
    h('div', { class: 'row' }, h('strong', {}, 'Variables'), h('span', { class: 'muted' }, 'JSON'), h('span', { class: 'spacer' }), btn('Format JSON', () => {
      if (!vars.value.trim()) return;
      try { current.graphql_variables = vars.value = JSON.stringify(JSON.parse(vars.value), null, 2); dirty(); helper.className = 'helper'; helper.textContent = ''; vars.removeAttribute('aria-invalid'); }
      catch (err) { vars.setAttribute('aria-invalid', 'true'); helper.className = 'helper error'; helper.textContent = `Not valid JSON — ${err.message}. Fix it, then format again.`; }
    })), vars, helper);
}

function bodyEditor() {
  if (isGql(current)) return graphqlEditor();
  const wrap = h('div', { class: 'fill' }, radios('body_type', [['None', 'None'], ['Json', 'JSON'], ['Text', 'Text'], ['Form', 'Form'], ['Multipart', 'Multipart'], ['Binary', 'Binary']],
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
  } else if (current.body_type === 'Multipart') {
    wrap.append(h('div', { class: 'helper' }, 'Sent as multipart/form-data. Switch a field to File to attach a file from disk.'),
      kvTable(current.form, 'Field', 'Value', renderRequest, { files: true }));
  } else if (current.body_type === 'Binary') {
    const name = current.body.trim();
    wrap.append(h('div', { class: 'row' },
      btn(name ? name.split('/').pop() : 'Choose file…', async () => {
        const path = await dialog.open({ multiple: false });
        if (path) { current.body = path; dirty(); renderRequest(); renderTabCounts(); }
      }, 'small'),
      name ? h('span', { class: 'muted' }, name) : null,
      name ? btn('×', () => { current.body = ''; dirty(); renderRequest(); renderTabCounts(); }, 'small ghost').withAttr('aria-label', 'Clear file') : null),
      h('div', { class: 'helper' }, 'The whole file is sent as the request body. Set Content-Type in Headers if the server needs it.'));
  }
  return wrap;
}

/** Cookie 管理：列出会话里的 cookie，可单删 */
async function renderCookies() {
  const list = $('#cookie-list');
  const rows = await invoke('list_cookies');
  if (!rows.length) { put(list, h('div', { class: 'ready' }, h('strong', {}, 'No cookies'), h('span', {}, 'Send a request to a server that sets one.'))); return; }
  put(list, h('table', { class: 'kv ro cookies' }, h('tr', {}, h('th', {}, 'Host'), h('th', {}, 'Name'), h('th', {}, 'Value'), h('th', {}, 'Expires'), h('th', {})),
    ...rows.map((c) => h('tr', {},
      h('td', {}, h('code', {}, c.domain + (c.path !== '/' ? c.path : '')), c.secure ? h('span', { class: 'muted', title: 'Secure' }, ' 🔒') : null),
      h('td', {}, h('code', {}, c.name)),
      h('td', { class: 'val' }, h('code', { title: c.value }, c.value)),
      h('td', { class: 'muted' }, c.expires ? new Date(c.expires).toLocaleString() : 'Session'),
      h('td', { class: 'ctl' }, btn('×', async () => { await invoke('delete_cookie', { domain: c.domain, path: c.path, name: c.name }); renderCookies(); renderRequest(); }, 'small ghost').withAttr('aria-label', 'Delete cookie'))))));
}

/** Actual Request：变量、继承 header/auth、默认 Content-Type、会话 Cookie 全算完之后真正会发出去的请求。只读。 */
function previewEditor() {
  const wrap = h('div', { class: 'preview' }, h('div', { class: 'helper' }, 'Exactly what will be sent: variables resolved, inherited headers and auth applied, session cookies attached.'));
  invoke('final_request', { request: current, env: activeEnv(), inherited: chainFor(current) || [] }).then((f) => {
    const kvs = (rows) => h('table', { class: 'kv ro' }, ...rows.map(([k, v]) => h('tr', {}, h('td', { class: 'key' }, h('code', {}, k)), h('td', { class: 'val' }, h('code', {}, v)))));
    put(wrap, wrap.firstChild,
      h('div', { class: 'row' }, h('code', { class: 'method' }, f.method), h('code', { class: 'url' }, f.url)),
      h('h4', {}, `Headers (${f.headers.length})`), f.headers.length ? kvs(f.headers) : h('div', { class: 'muted' }, 'None'),
      f.cookies.length ? h('h4', {}, `Cookies (${f.cookies.length})`) : null, f.cookies.length ? kvs(f.cookies.map((c) => c.split(/=(.*)/s).slice(0, 2))) : null,
      f.body ? h('h4', {}, 'Body') : null, f.body ? h('pre', {}, f.body) : null);
  }, (e) => put(wrap, wrap.firstChild, h('div', { class: 'helper error' }, String(e))));
  return wrap;
}

/** 响应后把值写进当前环境的变量：变量名 + JSONPath。复用 kvTable，行就是 KeyValue。 */
function captureEditor() {
  current.captures ||= [];
  const env = activeEnv();
  return h('div', {},
    h('div', { class: 'helper' }, env
      ? `After a 2xx response, each path below is read from the JSON body and written into “${env.name}”. Use it to carry a login token into the next request.`
      : 'Select an environment first — captured values are written into the active environment.'),
    kvTable(current.captures, 'Variable', 'JSONPath, e.g. $.data.token', renderRequest));
}

/** 跑一个请求的 captures；没有可跑的规则返回 null */
function runCaptures(req, dto, env) {
  const rules = (req.captures || []).filter((c) => c.enabled && c.key.trim() && c.value.trim());
  if (!rules.length) return null;
  if (!env) return { error: 'Captured nothing — no environment is active.' };
  if (!data.environments.includes(env)) return { error: `Captured nothing — the target environment was deleted while the request was in flight.` };
  let json;
  try { json = JSON.parse(dto.body); }
  catch { return { error: "Captured nothing — the response body isn't JSON." }; }
  const done = [], failed = [], more = [];
  for (const c of rules) {
    const name = c.key.trim();
    let hits;
    try { hits = jsonPath_(json, c.value.trim()); }
    catch (e) { failed.push(`${name} (${e.message})`); continue; }
    if (!hits.length) { failed.push(`${name} (no match)`); continue; }
    const v = hits[0];
    // null 是命中了但没有值——写成字符串 "null" 会把上一次的好值冲掉
    // （{"token":null,"error":"mfa_required"} 这种 200 响应很常见）
    if (v === null || v === undefined) { failed.push(`${name} (matched null)`); continue; }
    const text = typeof v === 'string' ? v : JSON.stringify(v);
    if (hits.length > 1) more.push(`${name} matched ${hits.length}, took the first`);
    const row = env.variables.find((x) => x.key === name);
    if (row) { row.value = text; row.enabled = true; } else env.variables.push({ enabled: true, key: name, value: text });
    done.push(name);
  }
  if (done.length) dirty();
  if (!done.length) return { error: `Captured nothing — ${failed.join(', ')}.` };
  return { ok: `Captured ${done.join(', ')} into “${env.name}”.` + (failed.length ? ` Missed ${failed.join(', ')}.` : '')
    + (more.length ? ` ${more.join(', ')}.` : '') };
}

const authKind = (a) => (typeof a === 'string' ? a : Object.keys(a)[0]); // 'None' / 'Off' 是无负载变体
/** container=true 时是集合 / 文件夹的编辑器：那里没有"继承"，也就没有 Off */
function authEditor(obj = current, rerender = renderRequest, container = false) {
  const kind = authKind(obj.auth);
  const inh = container ? null : inheritedAuth(obj);
  // 有东西可继承时，None 的语义就是"跟随上层"，标签跟着变；并多给一个明确不带凭据的选项
  const opts = [['None', inh ? 'Inherit' : 'None'],
    ...(inh || kind === 'Off' ? [['Off', 'No auth']] : []), // 已经是 Off 就一直显示，否则会四个都不选中
    ['Bearer', 'Bearer token'], ['Basic', 'Basic auth'], ['ApiKey', 'API key'], ['OAuth2', 'OAuth 2.0']];
  const wrap = h('div', {}, radios(`auth-${obj.id}`, opts, kind, (v) => {
    obj.auth = { None: 'None', Off: 'Off', Bearer: { Bearer: { token: '' } }, Basic: { Basic: { username: '', password: '' } },
                 ApiKey: { ApiKey: { key: '', value: '', in_query: false } },
                 OAuth2: { OAuth2: { token_url: '', client_id: '', client_secret: '', scope: '' } } }[v];
    dirty(); rerender();
  }));
  // 字段标签走 .field（文字在上、输入框在下）——inline-flex 的 label 在窄容器里
  // 会把标签文字挤成竖排的一列
  const field = (obj, key, label, type = 'text') => h('label', { class: 'field' }, label,
    h('input', { type, value: obj[key], spellcheck: 'false', 'aria-label': label, oninput: (e) => { obj[key] = e.target.value; dirty(); } }));
  const a = obj.auth[kind];
  if (kind === 'Bearer') wrap.append(h('div', { class: 'row fields' }, field(a, 'token', 'Token')));
  else if (kind === 'Basic') wrap.append(h('div', { class: 'row fields' }, field(a, 'username', 'Username'), field(a, 'password', 'Password', 'password')));
  else if (kind === 'ApiKey') wrap.append(
    h('div', { class: 'row fields' }, field(a, 'key', 'Name'), field(a, 'value', 'Value')),
    h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: a.in_query, onchange: (e) => { a.in_query = e.target.checked; dirty(); } }),
      'Send as a query parameter instead of a header'));
  else if (kind === 'OAuth2') wrap.append(
    h('div', { class: 'helper' }, 'Client Credentials grant. On Send the token is fetched from the token URL (client ID and secret as Basic auth) and sent as a Bearer token; it is cached in memory until it expires. Environment › Manage… › Clear cookies also clears cached tokens.'),
    h('div', { class: 'row fields' }, field(a, 'token_url', 'Token URL')),
    h('div', { class: 'row fields' }, field(a, 'client_id', 'Client ID'), field(a, 'client_secret', 'Client secret', 'password')),
    h('div', { class: 'row fields' }, field(a, 'scope', 'Scope (optional)')));
  return wrap;
}

// ---------- 发送 / 取消 ----------
async function send() {
  if (isPending()) return;
  if (!current.url.trim()) { $('#url').focus(); return; }
  const m = unresolvedVars();
  // 有未定义变量：第一次点击只提示（Send 旁的槽位变成"再点一次即发送"），第二次强制发送
  if (m.length && JSON.stringify(m) !== JSON.stringify(missing)) { missing = m; renderMissing(); return; }
  missing = [];
  const id = ++jobSeq, rid = current.id, req = structuredClone(current), chain = chainFor(current) || [];
  const capEnv = activeEnv(); // 目标环境按发送时算：飞行中切环境不能把 dev 的 token 写进 prod
  pendings.set(rid, id); responses.delete(rid); sentAt.set(rid, performance.now()); flash(null);
  renderRequestHeader(); renderResponse(); renderSidebar(); renderTabs();
  let status = null, duration_ms = null, result;
  try {
    const r = await invoke('send_request', { job_id: id, request: req, env: activeEnv(),
      options: { timeout_secs: Number($('#timeout').value) || 30, inherited: chain, net: { ...net, follow_redirects: followRedirects, insecure } } });
    result = { ok: r }; status = r.status; duration_ms = r.duration_ms;
  } catch (e) {
    result = /cancelled/i.test(String(e)) ? { cancelled: true } : { error: String(e) };
  }
  pendings.delete(rid); streams.delete(rid); setResponse(rid, result);
  if (result.ok && result.ok.status < 300) {
    const cap = runCaptures(req, result.ok, capEnv);
    if (cap) toast(cap.ok || cap.error, { error: !cap.ok });
    if (cap?.ok) renderRequest();
  }
  data.history.push({ timestamp: new Date().toISOString(), request: req, status, duration_ms, response: storedResponse(result.ok) });
  if (data.history.length > HISTORY_LIMIT) data.history.splice(0, data.history.length - HISTORY_LIMIT);
  // 只有最近 HISTORY_BODIES 条留响应体
  for (let i = 0; i < data.history.length - HISTORY_BODIES; i++) data.history[i].response = null;
  saveHistory();
  sentAt.delete(rid);
  if (current.id === rid) { renderRequestHeader(); renderResponse(); flash(result.ok); }
  renderSidebar(); renderTabs();
}
const sentAt = new Map(); // request.id → 发出时刻，Waiting 计时用
// SSE / NDJSON 边收边显示：request.id → {status, headers, text}；请求结束即删，最终响应走 responses
const streams = new Map();
window.__TAURI__.event.listen('stream', ({ payload }) => {
  const rid = [...pendings].find(([, job]) => job === payload.job_id)?.[0];
  if (!rid) return;
  if (payload.kind === 'start') { streams.set(rid, { status: payload.status, headers: payload.headers, text: '', bytes: 0 }); if (current.id === rid) renderResponse(); return; }
  const live = streams.get(rid);
  if (!live) return;
  live.text += payload.text; live.bytes += new TextEncoder().encode(payload.text).length;
  if (current.id !== rid) return;
  // 已经在流式视图里：只追加，不重建（每个 chunk 重画整段文本是 O(n²)，而且会抢走滚动位置）
  const pre = $('#resp-body pre.stream'), size = $('#resp-meta .stream-size');
  if (!pre) return renderResponse();
  const atBottom = pre.scrollHeight - pre.scrollTop - pre.clientHeight < 4;
  pre.append(payload.text); if (size) size.textContent = fmtSize(live.bytes);
  if (atBottom) pre.scrollTop = pre.scrollHeight;
});

/** URL 栏右侧的瞬时结果：亮 1.5s 后变淡，切请求 / 再次发送时清掉 */
let flashTimer = null;
function flash(dto) {
  const el = $('#flash'), wrap = el.parentElement;
  clearTimeout(flashTimer);
  el.className = ''; wrap.classList.remove('flashing');
  if (!dto) return;
  put(el, h('b', { class: `s${Math.floor(dto.status / 100)}` }, dto.status), `${dto.duration_ms} ms · ${fmtSize(dto.size_bytes)}`);
  el.className = 'on'; wrap.classList.add('flashing');
  flashTimer = setTimeout(() => { el.className = 'dim'; }, 1500);
}

/** 按字节预算截断，且不切开码位——半个代理对 serde_json 会拒收，整个 save_history 都会失败 */
function clipBytes(text, maxBytes) {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length <= maxBytes) return [text, false];
  let out = new TextDecoder().decode(bytes.slice(0, maxBytes));
  if (out.endsWith('\uFFFD')) out = out.slice(0, -1); // 末尾那个被切开的字符
  return [out, true];
}

/** 存进历史的响应；二进制（图片等）不留 body，超长按字节截断，凭据类响应头不落盘 */
function storedResponse(dto) {
  if (!dto) return null;
  const [body, truncated] = clipBytes(dto.body_base64 ? '' : dto.body || '', HISTORY_BODY_MAX);
  return { status: dto.status, headers: dto.headers.filter(([k]) => !SECRET_RESP_HEADERS.has(k.toLowerCase())),
    body, duration_ms: dto.duration_ms, ttfb_ms: dto.ttfb_ms, size_bytes: dto.size_bytes, truncated };
}

// 历史现在带响应体（最多 100 条 × 64KB），每次发送都全量序列化 + 重写整个文件太贵。
// 和 dirty() 一样防抖 500ms：连点 Send 只写一次。掉的最多是最后半秒的历史，能接受。
let histTimer = null;
let histSaving = Promise.resolve();
function saveHistory() {
  clearTimeout(histTimer);
  histTimer = setTimeout(() => {
    histTimer = null;
    histSaving = invoke('save_history', { history: data.history });
    histSaving.catch((e) => toast(`Couldn't save history. ${e}`, { error: true, action: ['Retry', saveHistory] }));
  }, 500);
}
// 把两个防抖立刻冲掉。关窗前是兜底（WKWebView 上 beforeunload 在 ⌘Q 时未必触发，
// 最坏情况丢最后半秒的改动）；更新后重启前会等它和已经在写的那次都写完
function flushSaves() {
  const jobs = [saving, histSaving];
  if (histTimer) { clearTimeout(histTimer); histTimer = null; jobs.push(invoke('save_history', { history: data.history })); }
  if (saveTimer) {
    clearTimeout(saveTimer); saveTimer = null;
    jobs.push(invoke('save_collections', { collections: data.collections }),
      invoke('save_environments', { environments: data.environments }));
  }
  return Promise.all(jobs);
}
window.addEventListener('beforeunload', flushSaves);

// ---------- 自动更新 ----------
// 启动 3 秒后查一次，之后每 24 小时一次；离线、没发 latest.json 都静默。
// 下载在后台跑；macOS 上装好就替换磁盘上的 .app，正在跑的进程不受影响，下次打开就是新版。
// 装进要管理员权限的目录时插件会自己弹系统授权框，用户取消了就引导去 Release 页手动下。
const updater = window.__TAURI__.updater;
const SKIP_KEY = 'firebee.update.skip';
let update = null; // 查到的新版本（插件的 Update 对象）
let updateState = 'idle'; // idle | downloading | ready
let checking = null; // 进行中的 check()：连点菜单、手动撞上自动检查时共用一次

async function checkForUpdates(manual = false) {
  if (!updater) return;
  if (updateState === 'downloading') { if (manual) toast('An update is already downloading.'); return; }
  if (updateState === 'ready') { if (manual) showUpdateReady(); return; }
  let found;
  try { found = await (checking ??= updater.check().finally(() => { checking = null; })); }
  catch (e) { if (manual) toast(`Couldn't check for updates. ${e}`, { error: true }); return; }
  // 等 check 的这段时间里用户可能已经点了 Update，别再弹一次、再下一份
  if (updateState !== 'idle') return;
  if (!found) {
    if (manual) toast(`You're on the latest version (${await window.__TAURI__.app.getVersion()}).`);
    return;
  }
  let skipped = null;
  try { skipped = localStorage.getItem(SKIP_KEY); } catch { /* private mode */ }
  // 自动检查不打断正在用的对话框，也不再提跳过的版本；手动检查一律弹
  if (!manual && (found.version === skipped || document.querySelector('dialog[open]'))) return;
  if (update && update !== found) update.close().catch(() => {});
  update = found;
  showUpdateDialog(`Firebee ${found.version} is available`, `You have ${found.currentVersion}.`, found.body || '', [
    btn('Skip This Version', () => {
      try { localStorage.setItem(SKIP_KEY, found.version); } catch { /* private mode */ }
      $('#update-dialog').close();
    }, 'small ghost'),
    h('span', { class: 'spacer' }),
    btn('Later', () => $('#update-dialog').close()),
    btn('Update', () => { $('#update-dialog').close(); downloadUpdate(); }, 'small primary'),
  ]);
}

async function downloadUpdate() {
  updateState = 'downloading';
  const bar = $('#update-progress');
  let total = 0, got = 0;
  bar.textContent = 'Downloading update…';
  bar.classList.remove('hidden');
  try {
    await update.downloadAndInstall((ev) => {
      if (ev.event === 'Started') total = ev.data.contentLength || 0;
      else if (ev.event === 'Progress') got += ev.data.chunkLength;
      bar.textContent = ev.event === 'Finished' ? 'Installing update…'
        : total ? `Downloading update… ${Math.floor((got / total) * 100)}%` : 'Downloading update…';
    });
    updateState = 'ready';
    showUpdateReady();
  } catch (e) {
    updateState = 'idle'; // 24 小时后或下次启动再试
    toast(`Couldn't install the update. ${e}`, { error: true, action: ['Download', () => invoke('open_releases')] });
  } finally {
    bar.classList.add('hidden');
  }
}

function showUpdateReady() {
  showUpdateDialog(`Firebee ${update.version} is ready`, 'It takes effect the next time Firebee starts.', '', [
    h('span', { class: 'spacer' }),
    btn('Later', () => $('#update-dialog').close()),
    btn('Restart Now', async () => {
      try { await flushSaves(); } catch (e) { toast(`Couldn't save your changes. ${e}`, { error: true }); return; }
      invoke('restart_app');
    }, 'small primary'),
  ]);
}

function showUpdateDialog(title, sub, notes, actions) {
  $('#update-title').textContent = title;
  $('#update-sub').textContent = sub;
  $('#update-notes').textContent = notes.trim();
  put($('#update-actions'), ...actions);
  const dlg = $('#update-dialog');
  if (!dlg.open) dlg.showModal();
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

let waitTick = null;
function renderResponse() {
  const meta = $('#resp-meta'), body = $('#resp-body'), tabs = $('#resp-tabs');
  const response = resp(), pending = isPending();
  meta.replaceChildren(); body.replaceChildren();
  tabs.classList.toggle('hidden', !response?.ok);
  // 首个响应回来后分栏从 70/30 变 55/45（用户拖过就不动，见 CSS）
  if (response) $('#split').classList.add('has-resp');
  clearInterval(waitTick);
  if (!response) {
    const live = streams.get(current.id);
    if (pending && live) {
      put(meta, h('span', { class: `status s${Math.floor(live.status / 100)}` }, live.status), REASON[live.status] && h('span', { class: 'reason' }, REASON[live.status]),
        h('span', { class: 'sep' }, '·'), h('span', { class: 'spinner' }), h('span', { class: 'waiting' }, 'Streaming…'),
        h('span', { class: 'sep' }, '·'), h('span', { class: 'meta stream-size' }, fmtSize(live.bytes)));
      const pre = h('pre', { class: 'stream' }, live.text);
      body.append(pre); pre.scrollTop = pre.scrollHeight;
      return;
    }
    if (pending) {
      const el = h('span', { class: 'muted waiting' });
      const t0 = sentAt.get(current.id) ?? performance.now();
      const tick = () => { el.textContent = `${((performance.now() - t0) / 1000).toFixed(1)} s`; };
      tick(); waitTick = setInterval(tick, 100);
      meta.append(h('span', { class: 'spinner' }), h('span', { class: 'waiting' }, 'Waiting for response…'), el);
    } else body.append(h('div', { class: 'ready' }, h('strong', {}, 'Ready'),
      h('span', {}, h('span', { class: 'kbd' }, MAC ? '⌘' : 'Ctrl'), h('span', { class: 'kbd' }, '↵'), 'to send')));
    return;
  }
  if (response.cancelled) { body.append(h('div', { class: 'ready' }, h('strong', {}, 'Cancelled'), h('span', {}, 'Nothing was received.'))); return; }
  if (response.error) {
    body.append(h('div', { class: 'net-err' }, h('strong', {}, 'Request failed'), h('div', {}, response.error),
      h('div', { class: 'row' }, btn('Retry', send), btn('Switch environment', () => $('#env-select').focus(), 'small ghost'))));
    return;
  }
  const r = response.ok, ct = contentType(r);
  const isImage = ct.startsWith('image/'), isHtml = ct.includes('text/html');
  const copy = btn('Copy body', () => copyText(r.body, copy), 'small ghost');
  // put 会过滤掉 null/undefined/false；append 不会——它会把 null 当文本渲染成 "null"
  put(meta,
    h('span', { class: `status s${Math.floor(r.status / 100)}` }, r.status), REASON[r.status] && h('span', { class: 'reason' }, REASON[r.status]),
    h('span', { class: 'sep' }, '·'), h('span', { class: 'meta', title: r.ttfb_ms != null ? `Time to first byte ${r.ttfb_ms} ms (connect + server), then ${r.duration_ms - r.ttfb_ms} ms downloading` : 'Time from request start to last byte received' }, `${r.duration_ms} ms`),
    r.ttfb_ms != null && r.duration_ms - r.ttfb_ms >= 50 ? h('span', { class: 'meta muted', title: 'Time to first byte' }, `TTFB ${r.ttfb_ms} ms`) : null,
    h('span', { class: 'sep' }, '·'), h('span', { class: 'meta', title: 'Body size' }, fmtSize(r.size_bytes)),
    r.from_history ? h('span', { class: 'meta from-history', title: `Kept from ${new Date(r.from_history).toLocaleString()} — press Send for a fresh one` },
      r.truncated ? 'from history · first 64 KB' : !r.body && r.size_bytes ? 'from history · body not kept' : 'from history') : null,
    h('span', { class: 'spacer' }), r.body_base64 ? null : copy, btn('Save…', () => saveBody(r, ct), 'small ghost'),
  );
  // 跟过的每一跳都列出来；3xx 说明停下了，说清为什么并给一键跟进
  if (r.redirects?.length) body.append(h('div', { class: 'helper redirects' },
    h('span', {}, `Followed ${r.redirects.length} redirect${r.redirects.length > 1 ? 's' : ''}: `),
    ...r.redirects.flatMap((u, i) => [i ? h('span', { class: 'muted' }, ' → ') : null, h('code', {}, u)])));
  if (r.status >= 300 && r.status < 400) {
    const loc = r.headers.find(([k]) => k.toLowerCase() === 'location')?.[1];
    if (loc) body.append(h('div', { class: 'helper' },
      h('span', {}, followRedirects ? 'Not followed — a redirect to a different host would leak this request’s headers. Target: ' : 'Not followed — “Follow redirects” is off. Target: '),
      h('code', {}, loc), ' ',
      btn('Use this URL', () => {
        // Location 可以是相对的（`/login`），直接写进去会毁掉原 URL
        let next = loc;
        try { next = new URL(loc, current.url).href; } catch { /* 原样用 */ }
        current.url = next; $('#url').value = next; dirty(); renderRequest(); renderUrlMirror();
      })));
  }
  const previewTab = document.querySelector('[data-resp=preview]');
  previewTab.classList.toggle('hidden', !isHtml);
  if (respTab === 'preview' && !isHtml) respTab = 'body';
  setTab('resp', respTab);
  put(document.querySelector('[data-resp=headers]'), 'Headers', h('span', { class: 'n' }, r.headers.length));
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

  // 工具行只有一个输入框：JSON 且以 $ 开头 → JSONPath 过滤；否则 → 在正文里查找并高亮
  const isPath = () => isJson && respQuery.trim().startsWith('$');
  const count = h('span', { class: 'meta' });
  const out = h('div', { class: 'out' });
  let marks = [], cur = -1;
  const paint = () => {
    out.replaceChildren();
    let value = parsed, text = r.body;
    count.className = 'meta'; count.textContent = '';
    if (isPath()) {
      try { const res = jsonPath_(parsed, respQuery); value = res.length === 1 ? res[0] : res; count.textContent = res.length === 1 ? '1 result' : `${res.length} results`; }
      catch (e) { count.className = 'meta error'; count.textContent = e.message; }
    }
    if (isJson) text = JSON.stringify(value, null, 2);
    const full = showAll.has(current.id) || text.length <= TRUNCATE_AT;
    // JSON 默认就是可折叠树。ponytail: >1MB 不建树（DOM 节点太多会卡），退回纯文本
    if (isJson && full && text.length <= 1_000_000) out.append(jsonTree(value, null, 0, text.length > 50_000));
    else {
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
    const q = isPath() ? '' : respQuery.trim().toLowerCase();
    if (!q) { if (!isPath()) count.textContent = ''; return; }
    marks = markMatches(out, q);
    count.className = 'meta'; count.textContent = marks.length ? `${marks.length} found` : 'No matches';
    if (marks.length) gotoMark(1);
  };
  const gotoMark = (dir) => {
    if (!marks.length) return;
    marks[cur]?.classList.remove('cur');
    cur = (cur + dir + marks.length) % marks.length;
    const m = marks[cur];
    // 命中可能在折叠的节点里：把祖先 <details> 都展开再滚过去
    for (let d = m.closest('details'); d; d = d.parentElement.closest('details')) d.open = true;
    m.classList.add('cur');
    m.scrollIntoView({ block: 'center' });
    count.textContent = `${cur + 1} / ${marks.length}`;
  };
  let t, wasPath = isPath();
  body.append(h('div', { class: 'jp' },
    h('input', { id: 'find', value: respQuery, spellcheck: 'false', 'aria-label': isJson ? 'Search the body, or filter with JSONPath' : 'Find in response body',
      placeholder: isJson ? `Search or $.json.path (${MOD}F)` : `Find in body (${MOD}F)`,
      title: isJson ? 'Plain text highlights matches. Start with $ for JSONPath, e.g. $.data.items[*].id or $..id' : null,
      oninput: (e) => {
        respQuery = e.target.value; clearTimeout(t);
        // 路径模式要重建内容；纯查找只重新标记。进出路径模式都要重建一次
        t = setTimeout(() => { const p = isPath(); p || wasPath ? paint() : applyFind(); wasPath = p; }, 150);
      },
      onkeydown: (e) => {
        if (e.key === 'Enter' && !isPath()) { e.preventDefault(); gotoMark(e.shiftKey ? -1 : 1); }
        else if (e.key === 'Escape' && respQuery) { e.stopPropagation(); clearTimeout(t); respQuery = e.target.value = ''; wasPath = false; paint(); }
      } }),
    count), out);
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
    // 折叠时显示成 "key": { … }, 5 keys —— .fold 和 .n 只在折叠状态下可见（见 CSS）
    h('summary', {}, label, label && ': ', open,
      h('span', { class: 'fold' }, `…${close}${depth ? ',' : ''}`),
      h('span', { class: 'n' }, `${entries.length} ${Array.isArray(v) ? (entries.length === 1 ? 'item' : 'items') : (entries.length === 1 ? 'key' : 'keys')}`)),
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
    return isObj(v) && Object.hasOwn(v, k) ? [v[k]] : []; // hasOwn：别让 $.constructor 之类走原型链命中
  };
  const descend = (v, acc = []) => { acc.push(v); children(v).forEach((c) => descend(c, acc)); return acc; };
  let cur = [root];
  for (const tk of tokens) cur = cur.flatMap((v) => tk.deep !== undefined ? descend(v).flatMap((d) => get(d, tk.deep)) : get(v, tk.key));
  return cur;
}

// ---------- 环境管理对话框 ----------
function renderEnvDialog() {
  const list = $('#env-list'), editor = $('#env-editor');
  const newEnv = () => {
    envSel = { id: crypto.randomUUID(), name: `Environment ${data.environments.length + 1}`, variables: [] };
    data.environments.push(envSel); dirty(); renderEnvDialog(); renderTopbar();
    setTimeout(() => $('#env-editor .head input')?.select(), 0);
  };
  list.replaceChildren(
    ...data.environments.map((e) => h('button', { class: 'item' + (e === envSel ? ' active' : ''), onclick: () => { envSel = e; renderEnvDialog(); } },
      h('span', { class: 'name' }, e.name),
      e.id === activeEnvId ? h('span', { class: 'badge' }, 'Active') : h('span', { class: 'meta muted' }, `${e.variables.filter((v) => v.key).length}`))),
    btn('+ New environment', newEnv, 'small ghost new'),
  );
  editor.replaceChildren();
  if (!data.environments.length) { editor.append(emptyState('No environments yet', 'An environment is a named set of variables — dev, staging, production. Switch between them from the top bar.', ['Create environment', newEnv])); return; }
  if (!envSel) { editor.append(emptyState('Pick an environment', 'Select one on the left to edit its variables.')); return; }
  const env = envSel, isActive = env.id === activeEnvId;
  const del = () => {
    const idx = data.environments.indexOf(env);
    data.environments.splice(idx, 1);
    if (isActive) setActiveEnv(null);
    envSel = data.environments[Math.min(idx, data.environments.length - 1)] || null;
    dirty(); renderEnvDialog(); renderTopbar(); renderRequestHeader();
    toast(`Deleted environment “${env.name}”.`, { action: ['Undo', () => { data.environments.splice(idx, 0, env); if (isActive) setActiveEnv(env.id); envSel = env; dirty(); renderEnvDialog(); renderTopbar(); renderRequestHeader(); }] });
  };
  editor.append(
    h('div', { class: 'head' },
      h('input', { value: env.name, 'aria-label': 'Environment name', oninput: (e) => { env.name = e.target.value; dirty(); renderTopbar(); $('#env-list .item.active .name').textContent = env.name; } }),
      isActive ? h('span', { class: 'badge' }, 'Active') : btn('Set active', () => { setActiveEnv(env.id); missing = []; renderTopbar(); renderRequestHeader(); renderRequest(); renderEnvDialog(); }),
      btn('Duplicate', () => { const d = structuredClone(env); d.id = crypto.randomUUID(); d.name += ' copy'; data.environments.splice(data.environments.indexOf(env) + 1, 0, d); envSel = d; dirty(); renderEnvDialog(); renderTopbar(); }),
      btn('Delete', del, 'small ghost'),
    ),
    h('div', { class: 'vars-head' }, h('span'), h('span', {}, 'Name'), h('span'), h('span', {}, 'Value'), h('span')),
    h('div', { class: 'vars' }, kvTable(env.variables, 'e.g. base_url', 'e.g. https://api.example.com', renderEnvDialog, { secrets: true })),
    h('div', { class: 'helper' }, '🔒 Secret values are kept in the macOS keychain, masked in the UI, and left out of exports, project folders and history.'),
  );
}

// ---------- {{变量}} 自动补全 ----------
const ac = { el: null, field: null, items: [], cur: 0, start: 0 };
const acEligible = (el) => (el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && (el.type === 'text' || !el.getAttribute('type'))))
  && el.closest('main, #env-editor') && !el.matches('.find, .jp input');
/** 光标在输入框里的视口坐标（返回该行底部）：镜像一个同样排版的隐藏 div，量零宽标记的位置 */
const CARET_STYLES = ['fontFamily', 'fontSize', 'fontWeight', 'fontStyle', 'letterSpacing', 'wordSpacing',
  'lineHeight', 'textIndent', 'tabSize', 'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft',
  'borderTopWidth', 'borderRightWidth', 'borderBottomWidth', 'borderLeftWidth', 'width'];
function caretPoint(el) {
  const mirror = caretPoint.el ||= document.body.appendChild(
    h('div', { 'aria-hidden': 'true', style: 'position:fixed;top:0;left:0;visibility:hidden;overflow-wrap:break-word' }));
  const cs = getComputedStyle(el);
  for (const k of CARET_STYLES) mirror.style[k] = cs[k];
  mirror.style.whiteSpace = el.tagName === 'INPUT' ? 'pre' : 'pre-wrap';
  mirror.style.boxSizing = 'content-box'; // cs.width 是内容宽，镜像也按内容宽算才和原框同一个换行点
  const mark = h('span', {}, '\u200b');
  mirror.replaceChildren(document.createTextNode(el.value.slice(0, el.selectionStart)), mark);
  const box = el.getBoundingClientRect(), m = mirror.getBoundingClientRect(), c = mark.getBoundingClientRect();
  return { x: box.left + (c.left - m.left) - el.scrollLeft, y: box.top + (c.bottom - m.top) - el.scrollTop, line: c.height };
}

function acUpdate(el) {
  const before = el.value.slice(0, el.selectionStart);
  const m = /\{\{([\w$-]*)$/.exec(before);
  if (!m) return acHide();
  const prefix = m[1].toLowerCase();
  const env = activeEnv();
  const names = [...(env ? env.variables.filter((v) => v.enabled && v.key).map((v) => [v.key, v.secret ? '••••••' : v.value]) : []), ...DYNAMIC_VARS]
    .filter(([k]) => k.toLowerCase().startsWith(prefix));
  if (!names.length) return acHide();
  Object.assign(ac, { field: el, items: names, cur: 0, start: el.selectionStart - m[1].length });
  ac.el.replaceChildren(...names.map(([k, v], i) => h('div', { class: 'item' + (i === 0 ? ' cur' : ''), role: 'option',
    onmousedown: (e) => { e.preventDefault(); acAccept(i); } }, `{{${k}}}`, h('span', { class: 'muted' }, v))));
  ac.el.classList.remove('hidden');
  // 先显示再量尺寸：贴着光标放，下方放不下就翻到光标上面
  const p = caretPoint(el), pw = ac.el.offsetWidth, ph = ac.el.offsetHeight;
  const below = p.y + 4 + ph <= window.innerHeight;
  ac.el.style.left = `${Math.max(8, Math.min(p.x, window.innerWidth - pw - 8))}px`;
  ac.el.style.top = `${Math.max(8, below ? p.y + 4 : p.y - p.line - ph - 4)}px`;
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
      ac.el.children[ac.cur].scrollIntoView({ block: 'nearest' });
      e.stopPropagation();
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      // 这是在"接受候选项"，不能继续冒泡 —— URL 栏的 Enter 会把请求直接发出去
      e.preventDefault(); e.stopPropagation(); acAccept();
    }
    else if (e.key === 'Escape') { e.stopPropagation(); acHide(); }
  }, true);
  document.addEventListener('focusout', (e) => { if (e.target === ac.field) setTimeout(() => { if (document.activeElement !== ac.field) acHide(); }, 0); });
}

// ---------- 事件绑定 ----------
function bind() {
  acBind();
  $('#cookies-btn').onclick = () => { renderCookies(); $('#cookie-dialog').showModal(); };
  $('#cookie-close').onclick = () => $('#cookie-dialog').close();
  $('#clear-cookies').onclick = async () => { await invoke('clear_cookies'); toast('Cookies and cached OAuth2 tokens cleared.'); };
  $('#method').replaceChildren(...METHODS.map((m) => h('option', { value: m }, m.toUpperCase())));
  $('#method').onchange = (e) => { current.method = e.target.value; e.target.className = `m-${current.method.toLowerCase()}`; dirty(); renderSidebar(); renderTabs(); };
  $('#url').oninput = (e) => { current.url = e.target.value; dirty(); renderUrlMirror(); renderMissing(); };
  $('#url').onscroll = () => { $('#url-mirror').scrollLeft = $('#url').scrollLeft; };
  // ⌘↩ 交给全局快捷键处理；这里只管裸 Enter，否则一次按键触发两次 send，
  // 第二次会跳过"未解析变量先提示"这一步直接发出去
  $('#url').onkeydown = (e) => { if (e.key === 'Enter' && !e.metaKey && !e.ctrlKey) send(); };
  $('#url').onpaste = (e) => {
    const t = e.clipboardData?.getData('text') || '';
    if (/^\s*curl(\.exe)?\s/.test(t)) { e.preventDefault(); importCurl(t, (m) => toast(m, { error: true })); }
  };
  $('#import-close').onclick = () => $('#import-dialog').close();
  $('#import-run').onclick = async () => {
    if (await importCurl($('#import-text').value, (m) => { $('#import-error').textContent = m; }, importInto)) { $('#import-text').value = ''; $('#import-dialog').close(); }
  };
  $('#import-text').onkeydown = (e) => { if ((MAC ? e.metaKey : e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); $('#import-run').click(); } };
  $('#side-add').onclick = (e) => openMenu(e, [
    ['New request', () => { createRequest(); $('#url').focus(); }, { kbd: `${MOD}N` }],
    ['New collection', addCollection],
    ['Import file…', importFile],
    ['Open project folder…', openProject],
    ['Import from curl…', () => openImport()],
  ]);
  // Capture 有规则时本身就是一个标签；只有标签藏起来时，菜单里才放入口
  $('#req-more').onclick = (e) => {
    const tabHidden = document.querySelector('[data-req=capture]').classList.contains('hidden');
    openMenu(e, [
      tabHidden && ['Capture', () => { reqTab = 'capture'; renderRequest(); }],
      ['Request settings…', () => toggleSettings(true)],
      ...exportItems(current),
    ]);
  };
  const pop = $('#req-settings'), popBtn = $('#settings-btn');
  const toggleSettings = (open = pop.classList.contains('hidden')) => {
    pop.classList.toggle('hidden', !open); popBtn.setAttribute('aria-expanded', open);
    if (open) $('#timeout').focus();
  };
  // 不 stopPropagation：让 document 上关 #menu 的监听照常跑，两者不会同时开着
  popBtn.onclick = () => toggleSettings();
  document.addEventListener('click', (e) => { if (!pop.contains(e.target) && !popBtn.contains(e.target)) toggleSettings(false); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !pop.classList.contains('hidden')) { toggleSettings(false); popBtn.focus(); } });
  // 图标按钮不能走 copyText 的换文字逻辑（会把 svg 冲掉），只改颜色和 title
  $('#copy-resolved').onclick = (e) => {
    const b = e.currentTarget; const r = $('#resolved-url'); copyText(r.dataset.exact || r.textContent);
    b.dataset.state = 'copied'; b.title = 'Copied';
    setTimeout(() => { delete b.dataset.state; b.title = 'Copy resolved URL'; }, 1500);
  };
  $('.search .hint').textContent = `${MOD}K`;
  for (const ev of ['input', 'change']) $('#req-body').addEventListener(ev, () => { if (reqTab === 'params' || reqTab === 'auth') renderResolved(); });
  $('#theme-btn').onclick = (e) => openMenu(e, [['Light', 'light'], ['Dark', 'dark'], ['System', 'system']].map(([label, v]) =>
    [label, () => setTheme(v), { kbd: theme === v ? '✓' : '' }]));
  // 代理 / CA / 客户端证书
  const pick = async (key, filters) => { const p = await dialog.open({ multiple: false, filters }); if (p) { net[key] = p; saveNet(); renderNet(); } };
  const renderNet = () => {
    $('#proxy-mode').value = net.proxy;
    $('#proxy-custom').classList.toggle('hidden', net.proxy !== 'custom');
    $('#proxy-url').value = net.proxy_url; $('#no-proxy').value = net.no_proxy;
    for (const [key, id] of [['ca_path', 'ca-file'], ['cert_path', 'cert-file']]) {
      const el = $(`#${id}`);
      put(el, net[key] ? h('span', { class: 'file', title: net[key] }, net[key].split('/').pop()) : null,
        btn(net[key] ? '×' : 'Choose…', () => { if (net[key]) { net[key] = ''; saveNet(); renderNet(); } else pick(key, [{ name: 'PEM', extensions: ['pem', 'crt', 'cer', 'key'] }]); }, 'small ghost'));
    }
  };
  $('#proxy-mode').onchange = (e) => { net.proxy = e.target.value; saveNet(); renderNet(); };
  $('#proxy-url').oninput = (e) => { net.proxy_url = e.target.value; saveNet(); };
  $('#no-proxy').oninput = (e) => { net.no_proxy = e.target.value; saveNet(); };
  renderNet();
  $('#insecure').checked = insecure;
  $('#insecure').onchange = (e) => {
    insecure = e.target.checked;
    try { localStorage.setItem('firebee.insecure', insecure ? '1' : '0'); } catch { /* private mode */ }
  };
  $('#follow').checked = followRedirects;
  $('#follow').onchange = (e) => {
    followRedirects = e.target.checked;
    try { localStorage.setItem('firebee.follow', followRedirects ? '1' : '0'); } catch { /* private mode */ }
  };
  $('#send').onclick = send;
  $('#cancel').onclick = () => invoke('cancel_request', { job_id: pendings.get(current.id) });
  $('#export-copy').onclick = (e) => copyText($('#export-text').textContent, e.currentTarget);
  $('#export-close').onclick = () => $('#export-dialog').close();
  $('#env-select').onchange = (e) => {
    if (e.target.value === '__manage') { e.target.value = activeEnvId || ''; envSel = activeEnv(); renderEnvDialog(); $('#env-dialog').showModal(); return; }
    setActiveEnv(e.target.value);
      missing = []; renderRequestHeader(); renderRequest();
  };
  // 原生菜单（macOS 菜单栏里可见快捷键）触发的动作
  window.__TAURI__.event?.listen('menu', ({ payload }) => {
    if (payload === 'send') send();
    else if (payload === 'new') { createRequest(); $('#url').focus(); }
    else if (payload === 'find') { const f = $('#find') || $('#search'); f.focus(); f.select(); }
    else if (payload === 'filter') { $('#search').focus(); $('#search').select(); }
    else if (payload === 'check-updates') checkForUpdates(true);
  });
  $('#env-close').onclick = () => $('#env-dialog').close();
  $('#env-dialog').addEventListener('close', renderRequestHeader); // 对话框里改过的变量要反映到 URL 预览

  document.querySelectorAll('[data-side]').forEach((b) => b.onclick = () => { sideTab = b.dataset.side; renderSidebar(); });
  $('#search').oninput = (e) => { filter = e.target.value; renderSidebar(); };
  $('#search').onkeydown = (e) => { if (e.key === 'Escape' && filter) { e.stopPropagation(); filter = e.target.value = ''; renderSidebar(); } };
  document.querySelectorAll('[data-req]').forEach((b) => b.onclick = () => { reqTab = b.dataset.req; renderRequest(); });
  document.querySelectorAll('[data-resp]').forEach((b) => b.onclick = () => { respTab = b.dataset.resp; renderResponse(); });
  // 全局快捷键：⌘↩ 发送 · ⌘S 保存到集合 · ⌘N 新请求
  document.addEventListener('keydown', (e) => {
    const mod = MAC ? e.metaKey : e.ctrlKey;
    if (!mod) return;
    // 模态对话框开着时，⌘↩ 会把背后的请求发出去、⌘W 会关掉背后的标签页
    if (document.querySelector('dialog[open]')) return;
    if (e.key === 'Enter') { e.preventDefault(); send(); }
    else if (e.key === 'n' || e.key === 't') { e.preventDefault(); createRequest(); $('#url').focus(); }
    else if (e.key === 'k') { e.preventDefault(); sideTab = 'collections'; renderSidebar(); $('#search').focus(); $('#search').select(); }
    else if (e.key === '/') { e.preventDefault(); $('#url').focus(); }
    else if (e.key === 'f') { e.preventDefault(); const f = $('#find') || $('#search'); f.focus(); f.select(); }
    else if (e.key === 'w') { e.preventDefault(); closeTab(current.id); }
    else if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      e.preventDefault();
      const i = openTabs.indexOf(current.id);
      const next = findById(openTabs[(i + (e.key === 'ArrowRight' ? 1 : -1) + openTabs.length) % openTabs.length]);
      if (next) selectRequest(next);
    }
  });
  document.addEventListener('keydown', (e) => {
    const el = document.activeElement;
    // 输入框里的退格是编辑文本，不是删请求（#search、重命名输入框都在 #sidebar 内）
    const typing = el?.matches('input, textarea, [contenteditable]');
    if ((e.key === 'Backspace' || e.key === 'Delete') && selected.size > 1 && !typing && el?.closest('#sidebar')) { e.preventDefault(); deleteSelected(); }
    else if (e.key === 'Escape' && selected.size) { selected.clear(); renderSidebar(); }
  });
  splitter($('#split-side'), '--side-w', (ev) => ev.clientX, 180, () => window.innerWidth * 0.5, 'firebee.sideW');
  splitter($('#split-main'), '--req-w', (ev) => ev.clientX - $('#request').getBoundingClientRect().left, 360,
    () => $('#split').getBoundingClientRect().width - 266, 'firebee.reqW');
  $('#send').title = `Send (${MOD}↩)`;
}

async function main() {
  bind();
  // 内置变量清单必须在首屏渲染前就位，否则 {{$timestamp}} 会被当成未定义变量报出来
  [data, DYNAMIC_VARS] = await Promise.all([
    invoke('load_data'),
    invoke('dynamic_vars').catch((e) => { toast(`Built-in variables unavailable. ${e}`, { error: true }); return []; }),
  ]);
  const restored = openTabs.map((id) => findById(id)).filter(Boolean);
  current = restored[0] || firstRequest() || (homeCollection().requests.push(current), dirty(), current);
  openTabs = restored.length ? restored.map((r) => r.id) : [current.id];
  saveTabs();
  renderTopbar(); renderTabs(); renderSidebar(); renderRequest(); renderResponse();
  $('#url').focus();
  // 开发版（cargo tauri dev）不自动检查，菜单里手动查仍可用
  const autoCheck = () => { if (!debugBuild) checkForUpdates(); };
  setTimeout(autoCheck, 3000);
  setInterval(autoCheck, 24 * 60 * 60 * 1000);
}
main();
