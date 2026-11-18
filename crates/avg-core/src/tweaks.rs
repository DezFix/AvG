//! Описание твиков. Сами твики живут в TOML-файлах в папке `tweaks/`
//! в корне репозитория. Встроенные файлы вшиваются в программу при сборке,
//! дополнительные можно положить в папку пользователя.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{is_safe_token, is_valid_package, Error, Result};

/// Встроенные наборы твиков (путь относительно этого файла).
const BUILTIN: &[(&str, &str)] = &[
    ("general.toml", include_str!("../../../tweaks/general.toml")),
    ("google.toml", include_str!("../../../tweaks/google.toml")),
    ("xiaomi.toml", include_str!("../../../tweaks/xiaomi.toml")),
    ("samsung.toml", include_str!("../../../tweaks/samsung.toml")),
];

/// Профили оптимизации, которые видит пользователь.
pub const PROFILES: &[(&str, &str)] = &[
    ("weak", "Слабый телефон"),
    ("battery", "Батарея"),
    ("privacy", "Приватность"),
    ("custom", "Свой набор"),
];

pub fn profile_label(id: &str) -> &str {
    PROFILES
        .iter()
        .find(|(p, _)| *p == id)
        .map(|(_, l)| *l)
        .unwrap_or(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Namespace {
    Global,
    Secure,
    System,
}

impl Namespace {
    pub fn as_str(&self) -> &'static str {
        match self {
            Namespace::Global => "global",
            Namespace::Secure => "secure",
            Namespace::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingValue {
    pub namespace: Namespace,
    pub key: String,
    pub value: String,
}

impl SettingValue {
    /// Ключ для карты текущих значений: `global/window_animation_scale`.
    pub fn id(&self) -> String {
        format!("{}/{}", self.namespace.as_str(), self.key)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// `pm uninstall -k --user 0` — удаление для текущего пользователя.
    /// Сам APK остаётся в системе, вернуть можно через `install-existing`.
    RemovePackages { packages: Vec<String> },
    /// `pm disable-user --user 0` — отключение без удаления.
    DisablePackages { packages: Vec<String> },
    /// Изменение системных настроек. Старые значения запоминаются для отката.
    Settings { values: Vec<SettingValue> },
    /// Произвольные команды. Пустой `revert` = изменение нельзя откатить.
    Commands {
        apply: Vec<String>,
        #[serde(default)]
        revert: Vec<String>,
    },
    /// Уменьшение разрешения и плотности экрана в `factor` раз.
    ScreenScale { factor: f32 },
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tweak {
    pub id: String,
    pub title: String,
    pub description: String,
    /// bloat | ui | performance | battery | network | privacy
    pub category: String,
    pub risk: Risk,
    /// В каких профилях твик включён.
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Для каких производителей (`xiaomi`, `samsung`, …). Пусто — для всех.
    #[serde(default)]
    pub vendors: Vec<String>,
    #[serde(default)]
    pub min_sdk: Option<u32>,
    #[serde(default)]
    pub max_sdk: Option<u32>,
    /// Отмечен ли галочкой по умолчанию. Рискованные твики не отмечаются никогда.
    #[serde(default = "yes")]
    pub default_on: bool,
    pub action: Action,
}

#[derive(Debug, Deserialize)]
struct TweakFile {
    #[serde(default)]
    tweak: Vec<Tweak>,
}

pub const CATEGORIES: &[&str] = &["bloat", "ui", "performance", "battery", "network", "privacy"];

impl Tweak {
    pub fn validate(&self) -> Result<()> {
        let err = |m: String| Err(Error::Tweak(format!("{}: {m}", self.id)));
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
            return err("недопустимый id".into());
        }
        if !CATEGORIES.contains(&self.category.as_str()) {
            return err(format!("неизвестная категория «{}»", self.category));
        }
        for p in &self.profiles {
            if !PROFILES.iter().any(|(id, _)| id == p) {
                return err(format!("неизвестный профиль «{p}»"));
            }
        }
        match &self.action {
            Action::RemovePackages { packages } | Action::DisablePackages { packages } => {
                if packages.is_empty() {
                    return err("пустой список пакетов".into());
                }
                if let Some(p) = packages.iter().find(|p| !is_valid_package(p)) {
                    return err(format!("недопустимое имя пакета «{p}»"));
                }
            }
            Action::Settings { values } => {
                if values.is_empty() {
                    return err("пустой список настроек".into());
                }
                for v in values {
                    if !is_safe_token(&v.key) || !is_safe_token(&v.value) {
                        return err(format!("недопустимая настройка {}={}", v.key, v.value));
                    }
                }
            }
            Action::Commands { apply, .. } => {
                if apply.is_empty() {
                    return err("пустой список команд".into());
                }
            }
            Action::ScreenScale { factor } => {
                if !(0.5..1.0).contains(factor) {
                    return err("factor должен быть от 0.5 до 1.0".into());
                }
            }
        }
        Ok(())
    }

    pub fn reversible(&self) -> bool {
        !matches!(&self.action, Action::Commands { revert, .. } if revert.is_empty())
    }
}

/// Разбирает один TOML-файл с твиками.
pub fn parse(source_name: &str, text: &str) -> Result<Vec<Tweak>> {
    let file: TweakFile =
        toml::from_str(text).map_err(|e| Error::Tweak(format!("{source_name}: {e}")))?;
    for t in &file.tweak {
        t.validate()?;
    }
    Ok(file.tweak)
}

/// Встроенные твики.
pub fn builtin() -> Result<Vec<Tweak>> {
    let mut all = Vec::new();
    for (name, text) in BUILTIN {
        all.extend(parse(name, text)?);
    }
    check_unique(&all)?;
    Ok(all)
}

/// Встроенные твики плюс все `*.toml` из папки пользователя.
/// Твик из папки пользователя с тем же id заменяет встроенный.
pub fn load_with_user_dir(dir: &Path) -> Result<(Vec<Tweak>, Vec<String>)> {
    let mut all = builtin()?;
    let mut warnings = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            match std::fs::read_to_string(&path)
                .map_err(Error::from)
                .and_then(|t| parse(&name, &t))
            {
                Ok(list) => {
                    for t in list {
                        all.retain(|x| x.id != t.id);
                        all.push(t);
                    }
                }
                Err(e) => warnings.push(e.to_string()),
            }
        }
    }
    Ok((all, warnings))
}

fn check_unique(list: &[Tweak]) -> Result<()> {
    let mut seen = HashSet::new();
    for t in list {
        if !seen.insert(&t.id) {
            return Err(Error::Tweak(format!("повторяющийся id «{}»", t.id)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tweaks_are_valid() {
        let list = builtin().expect("встроенные твики должны разбираться");
        assert!(list.len() >= 15, "твиков: {}", list.len());
        for t in &list {
            assert!(!t.profiles.is_empty(), "{} не входит ни в один профиль", t.id);
        }
    }

    #[test]
    fn rejects_bad_package() {
        let src = r#"
[[tweak]]
id = "bad"
title = "x"
description = "x"
category = "bloat"
risk = "safe"
profiles = ["weak"]
action = { type = "remove_packages", packages = ["com.x; reboot"] }
"#;
        assert!(parse("t", src).is_err());
    }
}
