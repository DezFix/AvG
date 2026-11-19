//! Поиск adb на компьютере: официальные Platform-Tools, Android SDK,
//! менеджеры пакетов и эмуляторы (Nox, MuMu, LDPlayer, BlueStacks, MEmu…).

use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{Error, Result};

#[cfg(windows)]
pub const ADB_EXE: &str = "adb.exe";
#[cfg(not(windows))]
pub const ADB_EXE: &str = "adb";

/// Имена исполняемых файлов adb (в эмуляторах они бывают переименованы).
fn is_adb_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if cfg!(windows) {
        matches!(n.as_str(), "adb.exe" | "nox_adb.exe" | "hd-adb.exe")
    } else {
        n == "adb"
    }
}

/// Версия adb: протокол (1.0.**41**) и версия Platform-Tools (35.0.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Default)]
pub struct AdbVersion {
    pub protocol: u32,
    pub tools: (u32, u32, u32),
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub path: String,
    /// Откуда этот adb: «Android SDK», «Nox», «Скачан AvG» …
    pub source: String,
    pub emulator: bool,
    pub version: Option<AdbVersion>,
    /// Короткая подпись версии для интерфейса: «35.0.2» или «1.0.39».
    pub version_label: String,
    /// Слишком старый adb (до 1.0.41) может не видеть новые телефоны.
    pub outdated: bool,
}

/// Разбор вывода `adb version`.
pub fn parse_version(text: &str) -> Option<AdbVersion> {
    let mut v = AdbVersion::default();
    let mut found = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Android Debug Bridge version ") {
            if let Some(last) = rest.trim().rsplit('.').next().and_then(|x| x.parse().ok()) {
                v.protocol = last;
                found = true;
            }
        } else if let Some(rest) = line.strip_prefix("Version ") {
            let core = rest.split(['-', ' ']).next().unwrap_or_default();
            let mut it = core.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
            v.tools = (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0));
            found = true;
        }
    }
    found.then_some(v)
}

/// Откуда этот adb — по пути.
pub fn classify(path: &Path, data_dir: Option<&Path>) -> (String, bool) {
    if data_dir.is_some_and(|d| path.starts_with(d)) {
        return ("Скачан AvG".into(), false);
    }
    let p = path.to_string_lossy().to_lowercase().replace('\\', "/");
    let emulators = [
        ("nox", "Nox"),
        ("mumu", "MuMu Player"),
        ("netease", "MuMu Player"),
        ("ldplayer", "LDPlayer"),
        ("bluestacks", "BlueStacks"),
        ("microvirt", "MEmu"),
        ("memu", "MEmu"),
        ("genymo", "Genymotion"),
    ];
    for (k, name) in emulators {
        if p.contains(k) {
            return (name.into(), true);
        }
    }
    let other = [
        ("android/sdk", "Android SDK"),
        ("library/android", "Android SDK"),
        ("chocolatey", "Chocolatey"),
        ("scoop", "Scoop"),
        ("winget", "WinGet"),
        ("scrcpy", "scrcpy"),
        ("homebrew", "Homebrew"),
        ("platform-tools", "Platform-Tools"),
    ];
    for (k, name) in other {
        if p.contains(k) {
            return (name.into(), false);
        }
    }
    ("Другое".into(), false)
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Известные места без обхода дисков, в порядке предпочтения:
/// сначала официальные сборки, потом эмуляторы.
pub fn known_paths(extra_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    for d in extra_dirs {
        v.push(d.join("platform-tools").join(ADB_EXE));
        v.push(d.join(ADB_EXE));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&paths) {
            v.push(d.join(ADB_EXE));
        }
    }
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(sdk) = env_path(var) {
            v.push(sdk.join("platform-tools").join(ADB_EXE));
        }
    }
    let home = env_path("USERPROFILE").or_else(|| env_path("HOME"));

    if cfg!(windows) {
        let pf = env_path("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        let pf86 = env_path("ProgramFiles(x86)").unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
        let pd = env_path("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        if let Some(lad) = env_path("LOCALAPPDATA") {
            v.push(lad.join(r"Android\Sdk\platform-tools\adb.exe"));
            v.push(lad.join(r"Microsoft\WinGet\Links\adb.exe"));
        }
        v.push(pd.join(r"chocolatey\bin\adb.exe"));
        v.push(pd.join(r"chocolatey\lib\adb\tools\platform-tools\adb.exe"));
        if let Some(h) = &home {
            v.push(h.join(r"scoop\shims\adb.exe"));
            v.push(h.join(r"scoop\apps\adb\current\platform-tools\adb.exe"));
            v.push(h.join(r"Downloads\platform-tools\adb.exe"));
            v.push(h.join(r"Desktop\platform-tools\adb.exe"));
        }
        for root in [r"C:\platform-tools", r"C:\adb", r"C:\Android\platform-tools", r"D:\platform-tools"] {
            v.push(PathBuf::from(root).join("adb.exe"));
        }
        // Эмуляторы.
        for base in [&pf, &pf86] {
            v.push(base.join(r"Nox\bin\nox_adb.exe"));
            v.push(base.join(r"Nox\bin\adb.exe"));
            v.push(base.join(r"Netease\MuMuPlayer-12.0\shell\adb.exe"));
            v.push(base.join(r"Netease\MuMu\nx_main\adb.exe"));
            v.push(base.join(r"BlueStacks_nxt\HD-Adb.exe"));
            v.push(base.join(r"BlueStacks\HD-Adb.exe"));
            v.push(base.join(r"Microvirt\MEmu\adb.exe"));
            v.push(base.join(r"Genymobile\Genymotion\tools\adb.exe"));
            v.push(base.join(r"LDPlayer\LDPlayer9\adb.exe"));
        }
        for drive in ["C", "D", "E"] {
            for ld in ["LDPlayer9", "LDPlayer4.0", "LDPlayer64"] {
                v.push(PathBuf::from(format!(r"{drive}:\LDPlayer\{ld}\adb.exe")));
            }
        }
    } else if let Some(h) = &home {
        v.push(h.join("Library/Android/sdk/platform-tools/adb"));
        v.push(h.join("Android/Sdk/platform-tools/adb"));
        v.push(PathBuf::from("/opt/homebrew/bin/adb"));
        v.push(PathBuf::from("/usr/local/bin/adb"));
        v.push(PathBuf::from("/usr/bin/adb"));
    }
    v
}

/// Папки, которые нет смысла обходить: системные и огромные.
fn skip_dir(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with('$')
        || n.starts_with('.')
        || matches!(
            n.as_str(),
            "windows" | "system volume information" | "winsxs" | "node_modules" | "target"
                | "steamapps" | "temp" | "tmp" | "cache" | "caches" | "packages" | "proc" | "sys"
                | "dev" | "recovery" | "msocache" | "perflogs" | "config.msi" | "onedrive"
        )
}

/// Обход дисков с ограничением глубины и числа папок.
pub fn deep_scan(roots: &[(PathBuf, usize)], dir_budget: usize, time_budget: Duration) -> Vec<PathBuf> {
    let started = Instant::now();
    let mut found = Vec::new();
    let mut visited = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().rev().cloned().collect();
    while let Some((dir, depth_left)) = stack.pop() {
        visited += 1;
        if visited > dir_budget || started.elapsed() > time_budget {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_file() && is_adb_name(&name) {
                found.push(e.path());
            } else if ft.is_dir() && depth_left > 0 && !skip_dir(&name) {
                stack.push((e.path(), depth_left - 1));
            }
        }
    }
    found
}

/// Корни для глубокого поиска.
pub fn default_roots() -> Vec<(PathBuf, usize)> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        for p in [env_path("ProgramFiles"), env_path("ProgramFiles(x86)"), env_path("LOCALAPPDATA")].into_iter().flatten() {
            roots.push((p, 4));
        }
        if let Some(h) = env_path("USERPROFILE") {
            roots.push((h, 3));
        }
        for letter in 'C'..='Z' {
            let root = PathBuf::from(format!("{letter}:\\"));
            if root.is_dir() {
                roots.push((root, 4));
            }
        }
    } else {
        if let Some(h) = env_path("HOME") {
            roots.push((h, 4));
        }
        roots.push((PathBuf::from("/opt"), 4));
        roots.push((PathBuf::from("/Applications"), 5));
    }
    roots
}

/// Запускает `<adb> version` с ограничением по времени.
pub fn probe_version(path: &Path) -> Option<AdbVersion> {
    let mut cmd = Command::new(path);
    cmd.arg("version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_version(&out)
}

/// Все adb на компьютере, лучший — первым.
pub fn discover(extra_dirs: &[PathBuf], data_dir: Option<&Path>, deep: bool) -> Vec<Candidate> {
    let mut paths: Vec<PathBuf> = known_paths(extra_dirs).into_iter().filter(|p| p.is_file()).collect();
    if deep {
        paths.extend(deep_scan(&default_roots(), 60_000, Duration::from_secs(25)));
    }
    let mut seen = HashSet::new();
    let mut list: Vec<Candidate> = Vec::new();
    for p in paths {
        let key = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if !seen.insert(key.to_string_lossy().to_lowercase()) {
            continue;
        }
        let Some(version) = probe_version(&p) else { continue };
        let (source, emulator) = classify(&p, data_dir);
        let version_label = if version.tools.0 > 0 {
            format!("{}.{}.{}", version.tools.0, version.tools.1, version.tools.2)
        } else {
            format!("1.0.{}", version.protocol)
        };
        list.push(Candidate {
            path: p.to_string_lossy().to_string(),
            source,
            emulator,
            outdated: version.protocol < 41,
            version: Some(version),
            version_label,
        });
    }
    // Новее — лучше; при равной версии официальный adb лучше эмуляторного.
    list.sort_by(|a, b| b.version.cmp(&a.version).then(a.emulator.cmp(&b.emulator)));
    list
}

/// Адрес официального архива Platform-Tools для этой ОС.
pub fn platform_tools_url() -> &'static str {
    if cfg!(windows) {
        "https://dl.google.com/android/repository/platform-tools-latest-windows.zip"
    } else if cfg!(target_os = "macos") {
        "https://dl.google.com/android/repository/platform-tools-latest-darwin.zip"
    } else {
        "https://dl.google.com/android/repository/platform-tools-latest-linux.zip"
    }
}

/// Распаковывает архив Platform-Tools в `dest/platform-tools` и возвращает путь к adb.
/// Сначала распаковывает во временную папку, чтобы не оставить полуготовый adb.
pub fn install_platform_tools(zip_bytes: &[u8], dest: &Path) -> Result<PathBuf> {
    let mut zip = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|e| Error::Other(format!("архив повреждён: {e}")))?;
    let tmp = dest.join("platform-tools.download");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| Error::Other(format!("архив повреждён: {e}")))?;
        // enclosed_name отбрасывает пути вида ../../ — защита от «zip slip».
        let Some(rel) = f.enclosed_name() else { continue };
        let out = tmp.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf)?;
        std::fs::write(&out, &buf)?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    let unpacked = tmp.join("platform-tools");
    if !unpacked.join(ADB_EXE).is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(Error::Other("в архиве нет adb".into()));
    }
    let target = dest.join("platform-tools");
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| {
            Error::Other(format!("не удалось заменить старую папку platform-tools (возможно, adb ещё запущен): {e}"))
        })?;
    }
    std::fs::rename(&unpacked, &target)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(target.join(ADB_EXE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn versions() {
        let v = parse_version("Android Debug Bridge version 1.0.41\nVersion 35.0.2-12147458\nInstalled as C:\\x\\adb.exe\n").unwrap();
        assert_eq!(v.protocol, 41);
        assert_eq!(v.tools, (35, 0, 2));
        let old = parse_version("Android Debug Bridge version 1.0.36\nRevision 0e9850346394-android\n").unwrap();
        assert_eq!(old.protocol, 36);
        assert!(old < v);
        assert!(parse_version("garbage").is_none());
    }

    #[test]
    fn classifies() {
        let c = |p: &str| classify(Path::new(p), Some(Path::new("/data/avg")));
        assert_eq!(c(r"C:\Program Files (x86)\Nox\bin\nox_adb.exe"), ("Nox".into(), true));
        assert_eq!(c(r"C:\LDPlayer\LDPlayer9\adb.exe"), ("LDPlayer".into(), true));
        assert_eq!(c(r"C:\Users\u\AppData\Local\Android\Sdk\platform-tools\adb.exe"), ("Android SDK".into(), false));
        assert_eq!(c("/data/avg/platform-tools/adb"), ("Скачан AvG".into(), false));
    }

    #[test]
    fn deep_scan_finds_and_respects_depth() {
        let root = std::env::temp_dir().join(format!("avg-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shallow = root.join("Emu").join("bin");
        let deep = root.join("a/b/c/d/e/f");
        let skipped = root.join("node_modules/x");
        for d in [&shallow, &deep, &skipped] {
            std::fs::create_dir_all(d).unwrap();
        }
        let name = if cfg!(windows) { "nox_adb.exe" } else { "adb" };
        for d in [&shallow, &deep, &skipped] {
            std::fs::write(d.join(name), b"").unwrap();
        }
        let found = deep_scan(&[(root.clone(), 3)], 1000, Duration::from_secs(5));
        assert_eq!(found, vec![shallow.join(name)]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installs_from_zip() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            w.add_directory("platform-tools/", opts).unwrap();
            w.start_file(format!("platform-tools/{ADB_EXE}"), opts).unwrap();
            w.write_all(b"fake adb").unwrap();
            w.start_file("../evil.txt", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let dest = std::env::temp_dir().join(format!("avg-pt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).unwrap();
        let adb = install_platform_tools(&buf, &dest).unwrap();
        assert_eq!(std::fs::read(&adb).unwrap(), b"fake adb");
        assert!(!dest.parent().unwrap().join("evil.txt").exists());
        // Повторная установка заменяет старую папку.
        install_platform_tools(&buf, &dest).unwrap();
        let _ = std::fs::remove_dir_all(&dest);
    }
}

#[cfg(test)]
mod real {
    /// Ручная проверка на настоящем компьютере: `cargo test -p avg-core real_discover -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_discover() {
        for c in super::discover(&[], None, true) {
            println!("{} | {} | {} | emulator={} outdated={}", c.path, c.source, c.version_label, c.emulator, c.outdated);
        }
    }
}
