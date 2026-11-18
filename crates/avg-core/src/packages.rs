//! Ручной режим: список приложений и действия с ними.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::adb::{Shell, ShellOutput};
use crate::engine::{restore_command, Change};
use crate::scan::parse_packages;
use crate::tweaks::{Action, Tweak};
use crate::{is_valid_package, Error, Result};

const MARK: &str = "__AVG_SECTION__";

/// Пакеты, без которых телефон не загрузится или перестанет звонить.
/// Удалять и отключать их из AvG нельзя.
pub const CRITICAL: &[&str] = &[
    "android",
    "com.android.systemui",
    "com.android.settings",
    "com.android.phone",
    "com.android.shell",
    "com.android.packageinstaller",
    "com.google.android.packageinstaller",
    "com.android.permissioncontroller",
    "com.google.android.permissioncontroller",
    "com.android.providers.settings",
    "com.android.providers.telephony",
    "com.android.providers.contacts",
    "com.android.providers.media",
    "com.android.providers.downloads",
    "com.android.server.telecom",
    "com.android.inputdevices",
    "com.android.location.fused",
    "com.google.android.gms",
    "com.google.android.gsf",
    "com.android.vending",
    "com.miui.securitycenter",
    "com.miui.home",
    "com.lbe.security.miui",
    "com.xiaomi.finddevice",
    "com.sec.android.app.launcher",
    "com.samsung.android.providers.context",
];

pub fn is_critical(p: &str) -> bool {
    CRITICAL.contains(&p) || p.starts_with("com.android.providers.")
}

#[derive(Debug, Clone, Serialize)]
pub struct AppEntry {
    pub package: String,
    /// Понятное название, если оно известно AvG.
    pub label: Option<String>,
    pub system: bool,
    /// Установлено для пользователя (false = удалено через `--user 0`).
    pub installed: bool,
    pub enabled: bool,
    pub critical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageAction {
    Disable,
    Enable,
    Uninstall,
    Restore,
}

/// Список приложений, включая удалённые для пользователя системные.
pub fn list(dev: &dyn Shell, labels: &HashMap<String, String>) -> Result<Vec<AppEntry>> {
    let script = [
        "pm list packages -u",
        "pm list packages",
        "pm list packages -s",
        "pm list packages -d",
    ]
    .join(&format!("; echo {MARK}; "));
    let out = dev.shell(&script)?;
    let s: Vec<&str> = out.stdout.split(MARK).collect();
    let all = parse_packages(s.first().copied().unwrap_or_default());
    let installed = parse_packages(s.get(1).copied().unwrap_or_default());
    let system = parse_packages(s.get(2).copied().unwrap_or_default());
    let disabled = parse_packages(s.get(3).copied().unwrap_or_default());
    if all.is_empty() && installed.is_empty() {
        return Err(Error::Adb(out.text()));
    }
    let mut list: Vec<AppEntry> = all
        .union(&installed)
        .map(|p| AppEntry {
            package: p.clone(),
            label: labels.get(p).cloned(),
            system: system.contains(p) || !installed.contains(p),
            installed: installed.contains(p),
            enabled: !disabled.contains(p),
            critical: is_critical(p),
        })
        .collect();
    list.sort_by(|a, b| {
        let ka = a.label.clone().unwrap_or_else(|| a.package.clone()).to_lowercase();
        let kb = b.label.clone().unwrap_or_else(|| b.package.clone()).to_lowercase();
        ka.cmp(&kb)
    });
    Ok(list)
}

/// Выполняет действие и возвращает запись для истории (если его можно откатить).
pub fn apply(
    dev: &dyn Shell,
    sdk: u32,
    package: &str,
    action: PackageAction,
    system: bool,
    label: &str,
) -> Result<(ShellOutput, Option<Change>)> {
    if !is_valid_package(package) {
        return Err(Error::Other("недопустимое имя пакета".into()));
    }
    if is_critical(package) && matches!(action, PackageAction::Disable | PackageAction::Uninstall) {
        return Err(Error::Other(
            "это системно важный пакет — без него телефон может не загрузиться".into(),
        ));
    }
    let (cmd, revert, verb) = match action {
        PackageAction::Disable => (
            format!("pm disable-user --user 0 {package}"),
            vec![format!("pm enable {package}")],
            "Отключено",
        ),
        PackageAction::Enable => (format!("pm enable {package}"), vec![], "Включено"),
        PackageAction::Uninstall if system => (
            format!("pm uninstall -k --user 0 {package}"),
            vec![restore_command(sdk, package)],
            "Удалено",
        ),
        // Пользовательское приложение удаляется полностью — вернуть его можно только установкой APK.
        PackageAction::Uninstall => (format!("pm uninstall {package}"), vec![], "Удалено"),
        PackageAction::Restore => (restore_command(sdk, package), vec![], "Восстановлено"),
    };
    let out = dev.shell(&cmd)?;
    let success = match action {
        PackageAction::Uninstall => out.stdout.contains("Success"),
        PackageAction::Disable => out.text().contains("disabled"),
        _ => out.looks_ok(),
    };
    if !success {
        return Err(Error::Adb(if out.text().is_empty() { "команда не выполнена".into() } else { out.text() }));
    }
    let change = (!revert.is_empty()).then(|| Change {
        tweak_id: format!("manual.{package}"),
        title: format!("{verb}: {label}"),
        ok: true,
        revert,
        reverted: false,
        note: String::new(),
    });
    Ok((out, change))
}

/// Путь к основному APK пакета (`base.apk`).
pub fn apk_path(dev: &dyn Shell, package: &str) -> Result<String> {
    if !is_valid_package(package) {
        return Err(Error::Other("недопустимое имя пакета".into()));
    }
    let out = dev.shell(&format!("pm path {package}"))?;
    let paths: Vec<&str> = out
        .stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix("package:"))
        .collect();
    paths
        .iter()
        .find(|p| p.ends_with("/base.apk"))
        .or(paths.first())
        .map(|p| p.to_string())
        .ok_or_else(|| Error::Adb("APK не найден".into()))
}

/// Понятные названия: из твиков (где один пакет) плюс популярные приложения.
pub fn known_labels(tweaks: &[Tweak]) -> HashMap<String, String> {
    let mut m: HashMap<String, String> = [
        ("com.android.chrome", "Chrome"),
        ("com.android.vending", "Play Маркет"),
        ("com.google.android.gms", "Сервисы Google Play"),
        ("com.google.android.youtube", "YouTube"),
        ("com.google.android.gm", "Gmail"),
        ("com.google.android.apps.maps", "Google Карты"),
        ("com.google.android.apps.photos", "Google Фото"),
        ("com.google.android.googlequicksearchbox", "Google"),
        ("com.google.android.apps.docs", "Google Диск"),
        ("com.google.android.calendar", "Google Календарь"),
        ("com.google.android.dialer", "Телефон Google"),
        ("com.google.android.apps.messaging", "Сообщения Google"),
        ("com.google.android.inputmethod.latin", "Gboard"),
        ("com.google.android.keep", "Google Keep"),
        ("com.android.settings", "Настройки"),
        ("com.android.systemui", "Системный интерфейс"),
        ("com.android.phone", "Телефония"),
        ("com.miui.home", "Рабочий стол MIUI"),
        ("com.miui.securitycenter", "Безопасность MIUI"),
        ("com.miui.gallery", "Галерея MIUI"),
        ("com.android.camera", "Камера"),
        ("com.miui.weather2", "Погода MIUI"),
        ("com.miui.notes", "Заметки MIUI"),
        ("com.android.thememanager", "Темы MIUI"),
        ("com.sec.android.app.launcher", "Рабочий стол One UI"),
        ("com.sec.android.app.sbrowser", "Samsung Internet"),
        ("com.sec.android.gallery3d", "Галерея Samsung"),
        ("org.telegram.messenger", "Telegram"),
        ("com.whatsapp", "WhatsApp"),
        ("com.vkontakte.android", "VK"),
        ("ru.sberbankmobile", "СберБанк Онлайн"),
        ("com.instagram.android", "Instagram"),
        ("com.spotify.music", "Spotify"),
        ("org.videolan.vlc", "VLC"),
        ("com.facebook.katana", "Facebook"),
        ("com.zhiliaoapp.musically", "TikTok"),
        ("ru.yandex.searchplugin", "Яндекс"),
        ("com.yandex.browser", "Яндекс Браузер"),
        ("org.mozilla.firefox", "Firefox"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for t in tweaks {
        if let Action::RemovePackages { packages } | Action::DisablePackages { packages } = &t.action {
            if packages.len() == 1 {
                m.entry(packages[0].clone()).or_insert_with(|| t.title.clone());
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_packages_protected() {
        struct Never;
        impl Shell for Never {
            fn shell(&self, _: &str) -> Result<ShellOutput> {
                panic!("не должно выполняться")
            }
        }
        assert!(apply(&Never, 30, "com.android.systemui", PackageAction::Uninstall, true, "x").is_err());
        assert!(apply(&Never, 30, "com.android.providers.media", PackageAction::Disable, true, "x").is_err());
        assert!(apply(&Never, 30, "bad name", PackageAction::Enable, true, "x").is_err());
    }
}
