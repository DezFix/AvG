// Демо-режим: подделка Rust-части для просмотра интерфейса в обычном браузере
// (откройте ui/index.html через любой локальный сервер). В приложении не используется.

const listeners = {};
export async function listen(ev, cb) { (listeners[ev] ||= []).push(cb); }
const emit = (ev, p) => (listeners[ev] || []).forEach((cb) => cb(p));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const PLAN = [
  { id: 'xiaomi.msa', title: 'Рекламная служба MIUI (MSA)', description: 'Показывает рекламу в системных приложениях Xiaomi и постоянно работает в фоне.', category: 'bloat', risk: 'safe', selected: true, reversible: true, packages: ['com.miui.msa.global'], commands: ['pm uninstall -k --user 0 com.miui.msa.global'] },
  { id: 'partners.facebook', title: 'Скрытые службы Facebook', description: 'Предустановленные службы Facebook работают в фоне, даже если самого Facebook нет.', category: 'bloat', risk: 'safe', selected: true, reversible: true, packages: ['com.facebook.services', 'com.facebook.system', 'com.facebook.appmanager'], commands: ['pm uninstall -k --user 0 com.facebook.services', 'pm uninstall -k --user 0 com.facebook.system', 'pm uninstall -k --user 0 com.facebook.appmanager'] },
  { id: 'xiaomi.videoplayer', title: 'Mi Видео', description: 'Видеоплеер с рекламой.', category: 'bloat', risk: 'medium', selected: true, reversible: true, packages: ['com.miui.videoplayer'], commands: ['pm uninstall -k --user 0 com.miui.videoplayer'] },
  { id: 'xiaomi.browser', title: 'Mi Браузер', description: 'Удаляйте, только если у вас установлен другой браузер.', category: 'bloat', risk: 'medium', selected: false, reversible: true, packages: ['com.mi.globalbrowser'], commands: ['pm uninstall -k --user 0 com.mi.globalbrowser'] },
  { id: 'general.animations', title: 'Ускорить анимации в 2 раза', description: 'Интерфейс ощущается заметно быстрее на слабом железе.', category: 'ui', risk: 'safe', selected: true, reversible: true, packages: [], commands: ['settings put global window_animation_scale 0.5', 'settings put global transition_animation_scale 0.5', 'settings put global animator_duration_scale 0.5'] },
  { id: 'general.compile', title: 'Скомпилировать приложения', description: 'Приложения будут быстрее запускаться. Занимает 3–15 минут.', category: 'performance', risk: 'safe', selected: true, reversible: true, packages: [], commands: ['cmd package compile -m speed-profile -a'] },
  { id: 'general.screen_scale', title: 'Снизить разрешение экрана на 25%', description: 'Меньше нагрузка на видеочип, но картинка станет мягче.', category: 'performance', risk: 'high', selected: false, reversible: true, packages: [], commands: ['wm size ×0.75', 'wm density ×0.75'] },
  { id: 'general.wifi_scan', title: 'Отключить фоновый поиск Wi-Fi', description: 'Телефон перестанет постоянно сканировать сети.', category: 'battery', risk: 'safe', selected: true, reversible: true, packages: [], commands: ['settings put global wifi_scan_always_enabled 0'] },
  { id: 'general.private_dns', title: 'Блокировка рекламы через Private DNS', description: 'Включает AdGuard DNS: реклама режется во всех приложениях.', category: 'network', risk: 'safe', selected: true, reversible: true, packages: [], commands: ['settings put global private_dns_specifier dns.adguard-dns.com', 'settings put global private_dns_mode hostname'] },
];

// Условные иконки для демо: цветная плитка со штриховым значком.
const GLYPH = {
  chat: '<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/>',
  video: '<rect x="3" y="5" width="18" height="14" rx="3"/><path d="m10 9 5 3-5 3z"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a14 14 0 0 1 0 18a14 14 0 0 1 0-18z"/>',
  map: '<path d="M9 4 3 6v14l6-2 6 2 6-2V4l-6 2z"/><path d="M9 4v14"/><path d="M15 6v14"/>',
  music: '<path d="M9 18V5l11-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/>',
  card: '<rect x="3" y="6" width="18" height="13" rx="2"/><path d="M3 10h18"/><path d="M7 15h4"/>',
  book: '<path d="M4 4h10a4 4 0 0 1 4 4v12H8a4 4 0 0 1-4-4z"/><path d="M4 16a4 4 0 0 1 4-4h10"/>',
};
const ico = (g, c) => 'data:image/svg+xml;base64,' + btoa(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect width="48" height="48" rx="12" fill="${c}"/><g transform="translate(12 12)" fill="none" stroke="#0D1014" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">${GLYPH[g]}</g></svg>`);

const FILES = [
  { file_name: 'Telegram-11.5.3.apk', size: 81788928, format: 'apk', parts: 1, package: 'org.telegram.messenger', label: 'Telegram', version_code: 5135, version_name: '11.5.3', kind: 'update', installed_version: '11.4.2', icon: ico('chat', '#5AB2FF') },
  { file_name: 'firefox-133.0.apk', size: 100663296, format: 'apk', parts: 1, package: 'org.mozilla.firefox', label: 'Firefox', version_name: '133.0', kind: 'new', icon: ico('globe', '#FF8A7A') },
  { file_name: 'osmand-4.9.10.apks', size: 148897792, format: 'apks', parts: 4, package: 'net.osmand', label: 'OsmAnd', version_name: '4.9.10', kind: 'new', icon: ico('map', '#3DDC97') },
  { file_name: 'aimp-4.12.apk', size: 22020096, format: 'apk', parts: 1, package: 'com.aimp.player', label: 'AIMP', version_name: '4.12', kind: 'new', icon: ico('music', '#B69CFF') },
  { file_name: 'app-release (3).apk', size: 9437184, format: 'apk', parts: 1, package: 'com.example.notes', label: null, version_name: '2.1', kind: 'new', icon: null },
  { file_name: 'VLC-3.6.0-arm64.apk', size: 54525952, format: 'apk', parts: 1, package: 'org.videolan.vlc', label: 'VLC', version_name: '3.6.0', kind: 'same', installed_version: '3.6.0', icon: ico('video', '#F5B544') },
  { file_name: 'thunderbird-8.2.apk', size: 18874368, format: 'apk', parts: 1, package: 'net.thunderbird.android', label: 'Thunderbird', version_name: '8.2', kind: 'older', installed_version: '9.0', icon: ico('book', '#7FD4E8') },
  { file_name: 'old-racer-1.0.apk', size: 36700160, format: 'apk', parts: 1, package: 'com.example.racer', label: 'Old Racer', version_name: '1.0', kind: 'bad', note: 'Нужен Android 14, на телефоне 11', icon: null },
];

const snaps = [];
let stopInstall = false;
let n = 0;

// ?noadb — показать первый запуск без adb.
let hasAdb = !location.search.includes('noadb');

export async function invoke(cmd, a) {
  await wait(250);
  switch (cmd) {
    case 'discover_adb': await wait(a.deep ? 2500 : 300); return { candidates: [], applied: null, info: await invoke('app_info') };
    case 'download_adb': {
      const total = 7340032;
      for (let r = 0; r <= total; r += 917504) { emit('avg://download', { stage: 'download', received: r, total }); await wait(250); }
      emit('avg://download', { stage: 'extract' }); await wait(500);
      hasAdb = true;
      return invoke('app_info');
    }
    case 'list_devices':
      if (!hasAdb) throw 'adb не найден. Укажите путь к adb в настройках или положите папку platform-tools рядом с AvG';
      return [{ serial: '7c1a2f9e', state: 'device', model: 'Redmi Note 8', product: 'ginkgo', device: 'ginkgo', transport: 'usb' }];
    case 'scan_device': await wait(700); return {
      info: { display_name: 'Xiaomi Redmi Note 8', codename: 'ginkgo', android: '11', sdk: 30, rom: 'MIUI 12.5', vendor: 'xiaomi', ram_total_kb: 3794500, ram_available_kb: 943718, storage_total_kb: 54007028, storage_used_kb: 43321568, battery_level: 64, battery_temp_c: 31, packages_total: 312, packages_system: 241, packages_disabled: 9 },
      profiles: [{ id: 'weak', label: 'Слабый телефон', count: 9 }, { id: 'battery', label: 'Батарея', count: 5 }, { id: 'privacy', label: 'Приватность', count: 6 }, { id: 'custom', label: 'Свой набор', count: 14 }],
      categories: { weak: [{ category: 'bloat', count: 6, max_risk: 'medium' }, { category: 'ui', count: 1, max_risk: 'safe' }, { category: 'performance', count: 2, max_risk: 'high' }, { category: 'battery', count: 1, max_risk: 'safe' }, { category: 'network', count: 1, max_risk: 'safe' }] },
    };
    case 'get_plan': return PLAN;
    case 'apply_plan': {
      const items = PLAN.filter((p) => a.ids.includes(p.id));
      const s = { id: Date.now(), number: ++n, created: Date.now() / 1000, serial: a.serial, device: 'Xiaomi Redmi Note 8', profile: a.profile, profile_label: 'Слабый телефон', changes: [] };
      snaps.push(s);
      emit('avg://apply', { kind: 'start', total: items.length, snapshot: s.id, number: s.number });
      for (const [i, it] of items.entries()) {
        emit('avg://apply', { kind: 'step', index: i, title: it.title });
        for (const c of it.commands) { await wait(300); emit('avg://apply', { kind: 'log', level: 'cmd', text: c }); emit('avg://apply', { kind: 'log', level: 'ok', text: 'Success' }); }
        s.changes.push({ tweak_id: it.id, title: it.title, ok: true, revert: ['cmd package install-existing ' + (it.packages[0] || 'x')], reverted: false, note: '' });
        emit('avg://apply', { kind: 'step_done', index: i, ok: true });
      }
      emit('avg://apply', { kind: 'done', snapshot: s.id, ok: items.length, failed: 0, cancelled: false });
      return s.id;
    }
    case 'cancel_apply': return null;
    case 'list_snapshots': return [...snaps].reverse();
    case 'revert_snapshot': case 'revert_all': snaps.forEach((s) => s.changes.forEach((c) => (c.reverted = true))); return { ok: true, reverted: 3, failed: 0, log: [] };
    case 'revert_change': snaps.find((s) => s.id === a.snapshot).changes[a.index].reverted = true; return { ok: true, reverted: 1, failed: 0, log: [] };
    case 'list_apps': return [
      { package: 'org.telegram.messenger', label: 'Telegram', system: false, installed: true, enabled: true, critical: false, safety: 'safe', safety_note: 'Установлено вами или магазином. Удаление ничего в системе не сломает.' },
      { package: 'com.google.android.youtube', label: 'YouTube', system: true, installed: true, enabled: true, critical: false, safety: 'optional', safety_note: 'YouTube. Смотреть можно и в браузере.' },
      { package: 'com.miui.videoplayer', label: 'Mi Видео', system: true, installed: true, enabled: false, critical: false, safety: 'optional', safety_note: 'Есть в наборах AvG («Mi Видео»). Удалить можно, если не пользуетесь.' },
      { package: 'com.miui.msa.global', label: 'Рекламная служба MIUI (MSA)', system: true, installed: false, enabled: true, critical: false, safety: 'safe', safety_note: 'Есть в наборах AvG: известный мусор. Можно удалить.' },
      { package: 'com.android.systemui', label: 'Системный интерфейс', system: true, installed: true, enabled: true, critical: true, safety: 'critical', safety_note: 'Без этого пакета телефон может не загрузиться или перестать звонить.' },
      { package: 'android.auto_generated_characteristics_rro', label: null, system: true, installed: true, enabled: true, critical: false, safety: 'keep', safety_note: 'Оверлей: меняет оформление или настройки системы. Не работает в фоне и почти не занимает места — удаление ничего не ускорит, а интерфейс может сломаться.' },
      { package: 'android.frameworkres.overlay.display.product', label: null, system: true, installed: true, enabled: true, critical: false, safety: 'keep', safety_note: 'Оверлей: меняет оформление или настройки системы.' },
      { package: 'com.facebook.services', label: null, system: true, installed: true, enabled: true, critical: false, safety: 'safe', safety_note: 'Предустановленный мусор: реклама, аналитика или партнёрское приложение. Можно удалить.' },
      { package: 'ak.alizandro.smartaudiobookplayer', label: null, system: false, installed: true, enabled: true, critical: false, safety: 'safe', safety_note: 'Установлено вами или магазином. Удаление ничего в системе не сломает.' },
      { package: 'com.xiaomi.joyose', label: null, system: true, installed: true, enabled: true, critical: false, safety: 'unknown', safety_note: 'AvG не знает этот пакет. Если сомневаетесь — не удаляйте, а отключите: это легко вернуть.' },
    ];
    case 'app_action': return 'ok';
    case 'scan_apks': {
      await wait(500);
      const dir = a.folder || 'D:\\APK\\Мои приложения';
      const items = FILES.map((f) => ({ path: `${dir}\\${f.file_name}`, min_sdk: 24, abis: [], icon_bg: null, icon_adaptive: false, installed_version: null, note: '', ...f }));
      for (const p of a.files || []) {
        const name = p.split(/[\\/]/).pop();
        items.push({ path: p, file_name: name, size: 5242880, format: 'apk', parts: 1, package: 'com.dropped.' + name.replace(/\W/g, '').toLowerCase(), label: null, version_name: '1.0', kind: 'new', icon: null, abis: [], note: '' });
      }
      return { folder: dir, items, compared: hasAdb };
    }
    case 'install_queue': {
      stopInstall = false;
      emit('avg://install', { kind: 'start', total: a.items.length });
      let stopped = false;
      for (const [i, it] of a.items.entries()) {
        if (stopped || stopInstall) { stopped = true; emit('avg://install', { kind: 'item', index: i, status: 'skipped' }); continue; }
        emit('avg://install', { kind: 'item', index: i, status: 'running' });
        await wait(900);
        const fail = it.package === 'net.thunderbird.android' && !it.downgrade;
        emit('avg://install', fail
          ? { kind: 'item', index: i, status: 'fail', error: 'На телефоне версия новее. Включите «Разрешить ставить старую версию» (INSTALL_FAILED_VERSION_DOWNGRADE)' }
          : { kind: 'item', index: i, status: 'done' });
        if (fail && !a.keepGoing) stopped = true;
      }
      emit('avg://install', { kind: 'finish' });
      return null;
    }
    case 'stop_install': stopInstall = true; return null;
    case 'load_icons': {
      const known = { 'org.telegram.messenger': ['chat', '#5AB2FF'], 'com.google.android.youtube': ['video', '#FF8A7A'], 'com.miui.videoplayer': ['video', '#F5B544'], 'ru.sberbankmobile': ['card', '#3DDC97'], 'ak.alizandro.smartaudiobookplayer': ['book', '#B69CFF'] };
      (async () => {
        let done = 0;
        for (const p of a.packages) {
          await wait(250);
          done++;
          const k = known[p];
          if (k) emit('avg://icon', { kind: 'icon', package: p, label: p.startsWith('ak.') ? 'Smart AudioBook Player' : null, icon: ico(k[0], k[1]), bg: null, adaptive: false });
          emit('avg://icon', { kind: 'progress', done, total: a.packages.length });
        }
      })();
      return null;
    }
    case 'run_shell': return { stdout: 'Redmi Note 8\n', stderr: '', code: 0 };
    case 'app_info': return { version: '0.1.0', adb_path: hasAdb ? 'C:\\Users\\you\\AppData\\Roaming\\io.github.dezfix.avg\\platform-tools\\adb.exe' : null, adb_version: hasAdb ? '35.0.2 (протокол 1.0.41)' : null, adb_source: hasAdb ? 'Скачан AvG' : null, adb_emulator: false, adb_outdated: false, custom_adb_path: null, data_dir: 'C:\\Users\\you\\AppData\\Roaming\\io.github.dezfix.avg', tweaks_dir: 'C:\\Users\\you\\AppData\\Roaming\\io.github.dezfix.avg\\tweaks', tweaks_count: 39, tweak_warnings: [], apk_folder: 'D:\\APK\\Мои приложения' };
    case 'set_adb_path': return invoke('app_info');
    default: throw new Error('mock: ' + cmd);
  }
}
