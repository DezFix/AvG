// Связь интерфейса с Rust-частью. Вне Tauri (обычный браузер) подключается
// демо-режим из mock.js — удобно для работы над дизайном без телефона.

const T = window.__TAURI__;
export const isTauri = !!T;

let mockMod = null;
async function mock() {
  mockMod ??= await import('./mock.js');
  return mockMod;
}

export async function call(cmd, args = {}) {
  if (T) return T.core.invoke(cmd, args);
  return (await mock()).invoke(cmd, args);
}

export async function listen(event, cb) {
  if (T) return T.event.listen(event, (e) => cb(e.payload));
  return (await mock()).listen(event, cb);
}

export async function pickApks() {
  if (!T) return null;
  const r = await T.dialog.open({ multiple: true, filters: [{ name: 'Android-приложение', extensions: ['apk', 'apks', 'xapk', 'apkm'] }] });
  if (!r) return null;
  return Array.isArray(r) ? r : [r];
}

export async function pickFolder() {
  if (!T) return null;
  return T.dialog.open({ directory: true, multiple: false, title: 'Папка с вашими APK' });
}

export async function pickAdb() {
  if (!T) return null;
  return T.dialog.open({ multiple: false, directory: false, title: 'Выберите adb (adb.exe в папке platform-tools)' });
}

export async function saveApk(name) {
  if (!T) return null;
  return T.dialog.save({ defaultPath: name, filters: [{ name: 'Android-приложение', extensions: ['apk'] }] });
}

/** Перетаскивание файлов в окно. cb получает {type, paths}. */
export function onDragDrop(cb) {
  if (!T) return;
  try {
    T.webview.getCurrentWebview().onDragDropEvent((e) => cb(e.payload));
  } catch (err) {
    console.warn('drag-drop недоступен', err);
  }
}
