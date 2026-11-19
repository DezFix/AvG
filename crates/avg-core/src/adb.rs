//! Обёртка над бинарником `adb` из Android platform-tools.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::{Error, Result};

use crate::discover::ADB_EXE;

/// Результат команды на устройстве.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl ShellOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// stdout и stderr одной строкой, без лишних пробелов по краям.
    pub fn text(&self) -> String {
        let out = self.stdout.trim();
        let err = self.stderr.trim();
        match (out.is_empty(), err.is_empty()) {
            (true, true) => String::new(),
            (false, true) => out.to_string(),
            (true, false) => err.to_string(),
            (false, false) => format!("{out}\n{err}"),
        }
    }

    /// Эвристика успеха для команд pm/cmd/settings: код 0 и нет слов-маркеров ошибки.
    pub fn looks_ok(&self) -> bool {
        let t = self.text();
        self.ok()
            && !t.contains("Failure")
            && !t.contains("Exception")
            && !t.starts_with("Error")
            && !t.contains("Unknown command")
    }
}

/// Всё, что умеет выполнять shell-команды на конкретном устройстве.
/// Нужен, чтобы движок можно было тестировать без телефона.
pub trait Shell {
    fn shell(&self, command: &str) -> Result<ShellOutput>;
}

/// Устройство из `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceEntry {
    pub serial: String,
    /// `device`, `unauthorized`, `offline`, `recovery`, `sideload`, `no permissions` …
    pub state: String,
    pub model: Option<String>,
    pub product: Option<String>,
    pub device: Option<String>,
    /// `usb`, `wifi` или пусто.
    pub transport: String,
}

#[derive(Debug, Clone)]
pub struct Adb {
    path: PathBuf,
}

impl Adb {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Быстрый поиск adb: явный путь → папки программы → PATH → Android SDK →
    /// менеджеры пакетов → эмуляторы. Диски не обходит (см. `discover`).
    pub fn locate(custom: Option<&Path>, extra_dirs: &[PathBuf]) -> Result<Self> {
        if let Some(p) = custom {
            let p = if p.is_dir() { p.join(ADB_EXE) } else { p.to_path_buf() };
            return if p.is_file() { Ok(Self::new(p)) } else { Err(Error::AdbNotFound) };
        }

        let candidates = crate::discover::known_paths(extra_dirs);
        candidates
            .into_iter()
            .find(|c| c.is_file())
            .map(Self::new)
            .ok_or(Error::AdbNotFound)
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.path);
        c.stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        c
    }

    /// Запускает `adb <args…>` и ждёт завершения.
    pub fn run<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> Result<ShellOutput> {
        let out = self.command().args(args).output()?;
        Ok(ShellOutput {
            stdout: String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
            stderr: String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"),
            code: out.status.code().unwrap_or(-1),
        })
    }

    pub fn version(&self) -> Result<String> {
        let out = self.run(&["version"])?;
        Ok(out.stdout.lines().next().unwrap_or_default().trim().to_string())
    }

    pub fn devices(&self) -> Result<Vec<DeviceEntry>> {
        let out = self.run(&["devices", "-l"])?;
        if !out.ok() {
            return Err(Error::Adb(out.text()));
        }
        Ok(parse_devices(&out.stdout))
    }

    pub fn shell(&self, serial: &str, command: &str) -> Result<ShellOutput> {
        self.run(&["-s", serial, "shell", command])
    }

    pub fn install(&self, serial: &str, apk: &Path) -> Result<ShellOutput> {
        let mut args: Vec<std::ffi::OsString> =
            vec!["-s".into(), serial.into(), "install".into(), "-r".into()];
        args.push(apk.as_os_str().to_owned());
        self.run(&args)
    }

    pub fn pull(&self, serial: &str, remote: &str, local: &Path) -> Result<ShellOutput> {
        let args: Vec<std::ffi::OsString> = vec![
            "-s".into(),
            serial.into(),
            "pull".into(),
            remote.into(),
            local.as_os_str().to_owned(),
        ];
        self.run(&args)
    }

    /// Привязывает adb к конкретному устройству.
    pub fn target<'a>(&'a self, serial: &'a str) -> Target<'a> {
        Target { adb: self, serial }
    }
}

/// adb + серийный номер = конкретный телефон.
pub struct Target<'a> {
    pub adb: &'a Adb,
    pub serial: &'a str,
}

impl Shell for Target<'_> {
    fn shell(&self, command: &str) -> Result<ShellOutput> {
        self.adb.shell(self.serial, command)
    }
}

/// Разбор вывода `adb devices -l`.
pub fn parse_devices(stdout: &str) -> Vec<DeviceEntry> {
    let mut list = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("List of devices") || line.starts_with('*') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else { continue };
        let rest: Vec<&str> = parts.collect();
        // Состояние может состоять из нескольких слов («no permissions (…)»):
        // всё до первого key:value.
        let state_words: Vec<&str> = rest
            .iter()
            .take_while(|w| !(w.contains(':') && !w.starts_with('(')))
            .copied()
            .collect();
        let mut state = state_words.join(" ");
        if state.starts_with("no permissions") {
            state = "no permissions".into();
        }
        let mut e = DeviceEntry {
            serial: serial.to_string(),
            state,
            model: None,
            product: None,
            device: None,
            transport: String::new(),
        };
        for kv in &rest {
            if let Some((k, v)) = kv.split_once(':') {
                match k {
                    "model" => e.model = Some(v.replace('_', " ")),
                    "product" => e.product = Some(v.to_string()),
                    "device" => e.device = Some(v.to_string()),
                    "usb" => e.transport = "usb".into(),
                    _ => {}
                }
            }
        }
        if e.transport.is_empty() && serial.contains(':') {
            e.transport = "wifi".into();
        }
        list.push(e);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devices() {
        let out = "* daemon not running; starting now at tcp:5037\n\
                   List of devices attached\n\
                   7c1a2f device usb:1-1 product:ginkgo model:Redmi_Note_8 device:ginkgo transport_id:1\n\
                   R58M12 unauthorized usb:1-2 transport_id:2\n\
                   192.168.1.5:5555 device product:a52q model:SM_A525F device:a52q transport_id:3\n\n";
        let d = parse_devices(out);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].serial, "7c1a2f");
        assert_eq!(d[0].state, "device");
        assert_eq!(d[0].model.as_deref(), Some("Redmi Note 8"));
        assert_eq!(d[0].transport, "usb");
        assert_eq!(d[1].state, "unauthorized");
        assert_eq!(d[2].transport, "wifi");
    }

    #[test]
    fn parses_no_permissions() {
        let out = "List of devices attached\nABC no permissions (missing udev rules?); see [http://developer.android.com/tools/device.html] usb:1-1 transport_id:1\n";
        let d = parse_devices(out);
        assert_eq!(d[0].state, "no permissions");
    }
}
