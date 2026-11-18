//! Команды, которые вызывает интерфейс через `invoke`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use avg_core::adb::{DeviceEntry, Shell, ShellOutput};
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
    custom_adb_path: Option<String>,
    data_dir: String,
    tweaks_dir: String,
    tweaks_count: usize,
    tweak_warnings: Vec<String>,
}

fn info(st: &AppState) -> AppInfo {
    let adb = st.adb().ok();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        adb_path: adb.as_ref().map(|a| a.path().display().to_string()),
        adb_version: adb.as_ref().and_then(|a| a.version().ok()),
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
