// AvG — оптимизатор Android-устройств через ADB.
// Copyright (C) 2026 DezFix
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Ядро AvG. Ничего не знает про интерфейс: его можно использовать
//! из GUI, из CLI или из тестов.

pub mod adb;
pub mod discover;
pub mod engine;
pub mod packages;
pub mod plan;
pub mod scan;
pub mod snapshot;
pub mod tweaks;

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("adb не найден. Укажите путь к adb в настройках или положите папку platform-tools рядом с AvG")]
    AdbNotFound,
    #[error("ошибка запуска adb: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Adb(String),
    #[error("не удалось разобрать ответ устройства: {0}")]
    Parse(String),
    #[error("ошибка в описании твиков: {0}")]
    Tweak(String),
    #[error("{0}")]
    Other(String),
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Имя пакета Android: латиница, цифры, `.` и `_`. Проверяем всё, что
/// подставляется в shell-команды, чтобы исключить инъекции.
pub fn is_valid_package(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.contains('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

/// Безопасный токен для `settings put`: ключи и значения без пробелов,
/// кавычек и спецсимволов shell.
pub fn is_safe_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | ','))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_validation() {
        assert!(is_valid_package("com.miui.msa.global"));
        assert!(is_valid_package("com.android.thememanager"));
        assert!(!is_valid_package("com.foo; rm -rf /"));
        assert!(!is_valid_package("nodot"));
        assert!(!is_valid_package(""));
    }

    #[test]
    fn token_validation() {
        assert!(is_safe_token("dns.adguard-dns.com"));
        assert!(is_safe_token("0.5"));
        assert!(!is_safe_token("a b"));
        assert!(!is_safe_token("x;reboot"));
        assert!(!is_safe_token("$(id)"));
    }
}
