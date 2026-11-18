// AvG — логика интерфейса. Без фреймворков: состояние → HTML-строка → DOM.
import { call, listen, pickApks, pickAdb, saveApk, onDragDrop, isTauri } from './api.js';
import { icon, hydrateIcons } from './icons.js';

// ───────────────────────── состояние ─────────────────────────
const S = {
  view: 'main',
  adbMissing: false,
  adbError: '',
  devices: [],
  serial: null,
  scan: null,
  scanning: false,
  scanError: '',
  profile: 'weak',
  plan: null,
  planLoading: false,
  selected: new Set(),
  apply: null,
  snapshots: [],
  snapSel: null,
  apps: null,
  appsLoading: false,
  appsTab: 'all',
  appsQuery: '',
  consoleLog: [],
  consoleBusy: false,
  info: null,
  busy: false,
};

const CATEGORY = {
  bloat: 'Реклама и мусор',
  ui: 'Интерфейс',
  performance: 'Производительность',
  battery: 'Батарея',
  network: 'Сеть',
  privacy: 'Приватность',
};
const RISK = { safe: 'Безопасно', medium: 'Средний риск', high: 'Рискованно' };
const PROFILE_TEXT = {
  weak: 'Ускорение слабого телефона: убираем мусор производителя, ускоряем анимации, компилируем приложения.',
  battery: 'Дольше от одной зарядки: меньше фоновых служб и сканирования сетей.',
  privacy: 'Меньше слежки: аналитика и реклама производителя, блокировка трекеров через DNS.',
  custom: 'Все подходящие этому телефону улучшения. Выберите нужные в плане.',
};

// ───────────────────────── утилиты ─────────────────────────
const $ = (sel, root = document) => root.querySelector(sel);
const view = $('#view');

function esc(v) {
  return String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

function plural(n, forms) {
  const a = Math.abs(n) % 100, b = a % 10;
  if (a > 10 && a < 20) return forms[2];
  if (b > 1 && b < 5) return forms[1];
  if (b === 1) return forms[0];
  return forms[2];
}

const gb = (kb) => (kb / 1048576).toFixed(kb >= 10 * 1048576 ? 0 : 1).replace('.', ',');

function fmtDate(secs) {
  const d = new Date(secs * 1000);
  const today = new Date();
  const time = d.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
  if (d.toDateString() === today.toDateString()) return `Сегодня, ${time}`;
  return `${d.toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' })}, ${time}`;
}

function errText(e) {
  return typeof e === 'string' ? e : e?.message || JSON.stringify(e);
}

function toast(text, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = text;
  $('#toasts').appendChild(el);
  setTimeout(() => el.remove(), kind === 'err' ? 8000 : 4500);
}

/** Модальное подтверждение. Возвращает true/false. */
function confirmBox({ title, text, ok = 'Продолжить', danger = false, cancel = 'Отмена' }) {
  return new Promise((resolve) => {
    const root = $('#modal-root');
    root.innerHTML = `
      <div class="modal-back">
        <div class="modal" role="dialog" aria-modal="true" aria-labelledby="m-title">
          <h2 id="m-title">${esc(title)}</h2>
          <p>${esc(text)}</p>
          <div class="btns">
            ${cancel ? `<button class="btn ghost" data-r="0">${esc(cancel)}</button>` : ''}
            <button class="btn ${danger ? 'danger' : 'primary'}" data-r="1">${esc(ok)}</button>
          </div>
        </div>
      </div>`;
    const done = (v) => { root.innerHTML = ''; document.removeEventListener('keydown', onKey); resolve(v); };
    const onKey = (e) => { if (e.key === 'Escape') done(false); };
    document.addEventListener('keydown', onKey);
    root.querySelectorAll('[data-r]').forEach((b) => b.addEventListener('click', () => done(b.dataset.r === '1')));
    root.querySelector('[data-r="1"]').focus();
  });
}

let renderQueued = false;
function render() {
  if (renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => {
    renderQueued = false;
    renderNow();
  });
}

function renderNow() {
  document.querySelectorAll('.nav-item').forEach((a) => {
    const active = a.dataset.view === S.view || (S.view === 'plan' || S.view === 'apply' ? a.dataset.view === 'main' : false);
    a.classList.toggle('active', active);
    if (active) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current');
  });
  renderChip();
  const views = { main: viewMain, plan: viewPlan, apply: viewApply, history: viewHistory, apps: viewApps, console: viewConsole, settings: viewSettings };
  const logBody = $('.log-body', view);
  const stick = logBody ? logBody.scrollHeight - logBody.scrollTop - logBody.clientHeight < 40 : true;
  view.innerHTML = `<div class="page">${(views[S.view] || viewMain)()}</div>`;
  hydrateIcons(view);
  const lb = $('.log-body', view);
  if (lb && stick) lb.scrollTop = lb.scrollHeight;
  if (S.view === 'apps') renderAppRows();
}

function go(v) {
  S.view = v;
  if (location.hash !== `#${v}`) history.replaceState(null, '', `#${v}`);
  if (v === 'history') loadSnapshots();
  if (v === 'apps' && !S.apps && device()) loadApps();
  if (v === 'settings') loadInfo();
  render();
  view.scrollTop = 0;
}

const device = () => S.devices.find((d) => d.serial === S.serial && d.state === 'device');

// ───────────────────────── устройства ─────────────────────────
async function pollDevices() {
  try {
    const list = await call('list_devices');
    S.adbMissing = false;
    const prev = S.serial;
    const prevState = S.devices.find((d) => d.serial === prev)?.state;
    S.devices = list;
    if (!list.some((d) => d.serial === S.serial)) {
      S.serial = (list.find((d) => d.state === 'device') || list[0])?.serial ?? null;
    }
    const cur = S.devices.find((d) => d.serial === S.serial);
    const changed = prev !== S.serial || prevState !== cur?.state;
    if (changed) {
      S.scan = null; S.plan = null; S.apps = null; S.scanError = '';
      if (cur?.state === 'device') scanDevice();
      if (S.view === 'history') loadSnapshots();
      render();
    } else {
      renderChip();
    }
  } catch (e) {
    const msg = errText(e);
    const missing = msg.includes('adb не найден');
    if (missing !== S.adbMissing || S.devices.length) {
      S.adbMissing = missing;
      S.adbError = msg;
      S.devices = [];
      S.serial = null;
      render();
    }
  }
}

function renderChip() {
  const chip = $('#device-chip');
  const d = S.devices.find((x) => x.serial === S.serial);
  let dot = '', title, sub;
  if (S.adbMissing) { dot = 'warn'; title = 'adb не найден'; sub = 'Укажите путь в настройках'; }
  else if (!d) { title = 'Телефон не подключён'; sub = 'Подключите по USB'; }
  else if (d.state === 'unauthorized') { dot = 'warn'; title = d.model || d.serial; sub = 'Подтвердите отладку на телефоне'; }
  else if (d.state !== 'device') { dot = 'warn'; title = d.model || d.serial; sub = `Состояние: ${d.state}`; }
  else {
    dot = 'on';
    title = S.scan?.info.display_name || d.model || d.serial;
    sub = `${d.transport === 'wifi' ? 'Wi-Fi' : 'USB'} · отладка включена`;
  }
  const select = S.devices.length > 1
    ? `<select aria-label="Выбор телефона" id="dev-select">${S.devices.map((x) =>
        `<option value="${esc(x.serial)}" ${x.serial === S.serial ? 'selected' : ''}>${esc(x.model || x.serial)}</option>`).join('')}</select>`
    : '';
  chip.innerHTML = `<div class="dot ${dot}"></div><div class="t" style="flex:1"><b>${esc(title)}</b><span>${esc(sub)}</span>${select}</div>`;
  $('#dev-select', chip)?.addEventListener('change', (e) => {
    S.serial = e.target.value;
    S.scan = null; S.plan = null; S.apps = null;
    scanDevice();
    render();
  });
}

async function scanDevice() {
  if (!device() || S.scanning) return;
  S.scanning = true; S.scanError = '';
  render();
  try {
    S.scan = await call('scan_device', { serial: S.serial });
  } catch (e) {
    S.scanError = errText(e);
  } finally {
    S.scanning = false;
    render();
  }
}

// ───────────────────────── главная ─────────────────────────
function viewMain() {
  const head = (sub) => `
    <header class="head"><div class="tt"><h1>Оптимизация</h1><p class="sub">${sub}</p></div>
    ${device() ? `<button class="btn" data-act="rescan" ${S.scanning ? 'disabled' : ''}>${icon('refresh', 16)}Пересканировать</button>` : ''}</header>`;

  if (S.adbMissing) {
    return head('Для работы нужен adb из Android platform-tools.') + `
      <section class="card empty">
        <h2>adb не найден</h2>
        <ol class="steps-list">
          <li>Скачайте <b>SDK Platform-Tools</b> с сайта developer.android.com (бесплатно, от Google).</li>
          <li>Распакуйте папку <code>platform-tools</code> рядом с AvG — программа найдёт её сама.</li>
          <li>Или укажите путь к <code>adb.exe</code> вручную.</li>
        </ol>
        <button class="btn primary" data-act="pick-adb">${icon('folder', 16)}Указать путь к adb</button>
      </section>`;
  }

  const d = S.devices.find((x) => x.serial === S.serial);
  if (!d) {
    return head('Подключите телефон кабелем — остальное AvG сделает сам.') + `
      <section class="card empty">
        <div class="tile">${icon('usb', 22)}</div>
        <h2>Как подключить телефон</h2>
        <ol class="steps-list">
          <li>Откройте <b>Настройки → О телефоне</b> и 7 раз нажмите на «Номер сборки» (на Xiaomi — «Версия MIUI/HyperOS»).</li>
          <li>В <b>Настройки → Для разработчиков</b> включите «Отладка по USB». На Xiaomi также включите «Отладка по USB (настройки безопасности)».</li>
          <li>Подключите телефон кабелем к компьютеру и разрешите отладку во всплывающем окне.</li>
        </ol>
        <p class="note">${icon('refresh', 16)}AvG проверяет подключение автоматически.</p>
      </section>`;
  }
  if (d.state === 'unauthorized') {
    return head('Осталось подтвердить отладку.') + `
      <section class="card empty">
        <div class="tile">${icon('phone', 22)}</div>
        <h2>Разрешите отладку на телефоне</h2>
        <p class="sub">На экране телефона появилось окно «Разрешить отладку по USB?». Отметьте «Всегда разрешать с этого компьютера» и нажмите «Разрешить».</p>
        <p class="note">Окна нет? Отключите и снова подключите кабель или переключите режим USB на «Передача файлов».</p>
      </section>`;
  }
  if (d.state !== 'device') {
    return head('Телефон подключён, но недоступен.') + `<section class="card empty"><h2>Состояние: ${esc(d.state)}</h2><p class="sub">Переподключите кабель. В Linux проверьте правила udev.</p></section>`;
  }
  if (S.scanError) {
    return head('Не удалось просканировать телефон.') + `
      <section class="card empty"><h2>Ошибка сканирования</h2><p class="sub selectable">${esc(S.scanError)}</p>
      <button class="btn primary" data-act="rescan">${icon('refresh', 16)}Попробовать снова</button></section>`;
  }
  if (!S.scan) {
    return head('Смотрю, что у телефона внутри…') + `
      <section class="card empty"><div style="display:flex;gap:14px;align-items:center"><div class="spinner"></div><h2>Сканирую телефон</h2></div>
      <p class="sub">Собираю модель, прошивку, память и список приложений. Обычно это 2–5 секунд.</p></section>`;
  }

  const { info, profiles, categories } = S.scan;
  const prof = profiles.find((p) => p.id === S.profile) || profiles[0];
  const storagePct = info.storage_total_kb ? Math.round((info.storage_used_kb / info.storage_total_kb) * 100) : 0;
  const cats = categories[prof.id] || [];
  const system = [`Android ${info.android}`, info.rom].filter(Boolean).join(' · ');

  return head('AvG просканировал телефон и подобрал улучшения под него.') + `
    <div class="row">
      <section class="card" aria-label="Устройство" style="flex:1 1 320px;display:flex;flex-direction:column;gap:20px">
        <div class="dev-head"><div class="tile">${icon('phone', 22)}</div>
          <div style="display:flex;flex-direction:column;gap:3px;min-width:0"><span class="dev-name">${esc(info.display_name)}</span>
          <span class="mono selectable">${esc(info.codename)} · ${esc(S.serial)}</span></div></div>
        <div class="stats">
          <div class="stat"><span class="label">Система</span><b>${esc(system)}</b></div>
          <div class="stat"><span class="label">Оперативная память</span><b>${gb(info.ram_total_kb)} ГБ · свободно ${gb(info.ram_available_kb)}</b></div>
          <div class="stat"><span class="label">Пакеты</span><b>${info.packages_total} · системных ${info.packages_system}</b></div>
          <div class="stat"><span class="label">Батарея</span><b>${info.battery_level ?? '—'}%${info.battery_temp_c != null ? ` · ${Math.round(info.battery_temp_c)} °C` : ''}</b></div>
        </div>
        ${info.storage_total_kb ? `<div style="display:flex;flex-direction:column;gap:8px">
          <div style="display:flex;justify-content:space-between;font-size:13px"><span class="label">Хранилище</span><b>${gb(info.storage_used_kb)} из ${gb(info.storage_total_kb)} ГБ</b></div>
          <div class="bar ${storagePct > 75 ? 'warn' : ''}"><div style="width:${storagePct}%"></div></div></div>` : ''}
      </section>

      <section class="card" aria-label="Результат сканирования" style="flex:2 1 460px;display:flex;flex-direction:column;gap:20px;padding:28px">
        <span class="overline">Результат сканирования</span>
        ${prof.count
          ? `<div style="display:flex;align-items:baseline;gap:14px;flex-wrap:wrap"><span class="big-num">${prof.count}</span>
             <span style="font-size:22px;font-weight:800">${plural(prof.count, ['улучшение найдено', 'улучшения найдено', 'улучшений найдено'])}</span></div>`
          : `<div style="display:flex;align-items:center;gap:14px"><span class="tile" style="color:var(--accent)">${icon('check', 22)}</span>
             <span style="font-size:22px;font-weight:800">В этом профиле всё уже сделано</span></div>`}
        <p class="sub" style="color:var(--text-2);max-width:580px">${esc(PROFILE_TEXT[prof.id] || '')} Каждое изменение можно отменить.</p>
        <div style="display:flex;flex-direction:column;gap:10px"><span class="label">Профиль</span>
          <div class="pills" role="radiogroup" aria-label="Профиль оптимизации">
            ${profiles.map((p) => `<button class="pill" role="radio" aria-checked="${p.id === prof.id}" data-act="profile" data-id="${p.id}">${esc(p.label)}<span class="n">${p.count}</span></button>`).join('')}
          </div></div>
        <div style="display:flex;flex-wrap:wrap;align-items:center;gap:18px">
          <button class="btn primary big" data-act="optimize" ${prof.count ? '' : 'disabled'}>${icon('zap', 18)}Оптимизировать</button>
          <button class="linkbtn" data-act="open-plan" ${prof.count ? '' : 'disabled'}>Посмотреть план</button>
          <span class="note">${icon('shield', 16)}Перед изменениями создаётся точка отката</span>
        </div>
      </section>
    </div>
    ${cats.length ? `<section style="display:flex;flex-direction:column;gap:14px" aria-label="Что нашёл сканер"><h2>Что нашёл сканер</h2>
      <div class="findings">${cats.map((c) => `<div class="card finding"><span class="num">${c.count}</span>
        <span class="title">${esc(CATEGORY[c.category] || c.category)}</span><span class="badge ${c.max_risk}">${{ safe: 'Безопасно', medium: 'Есть средний риск', high: 'Есть рискованное' }[c.max_risk]}</span></div>`).join('')}</div></section>` : ''}`;
}

// ───────────────────────── план ─────────────────────────
async function loadPlan() {
  S.planLoading = true; S.plan = null;
  render();
  try {
    S.plan = await call('get_plan', { serial: S.serial, profile: S.profile });
    S.selected = new Set(S.plan.filter((i) => i.selected).map((i) => i.id));
  } catch (e) {
    toast(errText(e), 'err');
  } finally {
    S.planLoading = false;
    render();
  }
}

function viewPlan() {
  const label = S.scan?.profiles.find((p) => p.id === S.profile)?.label || '';
  const head = `<header class="head"><div class="tt">
      <a class="back" data-act="go" data-v="main">← Назад</a>
      <h1>План оптимизации</h1>
      <p class="sub">Профиль «${esc(label)}». Снимите галочку с того, что трогать не нужно.</p></div>
      <div class="pills" aria-hidden="true"><span class="badge safe">Безопасно</span><span class="badge medium">Средний риск</span><span class="badge high">Рискованно</span></div></header>`;
  if (S.planLoading || !S.plan) return head + `<div class="card" style="display:flex;gap:14px;align-items:center"><div class="spinner"></div>Строю план…</div>`;
  if (!S.plan.length) return head + `<div class="card empty"><h2>Улучшать нечего</h2><p class="sub">Всё из этого профиля уже применено.</p></div>`;

  const groups = {};
  for (const it of S.plan) (groups[it.category] ||= []).push(it);
  const total = S.plan.length;
  const sel = S.plan.filter((i) => S.selected.has(i.id)).length;
  const hasCompile = S.plan.some((i) => i.id === 'general.compile' && S.selected.has(i.id));

  return head + Object.entries(groups).map(([cat, items]) => `
    <section class="card flush">
      <div class="group-head"><h2>${esc(CATEGORY[cat] || cat)}</h2><span class="label" style="font-size:13px">${items.filter((i) => S.selected.has(i.id)).length} из ${items.length}</span></div>
      ${items.map((it) => {
        const on = S.selected.has(it.id);
        const cmds = it.commands.slice(0, 3).join('\n') + (it.commands.length > 3 ? `\n… ещё ${it.commands.length - 3}` : '');
        return `<label class="item ${on ? '' : 'off'}">
          <input type="checkbox" data-check="${esc(it.id)}" ${on ? 'checked' : ''}>
          <div class="body"><span class="t">${esc(it.title)}</span><span class="d">${esc(it.description)}${it.reversible ? '' : ' Откат не требуется.'}</span>
          <code style="white-space:pre-line">${esc(cmds)}</code></div>
          <span class="badge ${it.risk}">${RISK[it.risk]}</span></label>`;
      }).join('')}
    </section>`).join('') + `
    <div class="footer-bar">
      <div style="display:flex;flex-direction:column;gap:3px"><b style="font-size:15px">Выбрано ${sel} из ${total}</b>
      <span class="label" style="font-size:13px">${hasCompile ? 'Компиляция займёт до 15 минут' : 'Займёт меньше минуты'} · всё можно откатить</span></div>
      <div style="display:flex;gap:10px;flex-wrap:wrap">
        <button class="btn ghost" data-act="select-safe">Только безопасное</button>
        <button class="btn primary" data-act="apply" ${sel ? '' : 'disabled'}>Применить ${sel} ${plural(sel, ['изменение', 'изменения', 'изменений'])}</button>
      </div>
    </div>`;
}

// ───────────────────────── применение ─────────────────────────
async function startApply(items) {
  if (!items.length) return;
  S.apply = {
    total: items.length,
    steps: items.map((i) => ({ title: i.title, status: 'wait' })),
    log: [],
    done: false,
    number: null,
    snapshot: null,
    ok: 0, failed: 0, cancelled: false, cancelling: false,
  };
  S.busy = true;
  go('apply');
  try {
    await call('apply_plan', { serial: S.serial, profile: S.profile, ids: items.map((i) => i.id) });
  } catch (e) {
    S.apply.log.push({ level: 'err', text: errText(e) });
    S.apply.done = true;
    toast(errText(e), 'err');
  }
  S.busy = false;
  S.plan = null;
  try { S.scan = await call('scan_device', { serial: S.serial }); } catch { /* телефон мог отключиться */ }
  render();
  if (S.apply.cancelled && S.apply.snapshot && S.apply.ok + S.apply.failed > 0) {
    const yes = await confirmBox({ title: 'Откатить уже сделанное?', text: `До остановки успело примениться изменений: ${S.apply.ok + S.apply.failed}. Вернуть их?`, ok: 'Откатить', cancel: 'Оставить' });
    if (yes) await revertSnapshot(S.apply.snapshot);
  }
}

function onApplyEvent(ev) {
  const a = S.apply;
  if (!a) return;
  switch (ev.kind) {
    case 'start': a.number = ev.number; a.snapshot = ev.snapshot; a.log.push({ level: 'info', text: `Точка отката №${ev.number} создана` }); break;
    case 'step': if (a.steps[ev.index]) a.steps[ev.index].status = 'run'; break;
    case 'log': a.log.push({ level: ev.level, text: ev.level === 'cmd' ? `$ adb shell ${ev.text}` : ev.text }); break;
    case 'step_done': if (a.steps[ev.index]) a.steps[ev.index].status = ev.ok ? 'done' : 'fail'; break;
    case 'done':
      Object.assign(a, { done: true, ok: ev.ok, failed: ev.failed, cancelled: ev.cancelled });
      a.steps.forEach((s) => { if (s.status === 'wait' || s.status === 'run') s.status = 'skip'; });
      a.log.push({ level: ev.failed ? 'err' : 'ok', text: ev.cancelled ? 'Остановлено пользователем' : `Готово: успешно ${ev.ok}, с ошибками ${ev.failed}` });
      toast(ev.cancelled ? 'Оптимизация остановлена' : ev.failed ? `Готово, но ${ev.failed} ${plural(ev.failed, ['изменение', 'изменения', 'изменений'])} с ошибками` : 'Готово! Телефон оптимизирован', ev.failed ? 'err' : 'ok');
      break;
  }
  if (S.view === 'apply') render();
}

function viewApply() {
  const a = S.apply;
  if (!a) return `<div class="card empty"><h2>Сейчас ничего не применяется</h2><button class="btn" data-act="go" data-v="main">На главную</button></div>`;
  const finished = a.steps.filter((s) => ['done', 'fail', 'skip'].includes(s.status)).length;
  const pct = a.done ? 100 : Math.round((finished / a.total) * 100);
  const cur = a.steps.findIndex((s) => s.status === 'run');
  const stepIcon = {
    done: `<span class="ic done" role="img" aria-label="Готово">${icon('check', 14)}</span>`,
    fail: `<span class="ic fail" role="img" aria-label="Ошибка">${icon('x', 14)}</span>`,
    run: `<span class="spinner" role="img" aria-label="Выполняется"></span>`,
    wait: `<span class="ic wait" role="img" aria-label="Ожидает"></span>`,
    skip: `<span class="ic wait" role="img" aria-label="Пропущено"></span>`,
  };
  const title = a.done ? (a.cancelled ? 'Остановлено' : a.failed ? 'Готово, но не всё' : 'Готово!') : 'Применяю изменения';
  const sub = a.done
    ? `Точка отката №${a.number ?? '—'} сохранена — всё можно вернуть в разделе «История и откат».${a.steps.some((s) => s.title.includes('разрешение')) ? ' Если экран выглядит странно, перезагрузите телефон.' : ''}`
    : `Не отключайте телефон.${a.number ? ` Точка отката №${a.number} создана.` : ''}`;
  return `
    <header class="head"><div class="tt"><h1>${title}</h1><p class="sub">${sub}</p></div></header>
    <section class="card" aria-label="Прогресс" style="display:flex;flex-direction:column;gap:14px">
      <div style="display:flex;justify-content:space-between;align-items:baseline;gap:8px;flex-wrap:wrap">
        <span class="big-num" style="font-size:40px">${pct}%</span>
        <b style="font-size:15px;color:var(--text-2)">${a.done ? `Успешно: ${a.ok} · с ошибками: ${a.failed}` : `Шаг ${Math.min(cur + 1 || finished + 1, a.total)} из ${a.total}`}</b></div>
      <div class="bar big" role="progressbar" aria-valuenow="${pct}" aria-valuemin="0" aria-valuemax="100" aria-label="Общий прогресс"><div style="width:${pct}%"></div></div>
    </section>
    <div class="row">
      <section class="card flush" aria-label="Шаги" style="flex:1 1 320px;padding:10px 0">
        ${a.steps.map((s) => `<div class="step ${s.status}">${stepIcon[s.status]}<span>${esc(s.title)}</span></div>`).join('')}
      </section>
      <section class="log" aria-label="Журнал команд" style="flex:1.3 1 380px">
        <div class="log-head"><b style="font-size:13px;color:var(--text-2)">Журнал ADB</b><button class="btn sm ghost" data-act="copy-log">Копировать</button></div>
        <div class="log-body">${a.log.map((l) => `<div class="${l.level}">${esc(l.text)}</div>`).join('')}</div>
      </section>
    </div>
    <div style="display:flex;flex-wrap:wrap;gap:10px;justify-content:flex-end">
      ${a.done
        ? `<button class="btn" data-act="go" data-v="history">${icon('history', 16)}История изменений</button><button class="btn primary" data-act="go" data-v="main">На главную</button>`
        : `<button class="btn danger" data-act="cancel" ${a.cancelling ? 'disabled' : ''}>${a.cancelling ? 'Останавливаю после текущего шага…' : 'Остановить'}</button>`}
    </div>`;
}

// ───────────────────────── история ─────────────────────────
async function loadSnapshots() {
  try {
    S.snapshots = await call('list_snapshots', { serial: S.serial });
    if (!S.snapshots.some((s) => s.id === S.snapSel)) S.snapSel = S.snapshots[0]?.id ?? null;
  } catch (e) {
    toast(errText(e), 'err');
  }
  render();
}

function showRevertResult(r, what) {
  if (r.ok) toast(`${what}: отменено ${r.reverted} ${plural(r.reverted, ['изменение', 'изменения', 'изменений'])}`, 'ok');
  else toast(`${what}: не удалось отменить ${r.failed}. ${r.log.filter((l) => !l.startsWith('$')).slice(-1)[0] || ''}`, 'err');
}

async function revertSnapshot(id) {
  try {
    const r = await call('revert_snapshot', { serial: S.serial, snapshot: id });
    showRevertResult(r, 'Откат');
  } catch (e) { toast(errText(e), 'err'); }
  await afterRevert();
}

async function afterRevert() {
  await loadSnapshots();
  S.plan = null;
  try { S.scan = await call('scan_device', { serial: S.serial }); } catch { /* нет телефона */ }
  render();
}

function viewHistory() {
  const head = `<header class="head"><div class="tt"><h1>История и откат</h1>
    <p class="sub">Каждый запуск сохраняет точку отката. Отменяйте всё сразу или по одному изменению.</p></div>
    ${S.snapshots.some((s) => s.changes.some((c) => !c.reverted && c.revert.length)) && device()
      ? `<button class="btn danger" data-act="revert-all">${icon('undo', 16)}Вернуть всё как было до AvG</button>` : ''}</header>`;
  if (!S.snapshots.length) {
    return head + `<div class="card empty"><h2>Пока пусто</h2><p class="sub">${device() ? 'После первой оптимизации здесь появятся точки отката.' : 'Подключите телефон, чтобы увидеть его историю.'}</p></div>`;
  }
  const cur = S.snapshots.find((s) => s.id === S.snapSel) || S.snapshots[0];
  const active = (s) => s.changes.filter((c) => !c.reverted && c.revert.length).length;
  const meta = (s) => {
    const n = s.changes.length, a = active(s);
    return `${n} ${plural(n, ['изменение', 'изменения', 'изменений'])} · ${a ? `активно ${a}` : 'откачено'}`;
  };
  return head + `
    <div class="row" style="align-items:flex-start">
      <section aria-label="Точки отката" style="flex:1 1 280px;display:flex;flex-direction:column;gap:10px">
        ${S.snapshots.map((s) => `<button class="snap" aria-pressed="${s.id === cur.id}" data-act="snap" data-id="${s.id}">
          <span class="top"><b>№${s.number} · ${esc(s.profile_label)}</b><span>${fmtDate(s.created)}</span></span>
          <span>${esc(meta(s))}${s.serial !== S.serial ? ` · ${esc(s.device)}` : ''}</span></button>`).join('')}
      </section>
      <section class="card flush" aria-label="Изменения" style="flex:2 1 420px;min-width:0">
        <div class="group-head" style="flex-wrap:wrap;gap:12px">
          <div style="display:flex;flex-direction:column;gap:3px"><h2 style="font-size:17px">Точка №${cur.number} · ${esc(cur.profile_label)}</h2>
          <span class="label" style="font-size:13px">${fmtDate(cur.created)} · ${esc(cur.device)}</span></div>
          ${active(cur) && cur.serial === S.serial && device() ? `<button class="btn primary" data-act="revert-snap" data-id="${cur.id}">${icon('undo', 16)}Откатить всю точку</button>` : ''}
        </div>
        ${cur.changes.map((c, i) => {
          const can = !c.reverted && c.revert.length;
          return `<div class="change ${c.reverted ? 'done' : ''}"><div class="body">
            <b>${esc(c.title)}${c.ok ? '' : ' <span class="badge high">с ошибками</span>'}</b>
            ${c.revert.length ? `<code>↺ ${esc(c.revert[0])}${c.revert.length > 1 ? ` (+${c.revert.length - 1})` : ''}</code>` : `<span class="label">${esc(c.note || 'Откат не требуется')}</span>`}</div>
            ${c.reverted ? '<span class="label" style="font-weight:700">Отменено</span>'
              : can && cur.serial === S.serial && device() ? `<button class="btn sm ghost" data-act="revert-one" data-id="${cur.id}" data-i="${i}">Отменить</button>` : ''}
          </div>`;
        }).join('')}
      </section>
    </div>`;
}

// ───────────────────────── приложения ─────────────────────────
async function loadApps() {
  if (!device()) return;
  S.appsLoading = true;
  render();
  try {
    S.apps = await call('list_apps', { serial: S.serial });
  } catch (e) {
    toast(errText(e), 'err');
  } finally {
    S.appsLoading = false;
    render();
  }
}

const AV = ['#5AB2FF', '#3DDC97', '#F5B544', '#B69CFF', '#FF8A7A', '#7FD4E8'];
function avatarColor(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return AV[h % AV.length];
}

function appTabs() {
  const a = S.apps || [];
  return [
    ['all', 'Все', a.filter((x) => x.installed).length],
    ['user', 'Пользовательские', a.filter((x) => x.installed && !x.system).length],
    ['system', 'Системные', a.filter((x) => x.installed && x.system).length],
    ['off', 'Отключённые', a.filter((x) => x.installed && !x.enabled).length],
    ['removed', 'Удалённые', a.filter((x) => !x.installed).length],
  ];
}

function filteredApps() {
  const q = S.appsQuery.trim().toLowerCase();
  return (S.apps || []).filter((x) => {
    switch (S.appsTab) {
      case 'all': if (!x.installed) return false; break;
      case 'user': if (!x.installed || x.system) return false; break;
      case 'system': if (!x.installed || !x.system) return false; break;
      case 'off': if (!x.installed || x.enabled) return false; break;
      case 'removed': if (x.installed) return false; break;
    }
    return !q || x.package.toLowerCase().includes(q) || (x.label || '').toLowerCase().includes(q);
  });
}

function viewApps() {
  const head = `<header class="head"><div class="tt"><h1>Приложения</h1><p class="sub">Ручной режим: управляйте каждым пакетом сами. Действия попадают в историю.</p></div>
    <div style="display:flex;gap:10px">${device() ? `<button class="btn" data-act="reload-apps">${icon('refresh', 16)}Обновить</button>
    <button class="btn primary" data-act="install">${icon('download', 16)}Установить APK</button>` : ''}</div></header>`;
  if (!device()) return head + `<div class="card empty"><h2>Телефон не подключён</h2><p class="sub">Подключите телефон, чтобы увидеть список приложений.</p></div>`;
  if (!S.apps) return head + `<div class="card" style="display:flex;gap:14px;align-items:center"><div class="spinner"></div>Загружаю список приложений…</div>`;
  return head + `
    <div style="display:flex;flex-wrap:wrap;gap:12px;align-items:center">
      <label class="search">${icon('search', 16)}<span class="sr">Поиск</span>
        <input type="search" id="apps-q" placeholder="Поиск по названию или пакету" value="${esc(S.appsQuery)}" aria-label="Поиск по названию или пакету"></label>
      <div class="pills" role="tablist" aria-label="Фильтр">
        ${appTabs().map(([id, l, n]) => `<button class="pill" role="tab" aria-selected="${S.appsTab === id}" data-act="apps-tab" data-id="${id}">${l}<span class="n">${n}</span></button>`).join('')}
      </div>
    </div>
    <div class="table-wrap"><table>
      <thead><tr><th scope="col">Приложение</th><th scope="col">Тип</th><th scope="col">Статус</th><th scope="col">Действия</th></tr></thead>
      <tbody id="apps-tbody"></tbody></table></div>
    <p class="label" style="font-size:13px;margin:0">Перетащите файлы .apk в окно, чтобы установить. Названия показываются для известных AvG приложений, у остальных — имя пакета.</p>`;
}

function renderAppRows() {
  const tb = $('#apps-tbody');
  if (!tb) return;
  const list = filteredApps();
  const MAX = 400;
  tb.innerHTML = list.slice(0, MAX).map((x) => {
    const name = x.label || x.package.split('.').pop();
    const status = !x.installed ? ['muted', 'Удалено'] : !x.enabled ? ['muted', 'Отключено'] : ['safe', 'Включено'];
    const p = esc(x.package);
    let actions;
    if (!x.installed) actions = `<button class="btn sm ghost" data-act="pkg" data-a="restore" data-p="${p}">Восстановить</button>`;
    else actions = `
      ${x.critical ? '<span class="badge medium" title="Без этого пакета телефон может не работать">Важное</span>'
        : `<button class="btn sm ghost" data-act="pkg" data-a="${x.enabled ? 'disable' : 'enable'}" data-p="${p}">${x.enabled ? 'Отключить' : 'Включить'}</button>`}
      <button class="btn sm ghost icon" aria-label="Сохранить APK" title="Сохранить APK на компьютер" data-act="pull" data-p="${p}">${icon('download', 16)}</button>
      ${x.critical ? '' : `<button class="btn sm danger icon" aria-label="Удалить" title="Удалить" data-act="pkg" data-a="uninstall" data-p="${p}">${icon('trash', 16)}</button>`}`;
    return `<tr><td><div class="app-cell"><span class="avatar" aria-hidden="true" style="background:${avatarColor(x.package)}">${esc(name[0]?.toUpperCase() || '?')}</span>
      <span class="n"><b>${esc(name)}</b><code class="selectable">${p}</code></span></div></td>
      <td>${x.system ? 'Системное' : 'Пользовательское'}</td><td><span class="badge ${status[0]}">${status[1]}</span></td>
      <td><span class="actions">${actions}</span></td></tr>`;
  }).join('') + (list.length > MAX ? `<tr><td colspan="4" class="label">Показано ${MAX} из ${list.length}. Уточните поиск.</td></tr>` : '')
    + (!list.length ? `<tr><td colspan="4" class="label">Ничего не найдено</td></tr>` : '');
  hydrateIcons(tb);
}

async function packageAction(pkg, action) {
  const app = S.apps.find((x) => x.package === pkg);
  const name = app?.label || pkg;
  if (action === 'uninstall') {
    const text = app.system
      ? `«${name}» будет удалено для пользователя. Сам APK останется в системе, вернуть можно здесь же или в «Истории и откате».`
      : `«${name}» будет удалено полностью вместе с данными. Вернуть можно только повторной установкой — при необходимости сначала сохраните APK.`;
    if (!(await confirmBox({ title: 'Удалить приложение?', text, ok: 'Удалить', danger: true }))) return;
  }
  if (action === 'disable' && !(await confirmBox({ title: 'Отключить приложение?', text: `«${name}» перестанет запускаться и пропадёт из меню. Включить обратно можно в любой момент.`, ok: 'Отключить' }))) return;
  try {
    await call('app_action', { serial: S.serial, package: pkg, action, system: !!app?.system });
    toast({ disable: 'Отключено', enable: 'Включено', uninstall: 'Удалено', restore: 'Восстановлено' }[action] + `: ${name}`, 'ok');
  } catch (e) {
    toast(errText(e), 'err');
  }
  await loadApps();
}

async function installApks(paths) {
  if (!device()) return toast('Сначала подключите телефон', 'err');
  for (const p of paths) {
    const name = p.split(/[\\/]/).pop();
    if (!/\.apk$/i.test(name)) { toast(`${name}: поддерживаются только .apk`, 'err'); continue; }
    toast(`Устанавливаю ${name}…`);
    try {
      await call('install_apk', { serial: S.serial, path: p });
      toast(`Установлено: ${name}`, 'ok');
    } catch (e) {
      toast(`${name}: ${errText(e)}`, 'err');
    }
  }
  if (S.view === 'apps') loadApps();
}

// ───────────────────────── консоль ─────────────────────────
const QUICK = ['getprop ro.product.model', 'dumpsys battery', 'pm list packages -3', 'df -h /data', 'settings list global', 'wm size'];

function viewConsole() {
  const head = `<header class="head"><div class="tt"><h1>Консоль ADB</h1><p class="sub">Команды выполняются как <code>adb shell …</code>. Будьте осторожны: здесь AvG ничего не проверяет и не сохраняет для отката.</p></div>
    ${S.consoleLog.length ? '<button class="btn ghost" data-act="clear-console">Очистить</button>' : ''}</header>`;
  if (!device()) return head + `<div class="card empty"><h2>Телефон не подключён</h2></div>`;
  return head + `
    <form class="console-input" id="console-form">
      <label class="search"><span style="font-family:var(--mono);color:var(--accent)">$</span>
        <input id="console-in" autocomplete="off" spellcheck="false" placeholder="например: pm list packages -s" aria-label="Команда"></label>
      <button class="btn primary" type="submit" ${S.consoleBusy ? 'disabled' : ''}>${icon('play', 16)}Выполнить</button>
    </form>
    <div class="chips">${QUICK.map((q) => `<button class="pill" data-act="quick" data-cmd="${esc(q)}" style="font-family:var(--mono);font-size:12px;font-weight:500">${esc(q)}</button>`).join('')}</div>
    <section class="log"><div class="log-body" style="max-height:none;min-height:360px">${S.consoleLog.length ? S.consoleLog.map((l) => `<div class="cmd">$ ${esc(l.cmd)}</div><div class="${l.err ? 'err' : ''}">${esc(l.out || (l.pending ? '…' : '(пусто)'))}</div>`).join('') : '<div class="cmd">Вывод команд появится здесь.</div>'}</div></section>`;
}

async function runConsole(cmd) {
  cmd = cmd.trim();
  if (!cmd || S.consoleBusy) return;
  const entry = { cmd, out: '', pending: true };
  S.consoleLog.push(entry);
  S.consoleBusy = true;
  render();
  try {
    const r = await call('run_shell', { serial: S.serial, command: cmd });
    entry.out = [r.stdout.trimEnd(), r.stderr.trimEnd()].filter(Boolean).join('\n');
    entry.err = r.code !== 0;
    if (r.code !== 0) entry.out += `\n[код выхода ${r.code}]`;
  } catch (e) {
    entry.out = errText(e); entry.err = true;
  }
  entry.pending = false;
  S.consoleBusy = false;
  render();
  requestAnimationFrame(() => $('#console-in')?.focus());
}

// ───────────────────────── настройки ─────────────────────────
async function loadInfo() {
  try { S.info = await call('app_info'); } catch (e) { toast(errText(e), 'err'); }
  render();
}

function viewSettings() {
  const i = S.info;
  const head = `<header class="head"><div class="tt"><h1>Настройки</h1><p class="sub">Путь к adb, папки программы и информация о версии.</p></div></header>`;
  if (!i) return head + `<div class="card" style="display:flex;gap:14px;align-items:center"><div class="spinner"></div>Загрузка…</div>`;
  return head + `
    <section class="card" style="display:flex;flex-direction:column;gap:18px">
      <h2>ADB</h2>
      <dl class="kv"><dt>Используется</dt><dd class="selectable">${i.adb_path ? esc(i.adb_path) : '<span style="color:var(--high)">не найден</span>'}</dd>
      <dt>Версия</dt><dd>${esc(i.adb_version || '—')}</dd></dl>
      <div class="field"><span class="label">Свой путь к adb (пусто — искать автоматически: рядом с AvG, в PATH, в Android SDK)</span>
        <input id="adb-path" value="${esc(i.custom_adb_path || '')}" placeholder="C:\\platform-tools\\adb.exe" spellcheck="false"></div>
      <div style="display:flex;gap:10px;flex-wrap:wrap">
        ${isTauri ? `<button class="btn" data-act="pick-adb">${icon('folder', 16)}Выбрать файл…</button>` : ''}
        <button class="btn primary" data-act="save-adb">Сохранить</button>
        <button class="btn ghost" data-act="reset-adb">Искать автоматически</button>
      </div>
    </section>
    <section class="card" style="display:flex;flex-direction:column;gap:14px">
      <h2>Твики</h2>
      <p class="sub">Загружено ${i.tweaks_count} ${plural(i.tweaks_count, ['твик', 'твика', 'твиков'])}. Свои наборы в формате TOML можно положить в папку ниже — они подхватятся при следующем запуске (формат описан в docs/TWEAKS.md).</p>
      <dl class="kv"><dt>Папка своих твиков</dt><dd class="selectable mono">${esc(i.tweaks_dir)}</dd>
      <dt>Данные и точки отката</dt><dd class="selectable mono">${esc(i.data_dir)}</dd></dl>
      ${i.tweak_warnings.length ? `<div class="log-body" style="min-height:0;border:1px solid var(--danger-line);border-radius:12px">${i.tweak_warnings.map((w) => `<div class="err">${esc(w)}</div>`).join('')}</div>` : ''}
    </section>
    <section class="card" style="display:flex;flex-direction:column;gap:10px">
      <h2>О программе</h2>
      <p class="sub">AvG ${esc(i.version)} — свободная программа под лицензией GNU GPL v3.0 или новее. Исходный код: github.com/DezFix/AvG</p>
      <p class="sub">Программа поставляется без каких-либо гарантий. Все изменения обратимы через «Историю и откат», но используйте на свой риск.</p>
    </section>`;
}

async function saveAdbPath(path) {
  try {
    S.info = await call('set_adb_path', { path });
    toast(S.info.adb_path ? `adb найден: ${S.info.adb_path}` : 'adb не найден по этому пути', S.info.adb_path ? 'ok' : 'err');
  } catch (e) { toast(errText(e), 'err'); }
  await pollDevices();
  render();
}

// ───────────────────────── обработчики ─────────────────────────
view.addEventListener('click', async (e) => {
  const el = e.target.closest('[data-act]');
  if (!el || el.disabled) return;
  const act = el.dataset.act;
  switch (act) {
    case 'go': e.preventDefault(); go(el.dataset.v); break;
    case 'rescan': S.plan = null; scanDevice(); break;
    case 'profile': S.profile = el.dataset.id; S.plan = null; render(); break;
    case 'open-plan': go('plan'); loadPlan(); break;
    case 'optimize': {
      const plan = await call('get_plan', { serial: S.serial, profile: S.profile }).catch((err) => { toast(errText(err), 'err'); return null; });
      if (!plan) return;
      const items = plan.filter((i) => i.selected);
      if (!items.length) { toast('Нет изменений, выбранных по умолчанию. Откройте план и отметьте нужные.'); go('plan'); loadPlan(); return; }
      const extra = plan.length - items.length;
      const ok = await confirmBox({
        title: `Применить ${items.length} ${plural(items.length, ['изменение', 'изменения', 'изменений'])}?`,
        text: `AvG применит рекомендованные изменения профиля${extra ? ` (ещё ${extra} не ${plural(extra, ['выбрано', 'выбраны', 'выбрано'])} по умолчанию — их можно отметить в плане)` : ''}. Перед началом будет создана точка отката.`,
        ok: 'Оптимизировать',
      });
      if (ok) startApply(items);
      break;
    }
    case 'select-safe':
      S.selected = new Set(S.plan.filter((i) => i.risk === 'safe').map((i) => i.id));
      render();
      break;
    case 'apply': startApply(S.plan.filter((i) => S.selected.has(i.id))); break;
    case 'cancel':
      S.apply.cancelling = true;
      await call('cancel_apply');
      render();
      break;
    case 'copy-log':
      await navigator.clipboard.writeText(S.apply.log.map((l) => l.text).join('\n')).then(() => toast('Журнал скопирован'), () => toast('Не удалось скопировать', 'err'));
      break;
    case 'snap': S.snapSel = Number(el.dataset.id); render(); break;
    case 'revert-one':
      try {
        const r = await call('revert_change', { serial: S.serial, snapshot: Number(el.dataset.id), index: Number(el.dataset.i) });
        showRevertResult(r, 'Откат');
      } catch (err) { toast(errText(err), 'err'); }
      await afterRevert();
      break;
    case 'revert-snap':
      if (await confirmBox({ title: 'Откатить всю точку?', text: 'Все изменения этой точки будут отменены в обратном порядке.', ok: 'Откатить' })) await revertSnapshot(Number(el.dataset.id));
      break;
    case 'revert-all':
      if (await confirmBox({ title: 'Вернуть всё как было до AvG?', text: 'Будут отменены все изменения, которые AvG делал на этом телефоне, начиная с последних.', ok: 'Вернуть всё', danger: true })) {
        try { showRevertResult(await call('revert_all', { serial: S.serial }), 'Полный откат'); } catch (err) { toast(errText(err), 'err'); }
        await afterRevert();
      }
      break;
    case 'reload-apps': S.apps = null; loadApps(); break;
    case 'apps-tab': S.appsTab = el.dataset.id; render(); break;
    case 'pkg': packageAction(el.dataset.p, el.dataset.a); break;
    case 'pull': {
      const pkg = el.dataset.p;
      const dest = await saveApk(`${pkg}.apk`);
      if (!dest) return;
      try { await call('pull_apk', { serial: S.serial, package: pkg, dest }); toast(`APK сохранён: ${dest}`, 'ok'); } catch (err) { toast(errText(err), 'err'); }
      break;
    }
    case 'install': { const files = await pickApks(); if (files) installApks(files); break; }
    case 'quick': runConsole(el.dataset.cmd); break;
    case 'clear-console': S.consoleLog = []; render(); break;
    case 'pick-adb': {
      const p = await pickAdb();
      if (p) { if (S.view === 'settings') $('#adb-path').value = p; await saveAdbPath(p); }
      else if (!isTauri) go('settings');
      break;
    }
    case 'save-adb': saveAdbPath($('#adb-path').value.trim() || null); break;
    case 'reset-adb': saveAdbPath(null); break;
  }
});

view.addEventListener('change', (e) => {
  const id = e.target.dataset?.check;
  if (id) {
    if (e.target.checked) S.selected.add(id); else S.selected.delete(id);
    render();
  }
});

view.addEventListener('input', (e) => {
  if (e.target.id === 'apps-q') {
    S.appsQuery = e.target.value;
    renderAppRows();
  }
});

view.addEventListener('submit', (e) => {
  if (e.target.id === 'console-form') {
    e.preventDefault();
    const inp = $('#console-in');
    runConsole(inp.value);
    inp.value = '';
  }
});

document.querySelectorAll('.nav-item').forEach((a) => a.addEventListener('click', (e) => {
  e.preventDefault();
  if (S.busy && a.dataset.view !== 'main') toast('Дождитесь окончания оптимизации — можно смотреть, но не менять');
  go(S.busy && a.dataset.view === 'main' ? 'apply' : a.dataset.view);
}));

// ───────────────────────── старт ─────────────────────────
hydrateIcons(document);
listen('avg://apply', onApplyEvent);

let dragDepth = false;
onDragDrop((p) => {
  const ov = $('#drop-overlay');
  if (p.type === 'enter' || p.type === 'over') { if (!dragDepth) { dragDepth = true; ov.hidden = false; } }
  else if (p.type === 'leave') { dragDepth = false; ov.hidden = true; }
  else if (p.type === 'drop') { dragDepth = false; ov.hidden = true; if (p.paths?.length) installApks(p.paths); }
});

const start = location.hash.slice(1);
go(['main', 'apps', 'console', 'history', 'settings'].includes(start) ? start : 'main');
pollDevices();
setInterval(pollDevices, 2500);
