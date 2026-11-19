//! Чтение APK без Android SDK: пакет, версия, minSdk, название и иконка.
//!
//! Разбирает бинарный `AndroidManifest.xml` (AXML) и таблицу ресурсов
//! `resources.arsc`, чтобы найти название на нужном языке и PNG/WebP-иконку
//! наибольшей плотности. Работает с любым `Read + Seek`: файлом на диске,
//! APK в памяти или APK на телефоне, который читается кусками через adb.

use std::collections::HashMap;
use std::io::{Read, Seek};

use serde::Serialize;

use crate::{Error, Result};

const MAX_MANIFEST: u64 = 8 << 20;
const MAX_ARSC: u64 = 96 << 20;
const MAX_ICON: u64 = 8 << 20;

// Идентификаторы атрибутов android:* (android.R.attr).
const ATTR_LABEL: u32 = 0x0101_0001;
const ATTR_ICON: u32 = 0x0101_0002;
const ATTR_MIN_SDK: u32 = 0x0101_020c;
const ATTR_TARGET_SDK: u32 = 0x0101_0270;
const ATTR_VERSION_CODE: u32 = 0x0101_021b;
const ATTR_VERSION_NAME: u32 = 0x0101_021c;
const ATTR_ROUND_ICON: u32 = 0x0101_052c;
const ATTR_DRAWABLE: u32 = 0x0101_0199;
const ATTR_SRC: u32 = 0x0101_0119;

// Типы значений Res_value.
const T_REFERENCE: u8 = 0x01;
const T_STRING: u8 = 0x03;
const T_INT_DEC: u8 = 0x10;
const T_INT_HEX: u8 = 0x11;
const T_COLOR_FIRST: u8 = 0x1c;
const T_COLOR_LAST: u8 = 0x1f;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Icon {
    pub mime: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
    /// Цвет фона адаптивной иконки, `#RRGGBB`.
    pub background: Option<String>,
    /// Слой адаптивной иконки: рисунок занимает только центр холста.
    pub adaptive: bool,
}

impl Icon {
    pub fn data_uri(&self) -> String {
        format!("data:{};base64,{}", self.mime, base64(&self.bytes))
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ApkMeta {
    pub package: String,
    pub version_code: u64,
    pub version_name: String,
    pub min_sdk: Option<u32>,
    pub target_sdk: Option<u32>,
    pub label: Option<String>,
    /// Имя сплита (`config.arm64_v8a`), у основного APK — пусто.
    pub split: Option<String>,
    /// Процессоры, под которые есть нативные библиотеки (`lib/<abi>/`).
    pub abis: Vec<String>,
    #[serde(skip)]
    pub icon: Option<Icon>,
}

/// Разбирает APK. `lang` — предпочтительный язык названия (`ru`).
pub fn read_apk<R: Read + Seek>(reader: R, lang: &str) -> Result<ApkMeta> {
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| bad(format!("не APK: {e}")))?;
    let mut abis: Vec<String> = Vec::new();
    for name in zip.file_names().flatten() {
        if let Some(rest) = name.strip_prefix("lib/") {
            if let Some((abi, _)) = rest.split_once('/') {
                if !abi.is_empty() && !abis.iter().any(|a| a == abi) {
                    abis.push(abi.to_string());
                }
            }
        }
    }
    abis.sort();

    let manifest = read_entry(&mut zip, "AndroidManifest.xml", MAX_MANIFEST)?
        .ok_or_else(|| bad("в архиве нет AndroidManifest.xml".into()))?;
    let doc = Axml::parse(&manifest)?;
    let mut meta = ApkMeta { abis, ..Default::default() };
    let mut label = None;
    let mut icon = None;
    let mut round_icon = None;
    for el in &doc.elements {
        match el.name.as_str() {
            "manifest" => {
                for a in &el.attrs {
                    match (a.res_id, a.name.as_str()) {
                        (_, "package") => meta.package = a.string.clone().unwrap_or_default(),
                        (_, "split") => meta.split = a.string.clone(),
                        (ATTR_VERSION_CODE, _) | (_, "versionCode") => meta.version_code = a.int().unwrap_or(0) as u64,
                        (ATTR_VERSION_NAME, _) | (_, "versionName") => meta.version_name = a.string.clone().unwrap_or_default(),
                        _ => {}
                    }
                }
            }
            "uses-sdk" => {
                for a in &el.attrs {
                    match (a.res_id, a.name.as_str()) {
                        (ATTR_MIN_SDK, _) | (_, "minSdkVersion") => meta.min_sdk = a.sdk(),
                        (ATTR_TARGET_SDK, _) | (_, "targetSdkVersion") => meta.target_sdk = a.sdk(),
                        _ => {}
                    }
                }
            }
            "application" => {
                for a in &el.attrs {
                    match (a.res_id, a.name.as_str()) {
                        (ATTR_LABEL, _) | (_, "label") => label = Some(a.clone()),
                        (ATTR_ICON, _) | (_, "icon") => icon = Some(a.clone()),
                        (ATTR_ROUND_ICON, _) | (_, "roundIcon") => round_icon = Some(a.clone()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if meta.package.is_empty() {
        return Err(bad("в манифесте нет имени пакета".into()));
    }

    let table = match read_entry(&mut zip, "resources.arsc", MAX_ARSC)? {
        Some(b) => Arsc::parse(&b).ok(),
        None => None,
    };
    meta.label = match &label {
        Some(a) if a.typ == T_REFERENCE => table.as_ref().and_then(|t| t.string(a.data, lang)),
        Some(a) => a.string.clone(),
        None => None,
    }
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());

    if let Some(t) = &table {
        for a in [icon, round_icon].into_iter().flatten() {
            if a.typ == T_REFERENCE {
                if let Some(i) = resolve_icon(&mut zip, t, a.data, 0) {
                    meta.icon = Some(i);
                    break;
                }
            }
        }
    }
    Ok(meta)
}

fn bad(m: String) -> Error {
    Error::Parse(m)
}

fn read_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str, max: u64) -> Result<Option<Vec<u8>>> {
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(bad(format!("{name}: {e}"))),
    };
    if f.size() > max {
        return Err(bad(format!("{name} слишком большой")));
    }
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).map_err(|e| bad(format!("{name}: {e}")))?;
    Ok(Some(buf))
}

/// Находит картинку для ресурса иконки. Адаптивные иконки (`.xml`)
/// раскрываются до слоя `foreground`; фон-цвет запоминается.
fn resolve_icon<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, t: &Arsc, id: u32, depth: u8) -> Option<Icon> {
    if depth > 4 {
        return None;
    }
    let mut vals = t.values(id);
    // Сначала растровые файлы наибольшей плотности, потом xml.
    vals.sort_by_key(|(cfg, v)| {
        let path = if v.typ == T_STRING { t.strings.get(v.data as usize).cloned().unwrap_or_default() } else { String::new() };
        let raster = is_raster(&path);
        let dens = match cfg.density {
            0xFFFE | 0xFFFF => 0,
            0 => 160,
            d => d as i32,
        };
        (std::cmp::Reverse(raster as i32), std::cmp::Reverse(dens))
    });
    for (_, v) in vals {
        match v.typ {
            T_REFERENCE => {
                if let Some(i) = resolve_icon(zip, t, v.data, depth + 1) {
                    return Some(i);
                }
            }
            T_STRING => {
                let path = t.strings.get(v.data as usize)?.clone();
                if is_raster(&path) {
                    if let Ok(Some(bytes)) = read_entry(zip, &path, MAX_ICON) {
                        return Some(Icon { mime: mime_of(&path).into(), bytes, background: None, adaptive: false });
                    }
                } else if path.ends_with(".xml") {
                    let Ok(Some(xml)) = read_entry(zip, &path, MAX_MANIFEST) else { continue };
                    let Ok(doc) = Axml::parse(&xml) else { continue };
                    let pick = |names: &[&str]| {
                        doc.elements.iter().find(|e| names.contains(&e.name.as_str())).and_then(|e| {
                            e.attrs.iter().find(|a| {
                                (a.res_id == ATTR_DRAWABLE || a.res_id == ATTR_SRC || a.name == "drawable" || a.name == "src")
                                    && (a.typ == T_REFERENCE || (T_COLOR_FIRST..=T_COLOR_LAST).contains(&a.typ))
                            })
                        })
                    };
                    let bg = pick(&["background"]).and_then(|a| color_of(t, a));
                    if let Some(fg) = pick(&["foreground", "bitmap", "inset", "item", "nine-patch"]) {
                        if fg.typ == T_REFERENCE {
                            if let Some(mut i) = resolve_icon(zip, t, fg.data, depth + 1) {
                                i.background = i.background.or(bg);
                                i.adaptive = true;
                                return Some(i);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn color_of(t: &Arsc, a: &Attr) -> Option<String> {
    let (typ, data) = if a.typ == T_REFERENCE {
        let v = t.values(a.data).into_iter().map(|(_, v)| v).find(|v| (T_COLOR_FIRST..=T_COLOR_LAST).contains(&v.typ))?;
        (v.typ, v.data)
    } else {
        (a.typ, a.data)
    };
    (T_COLOR_FIRST..=T_COLOR_LAST).contains(&typ).then(|| format!("#{:06X}", data & 0x00FF_FFFF))
}

fn is_raster(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.ends_with(".png") || l.ends_with(".webp") || l.ends_with(".jpg") || l.ends_with(".jpeg")
}

fn mime_of(p: &str) -> &'static str {
    let l = p.to_ascii_lowercase();
    if l.ends_with(".webp") {
        "image/webp"
    } else if l.ends_with(".jpg") || l.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    }
}

// ───────────────────────── низкоуровневое чтение ─────────────────────────

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Пул строк (ResStringPool): UTF-8 или UTF-16.
fn string_pool(b: &[u8], start: usize) -> Option<Vec<String>> {
    let header = u16_at(b, start + 2)? as usize;
    let count = u32_at(b, start + 8)? as usize;
    let flags = u32_at(b, start + 16)?;
    let strings_start = u32_at(b, start + 20)? as usize;
    let utf8 = flags & (1 << 8) != 0;
    if count > 4_000_000 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = u32_at(b, start + header + i * 4)? as usize;
        let mut p = start + strings_start + off;
        let s = if utf8 {
            // Длина в символах, затем длина в байтах (по 1–2 байта каждая).
            let skip_len = |p: &mut usize| -> Option<usize> {
                let a = *b.get(*p)? as usize;
                *p += 1;
                if a & 0x80 != 0 {
                    let c = *b.get(*p)? as usize;
                    *p += 1;
                    Some(((a & 0x7f) << 8) | c)
                } else {
                    Some(a)
                }
            };
            skip_len(&mut p)?;
            let n = skip_len(&mut p)?;
            String::from_utf8_lossy(b.get(p..p + n)?).into_owned()
        } else {
            let mut n = u16_at(b, p)? as usize;
            p += 2;
            if n & 0x8000 != 0 {
                n = ((n & 0x7fff) << 16) | u16_at(b, p)? as usize;
                p += 2;
            }
            let units: Vec<u16> = (0..n).map(|k| u16_at(b, p + k * 2)).collect::<Option<_>>()?;
            String::from_utf16_lossy(&units)
        };
        out.push(s);
    }
    Some(out)
}

#[derive(Debug, Clone)]
struct Attr {
    name: String,
    res_id: u32,
    typ: u8,
    data: u32,
    string: Option<String>,
}

impl Attr {
    fn int(&self) -> Option<i64> {
        match self.typ {
            T_INT_DEC | T_INT_HEX => Some(self.data as i64),
            T_STRING => self.string.as_deref()?.trim().parse().ok(),
            _ => None,
        }
    }

    /// minSdkVersion бывает числом или кодовым именем предварительной версии.
    fn sdk(&self) -> Option<u32> {
        self.int().map(|v| v as u32)
    }
}

#[derive(Debug)]
struct Element {
    name: String,
    attrs: Vec<Attr>,
}

/// Бинарный XML Android.
struct Axml {
    elements: Vec<Element>,
}

impl Axml {
    fn parse(b: &[u8]) -> Result<Self> {
        let err = || bad("повреждённый бинарный XML".into());
        if u16_at(b, 0) != Some(0x0003) {
            return Err(err());
        }
        let mut pos = u16_at(b, 2).ok_or_else(err)? as usize;
        let mut strings: Vec<String> = Vec::new();
        let mut res_map: Vec<u32> = Vec::new();
        let mut elements = Vec::new();
        while pos + 8 <= b.len() {
            let typ = u16_at(b, pos).ok_or_else(err)?;
            let size = u32_at(b, pos + 4).ok_or_else(err)? as usize;
            if size < 8 || pos + size > b.len() {
                break;
            }
            match typ {
                0x0001 => strings = string_pool(b, pos).ok_or_else(err)?,
                0x0180 => {
                    let header = u16_at(b, pos + 2).ok_or_else(err)? as usize;
                    res_map = (pos + header..pos + size).step_by(4).filter_map(|o| u32_at(b, o)).collect();
                }
                0x0102 => {
                    let header = u16_at(b, pos + 2).ok_or_else(err)? as usize;
                    let ext = pos + header;
                    let name_idx = u32_at(b, ext + 4).ok_or_else(err)? as usize;
                    let attr_start = u16_at(b, ext + 8).ok_or_else(err)? as usize;
                    let attr_size = u16_at(b, ext + 10).ok_or_else(err)? as usize;
                    let count = u16_at(b, ext + 12).ok_or_else(err)? as usize;
                    let mut attrs = Vec::with_capacity(count);
                    for i in 0..count {
                        let a = ext + attr_start + i * attr_size;
                        let name_i = u32_at(b, a + 4).ok_or_else(err)? as usize;
                        let raw = u32_at(b, a + 8).ok_or_else(err)?;
                        let typ = *b.get(a + 15).ok_or_else(err)?;
                        let data = u32_at(b, a + 16).ok_or_else(err)?;
                        let string = if typ == T_STRING {
                            strings.get(data as usize).cloned()
                        } else if raw != u32::MAX {
                            strings.get(raw as usize).cloned()
                        } else {
                            None
                        };
                        attrs.push(Attr {
                            name: strings.get(name_i).cloned().unwrap_or_default(),
                            res_id: res_map.get(name_i).copied().unwrap_or(0),
                            typ,
                            data,
                            string,
                        });
                    }
                    elements.push(Element { name: strings.get(name_idx).cloned().unwrap_or_default(), attrs });
                }
                _ => {}
            }
            pos += size;
        }
        Ok(Self { elements })
    }
}

#[derive(Debug, Clone, Copy)]
struct Config {
    language: [u8; 2],
    density: u16,
}

#[derive(Debug, Clone, Copy)]
struct Value {
    typ: u8,
    data: u32,
}

/// (пакет, тип) → конфигурации с записями.
type TypeTable = HashMap<(u8, u8), Vec<(Config, HashMap<u16, Value>)>>;

/// Таблица ресурсов: только простые значения, без стилей и «мешков».
struct Arsc {
    strings: Vec<String>,
    /// (пакет, тип) → список (конфигурация, индекс записи → значение).
    types: TypeTable,
}

impl Arsc {
    fn parse(b: &[u8]) -> Result<Self> {
        let err = || bad("повреждённый resources.arsc".into());
        if u16_at(b, 0) != Some(0x0002) {
            return Err(err());
        }
        let mut pos = u16_at(b, 2).ok_or_else(err)? as usize;
        let mut strings = Vec::new();
        let mut types: TypeTable = HashMap::new();
        while pos + 8 <= b.len() {
            let typ = u16_at(b, pos).ok_or_else(err)?;
            let size = u32_at(b, pos + 4).ok_or_else(err)? as usize;
            if size < 8 || pos + size > b.len() {
                break;
            }
            match typ {
                0x0001 => strings = string_pool(b, pos).ok_or_else(err)?,
                0x0200 => Self::package(b, pos, size, &mut types).ok_or_else(err)?,
                _ => {}
            }
            pos += size;
        }
        Ok(Self { strings, types })
    }

    fn package(
        b: &[u8],
        start: usize,
        size: usize,
        types: &mut TypeTable,
    ) -> Option<()> {
        let pkg_id = u32_at(b, start + 8)? as u8;
        let header = u16_at(b, start + 2)? as usize;
        let end = start + size;
        let mut pos = start + header;
        while pos + 8 <= end {
            let typ = u16_at(b, pos)?;
            let csize = u32_at(b, pos + 4)? as usize;
            if csize < 8 || pos + csize > end {
                break;
            }
            if typ == 0x0201 {
                let id = *b.get(pos + 8)?;
                let flags = *b.get(pos + 9)?;
                let count = u32_at(b, pos + 12)? as usize;
                let entries_start = u32_at(b, pos + 16)? as usize;
                let cfg_at = pos + 20;
                let cfg = Config {
                    language: [*b.get(cfg_at + 8)?, *b.get(cfg_at + 9)?],
                    density: u16_at(b, cfg_at + 14)?,
                };
                let hheader = u16_at(b, pos + 2)? as usize;
                let offs = pos + hheader;
                let sparse = flags & 0x01 != 0;
                let off16 = flags & 0x02 != 0;
                let mut map = HashMap::new();
                for i in 0..count.min(65536) {
                    let (idx, off) = if sparse {
                        (u16_at(b, offs + i * 4)?, u16_at(b, offs + i * 4 + 2)? as usize * 4)
                    } else if off16 {
                        let o = u16_at(b, offs + i * 2)?;
                        if o == 0xFFFF {
                            continue;
                        }
                        (i as u16, o as usize * 4)
                    } else {
                        let o = u32_at(b, offs + i * 4)?;
                        if o == u32::MAX {
                            continue;
                        }
                        (i as u16, o as usize)
                    };
                    let e = pos + entries_start + off;
                    let esize = u16_at(b, e)? as usize;
                    let eflags = u16_at(b, e + 2)?;
                    let v = if eflags & 0x0008 != 0 {
                        // Компактная запись: тип в старшем байте флагов, данные сразу.
                        Value { typ: (eflags >> 8) as u8, data: u32_at(b, e + 4)? }
                    } else if eflags & 0x0001 != 0 {
                        continue;
                    } else {
                        Value { typ: *b.get(e + esize + 3)?, data: u32_at(b, e + esize + 4)? }
                    };
                    map.insert(idx, v);
                }
                types.entry((pkg_id, id)).or_default().push((cfg, map));
            }
            pos += csize;
        }
        Some(())
    }

    fn values(&self, id: u32) -> Vec<(Config, Value)> {
        let key = ((id >> 24) as u8, ((id >> 16) & 0xFF) as u8);
        let entry = (id & 0xFFFF) as u16;
        self.types
            .get(&key)
            .map(|list| list.iter().filter_map(|(c, m)| m.get(&entry).map(|v| (*c, *v))).collect())
            .unwrap_or_default()
    }

    /// Строка ресурса: нужный язык → без языка → английский → любая.
    fn string(&self, id: u32, lang: &str, ) -> Option<String> {
        self.string_depth(id, lang, 0)
    }

    fn string_depth(&self, id: u32, lang: &str, depth: u8) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let vals = self.values(id);
        let l = lang.as_bytes();
        let rank = |c: &Config| {
            if l.len() == 2 && c.language == [l[0], l[1]] {
                0
            } else if c.language == [0, 0] {
                1
            } else if &c.language == b"en" {
                2
            } else {
                3
            }
        };
        let mut vals: Vec<_> = vals.into_iter().collect();
        vals.sort_by_key(|(c, _)| rank(c));
        for (_, v) in vals {
            match v.typ {
                T_STRING => return self.strings.get(v.data as usize).cloned(),
                T_REFERENCE => {
                    if let Some(s) = self.string_depth(v.data, lang, depth + 1) {
                        return Some(s);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

/// Base64 без внешних зависимостей (для data:-ссылок на иконки).
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Номер Android по уровню API — для понятных сообщений.
pub fn android_version(sdk: u32) -> String {
    let v = match sdk {
        0..=20 => "4.4",
        21 => "5.0",
        22 => "5.1",
        23 => "6",
        24 => "7.0",
        25 => "7.1",
        26 => "8.0",
        27 => "8.1",
        28 => "9",
        29 => "10",
        30 => "11",
        31 => "12",
        32 => "12L",
        33 => "13",
        34 => "14",
        35 => "15",
        36 => "16",
        _ => return format!("API {sdk}"),
    };
    v.to_string()
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Сборщик крошечных APK для тестов: бинарный манифест и resources.arsc.
    use std::io::{Cursor, Write};

    fn pool(strings: &[&str]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offs = Vec::new();
        for s in strings {
            offs.push(data.len() as u32);
            let u: Vec<u16> = s.encode_utf16().collect();
            data.extend((u.len() as u16).to_le_bytes());
            for c in &u {
                data.extend(c.to_le_bytes());
            }
            data.extend([0, 0]);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let header = 28usize;
        let strings_start = header + offs.len() * 4;
        let size = strings_start + data.len();
        let mut b = Vec::new();
        b.extend(1u16.to_le_bytes());
        b.extend((header as u16).to_le_bytes());
        b.extend((size as u32).to_le_bytes());
        b.extend((strings.len() as u32).to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(0u32.to_le_bytes()); // UTF-16
        b.extend((strings_start as u32).to_le_bytes());
        b.extend(0u32.to_le_bytes());
        for o in offs {
            b.extend(o.to_le_bytes());
        }
        b.extend(data);
        b
    }

    /// Атрибут: (индекс имени, тип, данные, индекс строки или MAX).
    pub type A = (u32, u8, u32, u32);

    /// Бинарный XML: `strings` — пул, первые `res_ids.len()` строк — имена атрибутов android:*.
    pub fn axml(strings: &[&str], res_ids: &[u32], elements: &[(u32, Vec<A>)]) -> Vec<u8> {
        let mut body = pool(strings);
        let mut rm = Vec::new();
        rm.extend(0x0180u16.to_le_bytes());
        rm.extend(8u16.to_le_bytes());
        rm.extend(((8 + res_ids.len() * 4) as u32).to_le_bytes());
        for r in res_ids {
            rm.extend(r.to_le_bytes());
        }
        body.extend(rm);
        for (name, attrs) in elements {
            let size = 16 + 20 + attrs.len() * 20;
            body.extend(0x0102u16.to_le_bytes());
            body.extend(16u16.to_le_bytes());
            body.extend((size as u32).to_le_bytes());
            body.extend(1u32.to_le_bytes());
            body.extend(u32::MAX.to_le_bytes());
            body.extend(u32::MAX.to_le_bytes()); // ns
            body.extend(name.to_le_bytes());
            body.extend(20u16.to_le_bytes());
            body.extend(20u16.to_le_bytes());
            body.extend((attrs.len() as u16).to_le_bytes());
            body.extend([0u8; 6]);
            for (n, typ, data, raw) in attrs {
                body.extend(u32::MAX.to_le_bytes());
                body.extend(n.to_le_bytes());
                body.extend(raw.to_le_bytes());
                body.extend(8u16.to_le_bytes());
                body.push(0);
                body.push(*typ);
                body.extend(data.to_le_bytes());
            }
        }
        let mut b = Vec::new();
        b.extend(3u16.to_le_bytes());
        b.extend(8u16.to_le_bytes());
        b.extend(((8 + body.len()) as u32).to_le_bytes());
        b.extend(body);
        b
    }

    /// Тип ресурса: id, (язык, плотность), записи (тип, данные).
    pub type T = (u8, [u8; 2], u16, Vec<(u8, u32)>);

    pub fn arsc(global: &[&str], types: &[T]) -> Vec<u8> {
        let mut chunks = Vec::new();
        chunks.extend(pool(&["mipmap", "string", "color"]));
        chunks.extend(pool(&["k"]));
        for (id, lang, dens, entries) in types {
            let header = 20 + 64;
            let offs_len = entries.len() * 4;
            let entries_start = header + offs_len;
            let size = entries_start + entries.len() * 16;
            chunks.extend(0x0201u16.to_le_bytes());
            chunks.extend((header as u16).to_le_bytes());
            chunks.extend((size as u32).to_le_bytes());
            chunks.push(*id);
            chunks.push(0);
            chunks.extend(0u16.to_le_bytes());
            chunks.extend((entries.len() as u32).to_le_bytes());
            chunks.extend((entries_start as u32).to_le_bytes());
            let mut cfg = vec![0u8; 64];
            cfg[0..4].copy_from_slice(&64u32.to_le_bytes());
            cfg[8] = lang[0];
            cfg[9] = lang[1];
            cfg[14..16].copy_from_slice(&dens.to_le_bytes());
            chunks.extend(cfg);
            for (i, (typ, _)) in entries.iter().enumerate() {
                // Тип 0 — «нет записи в этой конфигурации».
                let off = if *typ == 0 { u32::MAX } else { (i * 16) as u32 };
                chunks.extend(off.to_le_bytes());
            }
            for (typ, data) in entries {
                chunks.extend(8u16.to_le_bytes());
                chunks.extend(0u16.to_le_bytes());
                chunks.extend(0u32.to_le_bytes());
                chunks.extend(8u16.to_le_bytes());
                chunks.push(0);
                chunks.push(*typ);
                chunks.extend(data.to_le_bytes());
            }
        }
        let pkg_header = 288usize;
        let mut pkg = Vec::new();
        pkg.extend(0x0200u16.to_le_bytes());
        pkg.extend((pkg_header as u16).to_le_bytes());
        pkg.extend(((pkg_header + chunks.len()) as u32).to_le_bytes());
        pkg.extend(0x7fu32.to_le_bytes());
        pkg.extend(vec![0u8; 256]);
        pkg.extend([0u8; 20]);
        pkg.extend(chunks);
        let gp = pool(global);
        let mut b = Vec::new();
        b.extend(2u16.to_le_bytes());
        b.extend(12u16.to_le_bytes());
        b.extend(((12 + gp.len() + pkg.len()) as u32).to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(gp);
        b.extend(pkg);
        b
    }

    pub fn zip(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            for (name, data) in files {
                w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    /// APK «com.example.demo» 2.5 (25), minSdk 24, название en/ru, иконки mdpi и xxxhdpi.
    pub fn demo_apk(abi: Option<&str>) -> Vec<u8> {
        demo_apk_padded(abi, 0)
    }

    /// То же плюс `pad` байт несжимаемых данных в начале архива.
    pub fn demo_apk_padded(abi: Option<&str>, pad: usize) -> Vec<u8> {
        // Пул манифеста: имена атрибутов, затем прочие строки.
        let s = [
            "versionCode", "versionName", "minSdkVersion", "label", "icon", // 0..4 (android:*)
            "package", "manifest", "uses-sdk", "application", "com.example.demo", "2.5", // 5..10
        ];
        let ids = [0x0101_021b, 0x0101_021c, 0x0101_020c, 0x0101_0001, 0x0101_0002];
        let m = axml(
            &s,
            &ids,
            &[
                (6, vec![(5, 3, 9, 9), (0, 0x10, 25, u32::MAX), (1, 3, 10, 10)]),
                (7, vec![(2, 0x10, 24, u32::MAX)]),
                (8, vec![(3, 1, 0x7f02_0000, u32::MAX), (4, 1, 0x7f01_0000, u32::MAX)]),
            ],
        );
        let global = ["res/mipmap-mdpi/ic.png", "res/mipmap-xxxhdpi/ic.png", "Demo App", "Демо"];
        let r = arsc(
            &global,
            &[
                (1, [0, 0], 160, vec![(3, 0)]),
                (1, [0, 0], 640, vec![(3, 1)]),
                (2, [0, 0], 0, vec![(3, 2)]),
                (2, *b"ru", 0, vec![(3, 3)]),
            ],
        );
        let mut files = vec![
            ("AndroidManifest.xml", m),
            ("resources.arsc", r),
            ("res/mipmap-mdpi/ic.png", b"small".to_vec()),
            ("res/mipmap-xxxhdpi/ic.png", b"\x89PNGbig".to_vec()),
        ];
        if let Some(a) = abi {
            files.push((Box::leak(format!("lib/{a}/libx.so").into_boxed_str()), vec![0]));
        }
        if pad > 0 {
            let mut x: u32 = 0x1234_5678;
            let noise = (0..pad)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            files.insert(0, ("assets/noise.bin", noise));
        }
        zip(&files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_manifest_label_and_icon() {
        let apk = testkit::demo_apk(Some("arm64-v8a"));
        let m = read_apk(Cursor::new(&apk), "ru").unwrap();
        assert_eq!(m.package, "com.example.demo");
        assert_eq!(m.version_code, 25);
        assert_eq!(m.version_name, "2.5");
        assert_eq!(m.min_sdk, Some(24));
        assert_eq!(m.label.as_deref(), Some("Демо"));
        assert_eq!(m.abis, vec!["arm64-v8a".to_string()]);
        let icon = m.icon.unwrap();
        assert_eq!(icon.bytes, b"\x89PNGbig");
        assert_eq!(icon.mime, "image/png");

        let en = read_apk(Cursor::new(&apk), "de").unwrap();
        assert_eq!(en.label.as_deref(), Some("Demo App"));
    }

    #[test]
    fn unwraps_adaptive_icon() {
        use testkit::*;
        let s = ["icon", "package", "manifest", "application", "com.example.adaptive"];
        let m = axml(
            &s,
            &[0x0101_0002],
            &[(2, vec![(1, 3, 4, 4)]), (3, vec![(0, 1, 0x7f01_0000, u32::MAX)])],
        );
        let xs = ["drawable", "adaptive-icon", "background", "foreground"];
        let ids = [0x0101_0199];
        let xml = axml(
            &xs,
            &ids,
            &[(1, vec![]), (2, vec![(0, 1, 0x7f03_0000, u32::MAX)]), (3, vec![(0, 1, 0x7f01_0001, u32::MAX)])],
        );
        let global = ["res/mipmap-anydpi-v26/ic.xml", "res/mipmap-xxhdpi/fg.webp"];
        let r = arsc(
            &global,
            &[
                (1, [0, 0], 0xFFFE, vec![(3, 0), (0, 0)]),
                (1, [0, 0], 480, vec![(0, 0), (3, 1)]),
                (3, [0, 0], 0, vec![(0x1c, 0xFF12_3456)]),
            ],
        );
        let apk = zip(&[
            ("AndroidManifest.xml", m),
            ("resources.arsc", r),
            ("res/mipmap-anydpi-v26/ic.xml", xml),
            ("res/mipmap-xxhdpi/fg.webp", b"RIFFfg".to_vec()),
        ]);
        let meta = read_apk(Cursor::new(&apk), "ru").unwrap();
        assert_eq!(meta.package, "com.example.adaptive");
        let icon = meta.icon.unwrap();
        assert_eq!(icon.bytes, b"RIFFfg");
        assert_eq!(icon.mime, "image/webp");
        assert_eq!(icon.background.as_deref(), Some("#123456"));
        assert!(icon.adaptive);
    }

    #[test]
    fn rejects_garbage() {
        assert!(read_apk(Cursor::new(b"not a zip".to_vec()), "ru").is_err());
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
