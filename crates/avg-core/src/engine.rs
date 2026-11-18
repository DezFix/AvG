//! Применение и откат твиков. Перед каждым изменением запоминается,
//! как его отменить, — из этого строятся точки отката.

use serde::{Deserialize, Serialize};

use crate::adb::{Shell, ShellOutput};
use crate::is_safe_token;
use crate::scan::Scan;
use crate::tweaks::{Action, Tweak};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Выполняемая команда.
    Cmd,
    Ok,
    Err,
    Info,
}

/// Одно применённое изменение и способ его отменить.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub tweak_id: String,
    pub title: String,
    /// Всё ли прошло без ошибок.
    pub ok: bool,
    /// Команды отката в том порядке, в котором их нужно выполнить.
    pub revert: Vec<String>,
    #[serde(default)]
    pub reverted: bool,
    /// Пояснение для пользователя (например, «нельзя откатить»).
    #[serde(default)]
    pub note: String,
}

type Log<'a> = &'a mut dyn FnMut(LogLevel, &str);

fn run(dev: &dyn Shell, cmd: &str, log: Log) -> ShellOutput {
    log(LogLevel::Cmd, cmd);
    let out = dev.shell(cmd).unwrap_or_else(|e| ShellOutput {
        stdout: String::new(),
        stderr: e.to_string(),
        code: -1,
    });
    let text = out.text();
    if !text.is_empty() {
        log(if out.looks_ok() { LogLevel::Ok } else { LogLevel::Err }, &text);
    }
    out
}

fn quiet(dev: &dyn Shell, cmd: &str) -> String {
    dev.shell(cmd).map(|o| o.stdout.trim().to_string()).unwrap_or_default()
}

/// Команда, возвращающая удалённый для пользователя системный пакет.
pub fn restore_command(sdk: u32, package: &str) -> String {
    if sdk >= 24 || sdk == 0 {
        format!("cmd package install-existing {package}")
    } else {
        format!("pm install-existing {package}")
    }
}

/// Применяет один твик. Ошибки не прерывают работу: всё, что удалось
/// сделать, попадает в `revert`.
pub fn apply_tweak(dev: &dyn Shell, scan: &Scan, t: &Tweak, log: Log) -> Change {
    let mut revert: Vec<String> = Vec::new();
    let mut failures = 0usize;
    let mut note = String::new();
    let sdk = scan.info.sdk;

    match &t.action {
        Action::RemovePackages { packages } => {
            for p in packages.iter().filter(|p| scan.installed.contains(*p)) {
                let out = run(dev, &format!("pm uninstall -k --user 0 {p}"), log);
                if out.stdout.contains("Success") {
                    revert.push(restore_command(sdk, p));
                } else {
                    failures += 1;
                }
            }
        }
        Action::DisablePackages { packages } => {
            for p in packages
                .iter()
                .filter(|p| scan.installed.contains(*p) && !scan.disabled.contains(*p))
            {
                let out = run(dev, &format!("pm disable-user --user 0 {p}"), log);
                if out.text().contains("disabled") {
                    revert.push(format!("pm enable {p}"));
                } else {
                    failures += 1;
                }
            }
        }
        Action::Settings { values } => {
            for v in values {
                let ns = v.namespace.as_str();
                let old = quiet(dev, &format!("settings get {ns} {}", v.key));
                if old == v.value {
                    continue;
                }
                let out = run(dev, &format!("settings put {ns} {} {}", v.key, v.value), log);
                if !out.looks_ok() {
                    failures += 1;
                    continue;
                }
                if old.is_empty() || old == "null" {
                    revert.push(format!("settings delete {ns} {}", v.key));
                } else if is_safe_token(&old) {
                    revert.push(format!("settings put {ns} {} {old}", v.key));
                } else {
                    note = format!("Старое значение {} не удалось сохранить для отката", v.key);
                }
            }
        }
        Action::Commands { apply, revert: rev } => {
            for c in apply {
                if !run(dev, c, log).looks_ok() {
                    failures += 1;
                }
            }
            if rev.is_empty() {
                note = "Это действие не требует отката".into();
            } else {
                // Порядок сохраняем как в описании: ниже весь список разворачивается.
                revert.extend(rev.iter().rev().cloned());
            }
        }
        Action::ScreenScale { factor } => {
            let size = quiet(dev, "wm size");
            let density = quiet(dev, "wm density");
            match (parse_wm_size(&size), parse_wm_density(&density)) {
                (Some((w, h)), Some(d)) => {
                    let even = |x: f32| ((x / 2.0).round() as u32) * 2;
                    let (nw, nh) = (even(w as f32 * factor), even(h as f32 * factor));
                    let nd = (d as f32 * factor).round() as u32;
                    let a = run(dev, &format!("wm size {nw}x{nh}"), log);
                    let b = run(dev, &format!("wm density {nd}"), log);
                    if a.looks_ok() || b.looks_ok() {
                        revert.push("wm size reset".into());
                        revert.push("wm density reset".into());
                    }
                    if !a.looks_ok() || !b.looks_ok() {
                        failures += 1;
                    }
                }
                _ => {
                    log(LogLevel::Err, "Не удалось определить разрешение экрана");
                    failures += 1;
                }
            }
        }
    }

    // Откатывать нужно в обратном порядке.
    revert.reverse();
    Change {
        tweak_id: t.id.clone(),
        title: t.title.clone(),
        ok: failures == 0,
        revert,
        reverted: false,
        note,
    }
}

/// Выполняет команды отката. Возвращает `true`, если все прошли успешно.
pub fn revert_change(dev: &dyn Shell, change: &Change, log: Log) -> bool {
    let mut ok = true;
    for c in &change.revert {
        if !run(dev, c, log).looks_ok() {
            ok = false;
        }
    }
    ok
}

/// «Physical size: 1080x2340» (и, возможно, «Override size: …»).
pub fn parse_wm_size(text: &str) -> Option<(u32, u32)> {
    let line = text.lines().find(|l| l.contains("Physical size"))?;
    let (w, h) = line.split(':').nth(1)?.trim().split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

pub fn parse_wm_density(text: &str) -> Option<u32> {
    let line = text.lines().find(|l| l.contains("Physical density"))?;
    line.split(':').nth(1)?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweaks;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Поддельный телефон: хранит настройки и пакеты в памяти.
    struct Fake {
        settings: RefCell<HashMap<String, String>>,
        packages: RefCell<Vec<String>>,
        history: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new() -> Self {
            let mut s = HashMap::new();
            s.insert("global window_animation_scale".into(), "1.0".into());
            Fake {
                settings: RefCell::new(s),
                packages: RefCell::new(vec!["com.miui.msa.global".into()]),
                history: RefCell::new(vec![]),
            }
        }
    }

    impl Shell for Fake {
        fn shell(&self, cmd: &str) -> crate::Result<ShellOutput> {
            self.history.borrow_mut().push(cmd.into());
            let w: Vec<&str> = cmd.split_whitespace().collect();
            let ok = |s: &str| Ok(ShellOutput { stdout: s.into(), stderr: String::new(), code: 0 });
            match w.as_slice() {
                ["settings", "get", ns, k] => {
                    ok(self.settings.borrow().get(&format!("{ns} {k}")).map(String::as_str).unwrap_or("null"))
                }
                ["settings", "put", ns, k, v] => {
                    self.settings.borrow_mut().insert(format!("{ns} {k}"), v.to_string());
                    ok("")
                }
                ["settings", "delete", ns, k] => {
                    self.settings.borrow_mut().remove(&format!("{ns} {k}"));
                    ok("Deleted 1 rows")
                }
                ["pm", "uninstall", "-k", "--user", "0", p] => {
                    self.packages.borrow_mut().retain(|x| x != p);
                    ok("Success")
                }
                ["cmd", "package", "install-existing", p] => {
                    self.packages.borrow_mut().push(p.to_string());
                    ok(&format!("Package {p} installed for user: 0"))
                }
                ["wm", "size"] => ok("Physical size: 1080x2340"),
                ["wm", "density"] => ok("Physical density: 440"),
                _ => ok(""),
            }
        }
    }

    fn scan() -> Scan {
        let mut s = Scan::default();
        s.info.sdk = 30;
        s.info.vendor = "xiaomi".into();
        s.installed.insert("com.miui.msa.global".into());
        s
    }

    fn tweak(id: &str) -> Tweak {
        tweaks::builtin().unwrap().into_iter().find(|t| t.id == id).unwrap()
    }

    #[test]
    fn settings_apply_and_revert() {
        let dev = Fake::new();
        let mut log = |_: LogLevel, _: &str| {};
        let ch = apply_tweak(&dev, &scan(), &tweak("general.animations"), &mut log);
        assert!(ch.ok);
        assert_eq!(dev.settings.borrow()["global window_animation_scale"], "0.5");
        // Было 1.0 → вернём 1.0; остальных ключей не было → удалим.
        assert!(ch.revert.contains(&"settings put global window_animation_scale 1.0".to_string()));
        assert!(ch.revert.contains(&"settings delete global animator_duration_scale".to_string()));
        assert!(revert_change(&dev, &ch, &mut log));
        assert_eq!(dev.settings.borrow()["global window_animation_scale"], "1.0");
        assert!(!dev.settings.borrow().contains_key("global animator_duration_scale"));
    }

    #[test]
    fn remove_and_restore_package() {
        let dev = Fake::new();
        let mut log = |_: LogLevel, _: &str| {};
        let ch = apply_tweak(&dev, &scan(), &tweak("xiaomi.msa"), &mut log);
        assert!(ch.ok);
        assert!(dev.packages.borrow().is_empty());
        assert_eq!(ch.revert, vec!["cmd package install-existing com.miui.msa.global".to_string()]);
        // Второго пакета (systemAdSolution) на телефоне нет — его не трогаем.
        assert!(!dev.history.borrow().iter().any(|c| c.contains("systemAdSolution")));
        revert_change(&dev, &ch, &mut log);
        assert_eq!(dev.packages.borrow().len(), 1);
    }

    #[test]
    fn screen_scale() {
        let dev = Fake::new();
        let mut log = |_: LogLevel, _: &str| {};
        let ch = apply_tweak(&dev, &scan(), &tweak("general.screen_scale"), &mut log);
        assert!(dev.history.borrow().contains(&"wm size 810x1756".to_string()));
        assert!(dev.history.borrow().contains(&"wm density 330".to_string()));
        assert_eq!(ch.revert, vec!["wm density reset".to_string(), "wm size reset".to_string()]);
    }
}
