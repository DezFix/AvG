use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use avg_core::adb::Adb;
use avg_core::scan::Scan;
use avg_core::snapshot::Store;
use avg_core::tweaks::{self, Tweak};
use avg_core::packages;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Путь к adb, выбранный пользователем.
    #[serde(default)]
    pub adb_path: Option<String>,
}

pub struct AppState {
    pub config_path: PathBuf,
    pub config: Mutex<Config>,
    pub adb: Mutex<Option<Adb>>,
    /// Где ещё искать adb: папка программы и ресурсы установщика.
    pub search_dirs: Vec<PathBuf>,
    pub data_dir: PathBuf,
    pub tweaks_dir: PathBuf,
    pub tweaks: Vec<Tweak>,
    pub tweak_warnings: Vec<String>,
    pub labels: HashMap<String, String>,
    pub rules: avg_core::safety::Rules,
    pub store: Mutex<Store>,
    /// Последнее сканирование каждого телефона.
    pub scans: Mutex<HashMap<String, Scan>>,
    pub busy: AtomicBool,
    pub cancel: AtomicBool,
    /// Точка отката ручного режима для каждого телефона в текущем запуске.
    pub manual: Mutex<HashMap<String, u64>>,
}

impl AppState {
    pub fn init(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = app.path().app_data_dir()?;
        let config_dir = app.path().app_config_dir()?;
        let tweaks_dir = data_dir.join("tweaks");
        std::fs::create_dir_all(&tweaks_dir)?;
        std::fs::create_dir_all(&config_dir)?;

        let config_path = config_dir.join("config.json");
        let config: Config = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();

        let (tweaks, tweak_warnings) = tweaks::load_with_user_dir(&tweaks_dir)?;
        let labels = packages::known_labels(&tweaks);
        let rules = avg_core::safety::Rules::new(&tweaks);

        let snap_path = data_dir.join("snapshots.json");
        let store = match Store::open(&snap_path) {
            Ok(s) => s,
            Err(_) => {
                // Повреждённый файл не удаляем, а откладываем в сторону.
                let _ = std::fs::rename(&snap_path, data_dir.join("snapshots.broken.json"));
                Store::open(&snap_path)?
            }
        };

        let mut search_dirs = Vec::new();
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
            search_dirs.push(dir);
        }
        if let Ok(res) = app.path().resource_dir() {
            search_dirs.push(res);
        }
        // Сюда кладёт adb кнопка «Скачать adb».
        search_dirs.insert(0, data_dir.clone());

        let state = Self {
            config_path,
            config: Mutex::new(config),
            adb: Mutex::new(None),
            search_dirs,
            data_dir,
            tweaks_dir,
            tweaks,
            tweak_warnings,
            labels,
            rules,
            store: Mutex::new(store),
            scans: Mutex::new(HashMap::new()),
            busy: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            manual: Mutex::new(HashMap::new()),
        };
        // Пробуем найти adb сразу, но не падаем, если его нет.
        let _ = state.adb();
        Ok(state)
    }

    /// Найденный adb (ищется заново, если раньше не нашёлся).
    pub fn adb(&self) -> Result<Adb, String> {
        let mut guard = self.adb.lock().unwrap();
        if let Some(a) = guard.as_ref() {
            return Ok(a.clone());
        }
        let custom = self.config.lock().unwrap().adb_path.clone();
        let found = Adb::locate(custom.as_deref().map(std::path::Path::new), &self.search_dirs)
            .map_err(|e| e.to_string())?;
        *guard = Some(found.clone());
        Ok(found)
    }

    pub fn reset_adb(&self) {
        *self.adb.lock().unwrap() = None;
    }

    pub fn save_config(&self) -> Result<(), String> {
        let cfg = self.config.lock().unwrap().clone();
        let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        std::fs::write(&self.config_path, text).map_err(|e| e.to_string())
    }

    pub fn tweak(&self, id: &str) -> Option<&Tweak> {
        self.tweaks.iter().find(|t| t.id == id)
    }
}
