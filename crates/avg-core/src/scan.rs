//! Сканирование телефона: модель, прошивка, память, батарея, пакеты и
//! текущие значения настроек, нужные для построения плана.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::adb::Shell;
use crate::tweaks::{Action, Tweak};
use crate::{is_safe_token, Result};

const MARK: &str = "__AVG_SECTION__";

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceInfo {
    pub manufacturer: String,
    pub brand: String,
    pub model: String,
    pub codename: String,
    /// «Xiaomi Redmi Note 8»
    pub display_name: String,
    /// `xiaomi`, `samsung` или производитель в нижнем регистре.
    pub vendor: String,
    pub android: String,
    pub sdk: u32,
    /// «MIUI 12.5», «HyperOS 1.0», «One UI 5.1» или пусто.
    pub rom: String,
    pub ram_total_kb: u64,
    pub ram_available_kb: u64,
    pub storage_total_kb: u64,
    pub storage_used_kb: u64,
    pub battery_level: Option<u32>,
    pub battery_temp_c: Option<f32>,
    pub packages_total: usize,
    pub packages_system: usize,
    pub packages_disabled: usize,
}

/// Полный результат сканирования.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub info: DeviceInfo,
    /// Пакеты, установленные для пользователя 0.
    pub installed: HashSet<String>,
    pub system: HashSet<String>,
    pub disabled: HashSet<String>,
    /// Текущие значения настроек: `global/key` → значение (`null`, если не задано).
    pub settings: HashMap<String, String>,
    /// Разрешение уже изменено командой `wm size`.
    pub wm_overridden: bool,
}

/// Собирает всё одной shell-командой, чтобы не гонять adb десятки раз.
pub fn scan(dev: &dyn Shell, tweaks: &[Tweak]) -> Result<Scan> {
    let setting_ids = setting_ids(tweaks);
    let mut parts = vec![
        "getprop".to_string(),
        "cat /proc/meminfo".to_string(),
        "df /data".to_string(),
        "dumpsys battery".to_string(),
        "pm list packages".to_string(),
        "pm list packages -s".to_string(),
        "pm list packages -d".to_string(),
        "wm size".to_string(),
    ];
    for id in &setting_ids {
        let (ns, key) = id.split_once('/').unwrap_or_default();
        parts.push(format!("settings get {ns} {key}"));
    }
    let script = parts.join(&format!("; echo {MARK}; "));
    let out = dev.shell(&script)?;
    if out.stdout.trim().is_empty() {
        return Err(crate::Error::Adb(if out.text().is_empty() {
            "устройство не ответило".into()
        } else {
            out.text()
        }));
    }
    let sections: Vec<&str> = out.stdout.split(MARK).map(|s| s.trim_matches('\n')).collect();
    let get = |i: usize| sections.get(i).copied().unwrap_or_default();

    let props = parse_getprop(get(0));
    let (ram_total, ram_avail) = parse_meminfo(get(1));
    let (st_total, st_used) = parse_df(get(2));
    let (level, temp) = parse_battery(get(3));
    let installed = parse_packages(get(4));
    let system = parse_packages(get(5));
    let disabled = parse_packages(get(6));
    let wm_overridden = get(7).contains("Override size");

    let mut settings = HashMap::new();
    for (i, id) in setting_ids.iter().enumerate() {
        settings.insert(id.clone(), get(8 + i).trim().to_string());
    }

    let mut info = device_info(&props);
    info.ram_total_kb = ram_total;
    info.ram_available_kb = ram_avail;
    info.storage_total_kb = st_total;
    info.storage_used_kb = st_used;
    info.battery_level = level;
    info.battery_temp_c = temp;
    info.packages_total = installed.len();
    info.packages_system = system.intersection(&installed).count();
    info.packages_disabled = disabled.len();

    Ok(Scan { info, installed, system, disabled, settings, wm_overridden })
}

/// Все настройки, которые упоминаются в твиках.
fn setting_ids(tweaks: &[Tweak]) -> Vec<String> {
    let mut ids: Vec<String> = tweaks
        .iter()
        .filter_map(|t| match &t.action {
            Action::Settings { values } => Some(values.iter().map(|v| v.id()).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .filter(|id| id.split('/').all(is_safe_token))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

pub fn parse_getprop(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        // [ro.product.model]: [Redmi Note 8]
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            if let Some((k, v)) = rest.split_once("]: [") {
                m.insert(k.to_string(), v.trim_end_matches(']').to_string());
            }
        }
    }
    m
}

pub fn device_info(p: &HashMap<String, String>) -> DeviceInfo {
    let g = |k: &str| p.get(k).map(|s| s.trim().to_string()).unwrap_or_default();
    let manufacturer = g("ro.product.manufacturer");
    let brand = g("ro.product.brand");
    let model = g("ro.product.model");
    let display_name = if manufacturer.is_empty()
        || model.to_lowercase().starts_with(&manufacturer.to_lowercase())
    {
        model.clone()
    } else {
        format!("{} {}", capitalize(&manufacturer), model)
    };
    DeviceInfo {
        vendor: detect_vendor(&manufacturer, &brand),
        android: g("ro.build.version.release"),
        sdk: g("ro.build.version.sdk").parse().unwrap_or(0),
        rom: detect_rom(p),
        codename: g("ro.product.device"),
        manufacturer,
        brand,
        model,
        display_name,
        ..Default::default()
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

pub fn detect_vendor(manufacturer: &str, brand: &str) -> String {
    let s = format!("{} {}", manufacturer, brand).to_lowercase();
    if ["xiaomi", "redmi", "poco"].iter().any(|v| s.contains(v)) {
        "xiaomi".into()
    } else if s.contains("samsung") {
        "samsung".into()
    } else {
        manufacturer.to_lowercase()
    }
}

pub fn detect_rom(p: &HashMap<String, String>) -> String {
    let non_empty = |k: &str| p.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
    if let Some(v) = non_empty("ro.mi.os.version.name") {
        return format!("HyperOS {}", v.trim_start_matches("OS"));
    }
    if let Some(v) = non_empty("ro.miui.ui.version.name") {
        let d = v.trim_start_matches('V');
        let ver = if d.len() == 3 && d.chars().all(|c| c.is_ascii_digit()) {
            format!("{}.{}", &d[..2], &d[2..])
        } else {
            d.to_string()
        };
        return format!("MIUI {ver}");
    }
    if let Some(v) = non_empty("ro.build.version.oneui").and_then(|v| v.parse::<u32>().ok()) {
        return format!("One UI {}.{}", v / 10000, (v / 100) % 100);
    }
    if let Some(v) = non_empty("ro.build.version.opporom") {
        return format!("ColorOS {}", v.trim_start_matches('V'));
    }
    String::new()
}

/// (MemTotal, MemAvailable) в КБ.
pub fn parse_meminfo(text: &str) -> (u64, u64) {
    let field = |name: &str| {
        text.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (field("MemTotal:"), field("MemAvailable:"))
}

/// (всего, занято) в КБ из `df /data`.
pub fn parse_df(text: &str) -> (u64, u64) {
    // Filesystem 1K-blocks Used Available Use% Mounted on
    // /dev/block/dm-6 54007028 41321568 12554388 77% /data
    let line = text
        .lines()
        .rev()
        .find(|l| l.split_whitespace().count() >= 4 && !l.starts_with("Filesystem"));
    let Some(line) = line else { return (0, 0) };
    let cols: Vec<&str> = line.split_whitespace().collect();
    let total = cols.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    let used = cols.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
    (total, used)
}

/// (уровень %, температура °C) из `dumpsys battery`.
pub fn parse_battery(text: &str) -> (Option<u32>, Option<f32>) {
    let field = |name: &str| {
        text.lines()
            .map(str::trim)
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse::<i64>().ok())
    };
    (
        field("level:").map(|v| v as u32),
        field("temperature:").map(|v| v as f32 / 10.0),
    )
}

pub fn parse_packages(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("package:"))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn getprop_and_rom() {
        let p = parse_getprop(
            "[ro.product.manufacturer]: [Xiaomi]\n[ro.product.brand]: [Redmi]\n[ro.product.model]: [Redmi Note 8]\n[ro.miui.ui.version.name]: [V125]\n[ro.build.version.sdk]: [30]\n[ro.build.version.release]: [11]\n",
        );
        let i = device_info(&p);
        assert_eq!(i.display_name, "Xiaomi Redmi Note 8");
        assert_eq!(i.vendor, "xiaomi");
        assert_eq!(i.rom, "MIUI 12.5");
        assert_eq!(i.sdk, 30);

        let mut s = HashMap::new();
        s.insert("ro.build.version.oneui".to_string(), "50100".to_string());
        assert_eq!(detect_rom(&s), "One UI 5.1");
        s.clear();
        s.insert("ro.mi.os.version.name".to_string(), "OS1.0".to_string());
        assert_eq!(detect_rom(&s), "HyperOS 1.0");
    }

    #[test]
    fn samsung_name_not_duplicated() {
        let p = parse_getprop("[ro.product.manufacturer]: [samsung]\n[ro.product.model]: [SM-A525F]\n");
        assert_eq!(device_info(&p).display_name, "Samsung SM-A525F");
    }

    #[test]
    fn meminfo_df_battery() {
        assert_eq!(
            parse_meminfo("MemTotal:        3794500 kB\nMemFree:  100 kB\nMemAvailable:     912000 kB\n"),
            (3794500, 912000)
        );
        assert_eq!(
            parse_df("Filesystem       1K-blocks     Used Available Use% Mounted on\n/dev/block/dm-6  54007028 41321568  12554388  77% /data\n"),
            (54007028, 41321568)
        );
        assert_eq!(
            parse_battery("Current Battery Service state:\n  AC powered: false\n  level: 64\n  temperature: 310\n"),
            (Some(64), Some(31.0))
        );
    }

    #[test]
    fn packages() {
        let s = parse_packages("package:com.a.b\npackage:com.c.d\n\n");
        assert_eq!(s.len(), 2);
        assert!(s.contains("com.c.d"));
    }
}
