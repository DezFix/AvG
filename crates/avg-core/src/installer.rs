//! Мультиустановщик: разбор папки с APK, сравнение с телефоном и
//! подготовка наборов (.apks / .xapk) к `adb install-multiple`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::adb::Shell;
use crate::apk::{self, ApkMeta};
use crate::{is_valid_package, Error, Result};

pub const EXTENSIONS: &[&str] = &["apk", "apks", "xapk", "apkm"];
const MAX_INNER_APK: u64 = 512 << 20;

/// Что делать с файлом по сравнению с телефоном.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    New,
    Update,
    Same,
    Older,
    Bad,
    /// Телефон не подключён — сравнить не с чем.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApkFile {
    pub path: String,
    pub file_name: String,
    pub size: u64,
    /// apk | apks | xapk
    pub format: String,
    /// Сколько APK внутри набора (1 для обычного файла).
    pub parts: usize,
    pub package: String,
    pub label: Option<String>,
    pub version_code: u64,
    pub version_name: String,
    pub min_sdk: Option<u32>,
    pub abis: Vec<String>,
    /// data:-ссылка на иконку.
    pub icon: Option<String>,
    pub icon_bg: Option<String>,
    pub icon_adaptive: bool,
    pub kind: Kind,
    pub installed_version: Option<String>,
    /// Пояснение для «Не подходит» или ошибка разбора.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version_code: u64,
    pub version_name: String,
}

/// Файлы с подходящими расширениями в папке (без подпапок).
pub fn list_folder(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && has_apk_ext(p))
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    Ok(out)
}

pub fn has_apk_ext(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

fn format_of(p: &Path) -> String {
    let e = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if e == "apkm" { "apks".into() } else { e }
}

/// Разбирает файл. Ошибки не прерывают работу — попадают в `note`.
pub fn inspect(path: &Path, lang: &str) -> ApkFile {
    let mut f = ApkFile {
        path: path.display().to_string(),
        file_name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        format: format_of(path),
        parts: 1,
        package: String::new(),
        label: None,
        version_code: 0,
        version_name: String::new(),
        min_sdk: None,
        abis: Vec::new(),
        icon: None,
        icon_bg: None,
        icon_adaptive: false,
        kind: Kind::Unknown,
        installed_version: None,
        note: String::new(),
    };
    let res = if f.format == "apk" {
        File::open(path).map_err(Error::from).and_then(|h| apk::read_apk(BufReader::new(h), lang)).map(|m| (m, 1))
    } else {
        File::open(path).map_err(Error::from).and_then(|h| inspect_bundle(BufReader::new(h), lang))
    };
    match res {
        Ok((m, parts)) => {
            f.parts = parts;
            f.package = m.package;
            f.label = m.label;
            f.version_code = m.version_code;
            f.version_name = m.version_name;
            f.min_sdk = m.min_sdk;
            f.abis = m.abis;
            if let Some(i) = m.icon {
                f.icon = Some(i.data_uri());
                f.icon_bg = i.background.clone();
                f.icon_adaptive = i.adaptive;
            }
            if !is_valid_package(&f.package) {
                f.kind = Kind::Bad;
                f.note = "Недопустимое имя пакета".into();
            }
        }
        Err(e) => {
            f.kind = Kind::Bad;
            f.note = format!("Не удалось прочитать: {e}");
        }
    }
    f
}

/// Имена APK внутри набора, кроме «standalone»-вариантов bundletool.
fn inner_apks<R: Read + Seek>(zip: &zip::ZipArchive<R>) -> Vec<String> {
    zip.file_names()
        .flatten()
        .filter(|n| n.to_ascii_lowercase().ends_with(".apk") && !n.starts_with("standalones/"))
        .map(|n| n.into_owned())
        .collect()
}

/// Основной APK набора.
fn base_name(names: &[String]) -> Option<String> {
    let stem = |n: &str| n.rsplit('/').next().unwrap_or(n).trim_end_matches(".apk").to_ascii_lowercase();
    names
        .iter()
        .find(|n| matches!(stem(n).as_str(), "base" | "base-master"))
        .or_else(|| names.iter().find(|n| !stem(n).contains("config") && !stem(n).starts_with("split")))
        .or(names.first())
        .cloned()
}

fn read_inner<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut e = zip.by_name(name).map_err(|e| Error::Parse(format!("{name}: {e}")))?;
    if e.size() > MAX_INNER_APK {
        return Err(Error::Parse(format!("{name} слишком большой")));
    }
    let mut buf = Vec::with_capacity(e.size() as usize);
    e.read_to_end(&mut buf)?;
    Ok(buf)
}

fn inspect_bundle<R: Read + Seek>(r: R, lang: &str) -> Result<(ApkMeta, usize)> {
    let mut zip = zip::ZipArchive::new(r).map_err(|e| Error::Parse(format!("не архив: {e}")))?;
    let names = inner_apks(&zip);
    if names.is_empty() {
        return Err(Error::Parse("внутри нет APK".into()));
    }
    let base = base_name(&names).unwrap_or_default();
    let bytes = read_inner(&mut zip, &base)?;
    let mut meta = apk::read_apk(Cursor::new(bytes), lang)?;
    // Процессоры набора — по сплитам вида config.arm64_v8a.
    for n in &names {
        if let Some(a) = split_abi(n) {
            let a = abi_name(a).to_string();
            if !meta.abis.contains(&a) {
                meta.abis.push(a);
            }
        }
    }
    meta.abis.sort();
    if meta.icon.is_none() {
        if let Ok(icon) = read_inner(&mut zip, "icon.png") {
            meta.icon = Some(apk::Icon { mime: "image/png".into(), bytes: icon, background: None, adaptive: false });
        }
    }
    Ok((meta, names.len()))
}

const ABIS: &[&str] = &["arm64_v8a", "armeabi_v7a", "armeabi", "x86_64", "x86", "mips64", "mips"];

/// Имя ABI из сплита → имя, как его пишет Android (`ro.product.cpu.abilist`).
pub fn abi_name(split: &str) -> &str {
    match split {
        "arm64_v8a" => "arm64-v8a",
        "armeabi_v7a" => "armeabi-v7a",
        other => other,
    }
}

/// ABI конфигурационного сплита: `split_config.arm64_v8a.apk`, `base-x86.apk`.
pub fn split_abi(name: &str) -> Option<&'static str> {
    let file = name.rsplit('/').next().unwrap_or(name);
    let stem = file.strip_suffix(".apk").unwrap_or(file).to_ascii_lowercase();
    stem.split(['.', '-']).find_map(|seg| ABIS.iter().find(|a| **a == seg).copied())
}

/// Какие APK набора ставить: все, кроме сплитов чужих процессоров.
/// Из ABI-сплитов берётся один — для основного процессора телефона.
pub fn choose_parts(names: &[String], device_abis: &[String]) -> Vec<String> {
    let dev: Vec<String> = device_abis.iter().map(|a| a.trim().replace('-', "_")).collect();
    let present: Vec<&str> = names.iter().filter_map(|n| split_abi(n)).collect();
    let best = dev.iter().find(|d| present.contains(&d.as_str())).cloned();
    names
        .iter()
        .filter(|n| match split_abi(n) {
            None => true,
            Some(a) => best.as_deref() == Some(a),
        })
        .cloned()
        .collect()
}

/// Распаковывает набор во временную папку для `install-multiple`.
/// Распакованный набор: APK и OBB-файлы с путём назначения на телефоне.
pub struct Extracted {
    pub apks: Vec<PathBuf>,
    pub obbs: Vec<(PathBuf, String)>,
}

pub fn extract_bundle(path: &Path, device_abis: &[String], out: &Path) -> Result<Extracted> {
    std::fs::create_dir_all(out)?;
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(|e| Error::Parse(e.to_string()))?;
    let names = choose_parts(&inner_apks(&zip), device_abis);
    let mut apks = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let dest = out.join(format!("{i:02}.apk"));
        let mut e = zip.by_name(n).map_err(|e| Error::Parse(e.to_string()))?;
        let mut w = File::create(&dest)?;
        std::io::copy(&mut e, &mut w)?;
        apks.push(dest);
    }
    // OBB из XAPK: Android/obb/<пакет>/<файл>.obb → /sdcard/Android/obb/…
    let obb_names: Vec<String> = zip
        .file_names()
        .flatten()
        .filter(|n| n.starts_with("Android/obb/") && n.to_ascii_lowercase().ends_with(".obb"))
        .map(|n| n.into_owned())
        .collect();
    let mut obbs = Vec::new();
    for (i, n) in obb_names.iter().enumerate() {
        let remote = format!("/sdcard/{n}");
        if !crate::remote::is_safe_path(&remote) {
            continue;
        }
        let dest = out.join(format!("{i:02}.obb"));
        let mut e = zip.by_name(n).map_err(|e| Error::Parse(e.to_string()))?;
        std::io::copy(&mut e, &mut File::create(&dest)?)?;
        obbs.push((dest, remote));
    }
    if apks.is_empty() {
        return Err(Error::Parse("в наборе нет APK".into()));
    }
    Ok(Extracted { apks, obbs })
}

/// Версии установленных пакетов одним вызовом.
pub fn installed_versions(dev: &dyn Shell, packages: &[String]) -> Result<HashMap<String, Installed>> {
    let pkgs: Vec<&String> = packages.iter().filter(|p| is_valid_package(p)).collect();
    let mut out = HashMap::new();
    for chunk in pkgs.chunks(40) {
        let script = chunk
            .iter()
            .map(|p| format!("echo '@@{p}'; dumpsys package {p} | grep -E 'versionCode=|versionName='"))
            .collect::<Vec<_>>()
            .join("; ");
        let res = dev.shell(&script)?;
        out.extend(parse_versions(&res.stdout));
    }
    Ok(out)
}

pub fn parse_versions(text: &str) -> HashMap<String, Installed> {
    let mut out = HashMap::new();
    let mut cur: Option<(String, Option<u64>, Option<String>)> = None;
    let flush = |c: Option<(String, Option<u64>, Option<String>)>, out: &mut HashMap<String, Installed>| {
        if let Some((p, Some(code), name)) = c {
            out.insert(p, Installed { version_code: code, version_name: name.unwrap_or_default() });
        }
    };
    for line in text.lines() {
        let l = line.trim();
        if let Some(p) = l.strip_prefix("@@") {
            flush(cur.take(), &mut out);
            cur = Some((p.to_string(), None, None));
        } else if let Some(c) = cur.as_mut() {
            // Берём первую найденную версию (у обновлённых системных их две).
            if c.1.is_none() {
                if let Some(v) = l.split_whitespace().find_map(|w| w.strip_prefix("versionCode=")) {
                    c.1 = v.parse().ok();
                }
            }
            if c.2.is_none() {
                if let Some(v) = l.strip_prefix("versionName=") {
                    c.2 = Some(v.to_string());
                }
            }
        }
    }
    flush(cur, &mut out);
    out
}

/// Сравнивает файл с телефоном и проверяет совместимость.
pub fn classify(f: &mut ApkFile, installed: Option<&Installed>, sdk: u32, device_abis: &[String]) {
    if f.kind == Kind::Bad {
        return;
    }
    if let Some(min) = f.min_sdk {
        if sdk > 0 && min > sdk {
            f.kind = Kind::Bad;
            f.note = format!("Нужен Android {}, на телефоне {}", apk::android_version(min), apk::android_version(sdk));
            return;
        }
    }
    if !f.abis.is_empty() && !device_abis.is_empty() && !f.abis.iter().any(|a| device_abis.contains(a)) {
        f.kind = Kind::Bad;
        f.note = format!("Нет версии для процессора телефона ({})", device_abis.first().cloned().unwrap_or_default());
        return;
    }
    f.installed_version = installed.map(|i| i.version_name.clone());
    f.kind = match installed {
        None => Kind::New,
        Some(i) if f.version_code > i.version_code => Kind::Update,
        Some(i) if f.version_code == i.version_code => Kind::Same,
        Some(_) => Kind::Older,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk::testkit;

    #[test]
    fn picks_abi_split() {
        let names: Vec<String> = ["base.apk", "split_config.arm64_v8a.apk", "split_config.armeabi_v7a.apk", "split_config.xxhdpi.apk", "split_config.ru.apk"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got = choose_parts(&names, &["arm64-v8a".into(), "armeabi-v7a".into(), "armeabi".into()]);
        assert_eq!(got, vec!["base.apk", "split_config.arm64_v8a.apk", "split_config.xxhdpi.apk", "split_config.ru.apk"]);
        assert_eq!(split_abi("splits/base-x86_64.apk"), Some("x86_64"));
        assert_eq!(split_abi("splits/base-x86.apk"), Some("x86"));
        assert_eq!(split_abi("base-master.apk"), None);
    }

    #[test]
    fn parses_dumpsys_versions() {
        let t = "@@org.telegram.messenger\n    versionCode=51234 minSdk=21 targetSdk=34\n    versionName=11.4.2\n@@com.missing\n@@com.sys\n    versionCode=30 minSdk=30\n    versionName=3.0\n    versionCode=29 minSdk=29\n    versionName=2.9\n";
        let v = parse_versions(t);
        assert_eq!(v["org.telegram.messenger"], Installed { version_code: 51234, version_name: "11.4.2".into() });
        assert_eq!(v["com.sys"].version_code, 30);
        assert!(!v.contains_key("com.missing"));
    }

    #[test]
    fn classifies_against_phone() {
        let dir = std::env::temp_dir().join(format!("avg-inst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("demo.apk");
        std::fs::write(&p, testkit::demo_apk(Some("arm64-v8a"))).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(list_folder(&dir).unwrap(), vec![p.clone()]);

        let f = inspect(&p, "ru");
        assert_eq!(f.package, "com.example.demo");
        assert!(f.icon.as_deref().unwrap().starts_with("data:image/png;base64,"));
        let abis = vec!["arm64-v8a".to_string()];
        let cases = [(None, Kind::New), (Some(20), Kind::Update), (Some(25), Kind::Same), (Some(30), Kind::Older)];
        for (code, kind) in cases {
            let mut g = f.clone();
            let inst = code.map(|c| Installed { version_code: c, version_name: "x".into() });
            classify(&mut g, inst.as_ref(), 30, &abis);
            assert_eq!(g.kind, kind);
        }
        let mut old_phone = f.clone();
        classify(&mut old_phone, None, 23, &abis);
        assert_eq!(old_phone.kind, Kind::Bad);
        assert_eq!(old_phone.note, "Нужен Android 7.0, на телефоне 6");
        let mut x86 = f.clone();
        classify(&mut x86, None, 30, &["x86_64".into()]);
        assert_eq!(x86.kind, Kind::Bad);

        // Набор .apks: основной APK + два ABI-сплита.
        let bundle = testkit::zip(&[
            ("base.apk", testkit::demo_apk(None)),
            ("split_config.arm64_v8a.apk", b"PK".to_vec()),
            ("split_config.x86_64.apk", b"PK".to_vec()),
        ]);
        let b = dir.join("demo.apks");
        std::fs::write(&b, bundle).unwrap();
        let fb = inspect(&b, "ru");
        assert_eq!((fb.format.as_str(), fb.parts, fb.package.as_str()), ("apks", 3, "com.example.demo"));
        assert_eq!(fb.abis, vec!["arm64-v8a".to_string(), "x86_64".to_string()]);
        let x = extract_bundle(&b, &abis, &dir.join("out")).unwrap();
        assert_eq!(x.apks.len(), 2);
        assert!(x.obbs.is_empty());

        let broken = dir.join("broken.apk");
        std::fs::write(&broken, b"nope").unwrap();
        let fx = inspect(&broken, "ru");
        assert_eq!(fx.kind, Kind::Bad);
        assert!(fx.note.starts_with("Не удалось прочитать"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
