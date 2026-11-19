//! Команды, которые вызывает интерфейс через `invoke`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use avg_core::adb::{DeviceEntry, Shell, ShellOutput};
use avg_core::discover::{self, Candidate};
use avg_core::engine::{self, LogLevel};
use avg_core::packages::{self, AppEntry, PackageAction};
use avg_core::plan::{self, CategorySummary, PlanItem, ProfileSummary};
use avg_core::scan::{self, DeviceInfo};
use avg_core::snapshot::Snapshot;
use avg_core::tweaks::profile_label;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

type St<'a> = State<'a, Arc<AppState>>;
type Res<T> = Result<T, String>;

const APPLY_EVENT: &str = "avg://apply";

/// Выполняет блокирующую работу (adb) вне потока интерфейса.
async fn blocking<T, F>(f: F) -> Res<T>
where
    T: Send + 'static,
    F: FnOnce() -> Res<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Не даёт запустить две длинные операции одновременно.
struct BusyGuard<'a>(&'a AtomicBool);

impl<'a> BusyGuard<'a> {
    fn take(flag: &'a AtomicBool) -> Res<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self(flag))
            .map_err(|_| "AvG уже выполняет операцию, дождитесь её окончания".to_string())
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn s<E: ToString>(e: E) -> String {
    e.to_string()
}

// ───────────────────────── программа и adb ─────────────────────────

#[derive(Serialize)]
pub struct AppInfo {
    version: String,
    adb_path: Option<String>,
    adb_version: Option<String>,
    /// Откуда adb: «Android SDK», «Nox», «Скачан AvG»…
    adb_source: Option<String>,
    adb_emulator: bool,
    adb_outdated: bool,
    custom_adb_path: Option<String>,
    data_dir: String,
    tweaks_dir: String,
    tweaks_count: usize,
    tweak_warnings: Vec<String>,
}

fn info(st: &AppState) -> AppInfo {
    let adb = st.adb().ok();
    let ver = adb.as_ref().and_then(|a| discover::probe_version(a.path()));
    let (source, emulator) = adb
        .as_ref()
        .map(|a| discover::classify(a.path(), Some(&st.data_dir)))
        .map_or((None, false), |(s, e)| (Some(s), e));
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        adb_path: adb.as_ref().map(|a| a.path().display().to_string()),
        adb_version: ver.map(|v| {
            if v.tools.0 > 0 {
                format!("{}.{}.{} (протокол 1.0.{})", v.tools.0, v.tools.1, v.tools.2, v.protocol)
            } else {
                format!("1.0.{}", v.protocol)
            }
        }),
        adb_source: source,
        adb_emulator: emulator,
        adb_outdated: ver.is_some_and(|v| v.protocol < 41),
        custom_adb_path: st.config.lock().unwrap().adb_path.clone(),
        data_dir: st.data_dir.display().to_string(),
        tweaks_dir: st.tweaks_dir.display().to_string(),
        tweaks_count: st.tweaks.len(),
        tweak_warnings: st.tweak_warnings.clone(),
    }
}

#[tauri::command]
pub async fn app_info(state: St<'_>) -> Res<AppInfo> {
    let st = state.inner().clone();
    blocking(move || Ok(info(&st))).await
}

#[tauri::command]
pub async fn set_adb_path(state: St<'_>, path: Option<String>) -> Res<AppInfo> {
    let st = state.inner().clone();
    blocking(move || {
        st.config.lock().unwrap().adb_path = path.filter(|p| !p.trim().is_empty());
        st.save_config()?;
        st.reset_adb();
        Ok(info(&st))
    })
    .await
}

#[derive(Serialize)]
pub struct DiscoverResult {
    candidates: Vec<Candidate>,
    /// Путь, который AvG выбрал сам (если adb до этого не был найден).
    applied: Option<String>,
    info: AppInfo,
}

/// Ищет adb на компьютере. `deep` — дополнительно обойти диски (до ~25 секунд).
/// Если рабочего adb ещё нет, сразу подключает лучший найденный.
#[tauri::command]
pub async fn discover_adb(state: St<'_>, deep: bool) -> Res<DiscoverResult> {
    let st = state.inner().clone();
    blocking(move || {
        let candidates = discover::discover(&st.search_dirs, Some(&st.data_dir), deep);
        let mut applied = None;
        if st.adb().is_err() {
            if let Some(best) = candidates.first() {
                st.config.lock().unwrap().adb_path = Some(best.path.clone());
                st.save_config()?;
                st.reset_adb();
                applied = Some(best.path.clone());
            }
        }
        Ok(DiscoverResult { candidates, applied, info: info(&st) })
    })
    .await
}

const DOWNLOAD_EVENT: &str = "avg://download";

/// Скачивает официальные Platform-Tools от Google в папку данных AvG.
#[tauri::command]
pub async fn download_adb(app: AppHandle, state: St<'_>) -> Res<AppInfo> {
    let st = state.inner().clone();
    blocking(move || {
        use std::io::Read;
        let _busy = BusyGuard::take(&st.busy)?;
        let emit = |v: serde_json::Value| {
            let _ = app.emit(DOWNLOAD_EVENT, v);
        };
        emit(json!({ "stage": "connect" }));
        let fail = |e: String| {
            format!("Не удалось скачать adb: {e}. Проверьте подключение к интернету или скачайте Platform-Tools вручную с developer.android.com")
        };
        let mut resp = ureq::get(discover::platform_tools_url()).call().map_err(|e| fail(e.to_string()))?;
        let total: Option<u64> = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let mut reader = resp.body_mut().as_reader();
        let mut data: Vec<u8> = Vec::with_capacity(total.unwrap_or(8 << 20) as usize);
        let mut chunk = vec![0u8; 64 * 1024];
        let mut last = 0usize;
        loop {
            let n = reader.read(&mut chunk).map_err(|e| fail(e.to_string()))?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&chunk[..n]);
            if data.len() > 300 << 20 {
                return Err(fail("файл слишком большой".into()));
            }
            if data.len() - last >= 256 * 1024 {
                last = data.len();
                emit(json!({ "stage": "download", "received": data.len(), "total": total }));
            }
        }
        emit(json!({ "stage": "extract" }));
        // Если уже работает ранее скачанный adb — останавливаем его, иначе файлы заняты.
        if let Ok(a) = st.adb() {
            if a.path().starts_with(&st.data_dir) {
                let _ = a.run(&["kill-server"]);
            }
        }
        st.reset_adb();
        let adb_path = discover::install_platform_tools(&data, &st.data_dir).map_err(s)?;
        st.config.lock().unwrap().adb_path = Some(adb_path.display().to_string());
        st.save_config()?;
        st.reset_adb();
        emit(json!({ "stage": "done" }));
        Ok(info(&st))
    })
    .await
}

#[tauri::command]
pub async fn list_devices(state: St<'_>) -> Res<Vec<DeviceEntry>> {
    let st = state.inner().clone();
    blocking(move || {
        let adb = st.adb()?;
        adb.devices().map_err(s)
    })
    .await
}

// ───────────────────────── сканирование и план ─────────────────────────

#[derive(Serialize)]
pub struct ScanResult {
    info: DeviceInfo,
    profiles: Vec<ProfileSummary>,
    /// Сводка «что нашёл сканер» для каждого профиля.
    categories: HashMap<String, Vec<CategorySummary>>,
}

fn do_scan(st: &AppState, serial: &str) -> Res<ScanResult> {
    let adb = st.adb()?;
    let scan = scan::scan(&adb.target(serial), &st.tweaks).map_err(s)?;
    let profiles = plan::profiles(&st.tweaks, &scan);
    let categories = profiles
        .iter()
        .map(|p| (p.id.clone(), plan::categories(&plan::build(&st.tweaks, &scan, &p.id))))
        .collect();
    let info = scan.info.clone();
    st.scans.lock().unwrap().insert(serial.to_string(), scan);
    Ok(ScanResult { info, profiles, categories })
}

#[tauri::command]
pub async fn scan_device(state: St<'_>, serial: String) -> Res<ScanResult> {
    let st = state.inner().clone();
    blocking(move || do_scan(&st, &serial)).await
}

#[tauri::command]
pub async fn get_plan(state: St<'_>, serial: String, profile: String) -> Res<Vec<PlanItem>> {
    let st = state.inner().clone();
    blocking(move || {
        let scans = st.scans.lock().unwrap();
        let scan = scans.get(&serial).ok_or("Сначала просканируйте телефон")?;
        Ok(plan::build(&st.tweaks, scan, &profile))
    })
    .await
}

// ───────────────────────── применение ─────────────────────────

#[tauri::command]
pub async fn apply_plan(
    app: AppHandle,
    state: St<'_>,
    serial: String,
    profile: String,
    ids: Vec<String>,
) -> Res<u64> {
    let st = state.inner().clone();
    blocking(move || {
        let _busy = BusyGuard::take(&st.busy)?;
        st.cancel.store(false, Ordering::SeqCst);
        let adb = st.adb()?;
        let target = adb.target(&serial);
        let emit = |v: serde_json::Value| {
            let _ = app.emit(APPLY_EVENT, v);
        };

        // Свежее состояние телефона: пользователь мог что-то поменять после сканирования.
        let scan = scan::scan(&target, &st.tweaks).map_err(s)?;
        let tweaks: Vec<_> = ids.iter().filter_map(|id| st.tweak(id)).collect();
        if tweaks.is_empty() {
            return Err("Ничего не выбрано".into());
        }

        let snap_id = st
            .store
            .lock()
            .unwrap()
            .create(&serial, &scan.info.display_name, &profile, profile_label(&profile))
            .map_err(s)?;
        let number = st.store.lock().unwrap().get(snap_id).map(|x| x.number).unwrap_or(0);
        emit(json!({ "kind": "start", "total": tweaks.len(), "snapshot": snap_id, "number": number }));

        let (mut ok, mut failed, mut cancelled) = (0, 0, false);
        for (i, t) in tweaks.iter().enumerate() {
            if st.cancel.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            emit(json!({ "kind": "step", "index": i, "title": t.title }));
            let mut log = |level: LogLevel, text: &str| {
                emit(json!({ "kind": "log", "level": level, "text": text }));
            };
            let change = engine::apply_tweak(&target, &scan, t, &mut log);
            if change.ok { ok += 1 } else { failed += 1 }
            emit(json!({ "kind": "step_done", "index": i, "ok": change.ok }));
            st.store.lock().unwrap().push(snap_id, change).map_err(s)?;
        }
        st.store.lock().unwrap().drop_if_empty(snap_id).map_err(s)?;
        let _ = do_scan(&st, &serial);
        emit(json!({ "kind": "done", "snapshot": snap_id, "ok": ok, "failed": failed, "cancelled": cancelled }));
        Ok(snap_id)
    })
    .await
}

#[tauri::command]
pub fn cancel_apply(state: St<'_>) {
    state.cancel.store(true, Ordering::SeqCst);
}

// ───────────────────────── откат ─────────────────────────

#[tauri::command]
pub async fn list_snapshots(state: St<'_>, serial: Option<String>) -> Res<Vec<Snapshot>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.store.lock().unwrap().list(serial.as_deref()))).await
}

#[derive(Serialize)]
pub struct RevertResult {
    ok: bool,
    reverted: usize,
    failed: usize,
    log: Vec<String>,
}

/// Откатывает выбранные изменения точки (в обратном порядке).
fn revert_in(st: &AppState, serial: &str, snap_id: u64, only: Option<usize>, out: &mut RevertResult) -> Res<()> {
    let snap = st.store.lock().unwrap().get(snap_id).map_err(s)?.clone();
    if snap.serial != serial {
        return Err(format!(
            "Эта точка отката сделана для другого телефона ({}). Подключите его.",
            snap.device
        ));
    }
    let adb = st.adb()?;
    let target = adb.target(serial);
    for (i, c) in snap.changes.iter().enumerate().rev() {
        if only.is_some_and(|o| o != i) || c.reverted || c.revert.is_empty() {
            continue;
        }
        let mut log = |level: LogLevel, text: &str| {
            let prefix = if level == LogLevel::Cmd { "$ " } else { "" };
            out.log.push(format!("{prefix}{text}"));
        };
        if engine::revert_change(&target, c, &mut log) {
            st.store.lock().unwrap().set_reverted(snap_id, i).map_err(s)?;
            out.reverted += 1;
        } else {
            out.failed += 1;
            out.ok = false;
        }
    }
    Ok(())
}

fn new_result() -> RevertResult {
    RevertResult { ok: true, reverted: 0, failed: 0, log: Vec::new() }
}

#[tauri::command]
pub async fn revert_change(state: St<'_>, serial: String, snapshot: u64, index: usize) -> Res<RevertResult> {
    let st = state.inner().clone();
    blocking(move || {
        let _busy = BusyGuard::take(&st.busy)?;
        let mut r = new_result();
        revert_in(&st, &serial, snapshot, Some(index), &mut r)?;
        let _ = do_scan(&st, &serial);
        Ok(r)
    })
    .await
}

#[tauri::command]
pub async fn revert_snapshot(state: St<'_>, serial: String, snapshot: u64) -> Res<RevertResult> {
    let st = state.inner().clone();
    blocking(move || {
        let _busy = BusyGuard::take(&st.busy)?;
        let mut r = new_result();
        revert_in(&st, &serial, snapshot, None, &mut r)?;
        let _ = do_scan(&st, &serial);
        Ok(r)
    })
    .await
}

#[tauri::command]
pub async fn revert_all(state: St<'_>, serial: String) -> Res<RevertResult> {
    let st = state.inner().clone();
    blocking(move || {
        let _busy = BusyGuard::take(&st.busy)?;
        let mut r = new_result();
        let ids: Vec<u64> = st.store.lock().unwrap().list(Some(&serial)).iter().map(|x| x.id).collect();
        for id in ids {
            revert_in(&st, &serial, id, None, &mut r)?;
        }
        let _ = do_scan(&st, &serial);
        Ok(r)
    })
    .await
}

// ───────────────────────── ручной режим ─────────────────────────

#[tauri::command]
pub async fn list_apps(state: St<'_>, serial: String) -> Res<Vec<AppEntry>> {
    let st = state.inner().clone();
    blocking(move || {
        let adb = st.adb()?;
        packages::list(&adb.target(&serial), &st.labels).map_err(s)
    })
    .await
}

fn sdk_of(st: &AppState, target: &dyn Shell, serial: &str) -> u32 {
    if let Some(sc) = st.scans.lock().unwrap().get(serial) {
        return sc.info.sdk;
    }
    target
        .shell("getprop ro.build.version.sdk")
        .ok()
        .and_then(|o| o.stdout.trim().parse().ok())
        .unwrap_or(0)
}

#[tauri::command]
pub async fn app_action(
    state: St<'_>,
    serial: String,
    package: String,
    action: PackageAction,
    system: bool,
) -> Res<String> {
    let st = state.inner().clone();
    blocking(move || {
        let _busy = BusyGuard::take(&st.busy)?;
        let adb = st.adb()?;
        let target = adb.target(&serial);
        let sdk = sdk_of(&st, &target, &serial);
        let label = st.labels.get(&package).cloned().unwrap_or_else(|| package.clone());
        let (out, change) = packages::apply(&target, sdk, &package, action, system, &label).map_err(s)?;
        if let Some(change) = change {
            let mut manual = st.manual.lock().unwrap();
            let mut store = st.store.lock().unwrap();
            let id = match manual.get(&serial).copied().filter(|id| store.get(*id).is_ok()) {
                Some(id) => id,
                None => {
                    let device = st
                        .scans
                        .lock()
                        .unwrap()
                        .get(&serial)
                        .map(|x| x.info.display_name.clone())
                        .unwrap_or_else(|| serial.clone());
                    let id = store.create(&serial, &device, "manual", "Ручной режим").map_err(s)?;
                    manual.insert(serial.clone(), id);
                    id
                }
            };
            store.push(id, change).map_err(s)?;
        }
        Ok(out.text())
    })
    .await
}

#[tauri::command]
pub async fn install_apk(state: St<'_>, serial: String, path: String) -> Res<String> {
    let st = state.inner().clone();
    blocking(move || {
        let _busy = BusyGuard::take(&st.busy)?;
        let p = PathBuf::from(&path);
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("apk")) {
            return Err("Поддерживаются только файлы .apk".into());
        }
        let out = st.adb()?.install(&serial, &p).map_err(s)?;
        if out.stdout.contains("Success") {
            Ok("Установлено".into())
        } else {
            Err(out.text())
        }
    })
    .await
}

#[tauri::command]
pub async fn pull_apk(state: St<'_>, serial: String, package: String, dest: String) -> Res<String> {
    let st = state.inner().clone();
    blocking(move || {
        let adb = st.adb()?;
        let remote = packages::apk_path(&adb.target(&serial), &package).map_err(s)?;
        let out = adb.pull(&serial, &remote, &PathBuf::from(&dest)).map_err(s)?;
        if out.ok() {
            Ok(dest)
        } else {
            Err(out.text())
        }
    })
    .await
}

#[tauri::command]
pub async fn run_shell(state: St<'_>, serial: String, command: String) -> Res<ShellOutput> {
    let st = state.inner().clone();
    blocking(move || {
        if command.trim().is_empty() {
            return Err("Пустая команда".into());
        }
        st.adb()?.shell(&serial, &command).map_err(s)
    })
    .await
}
