//! Оценка «можно ли удалить этот пакет».
//!
//! Порядок проверки: системно важные пакеты → установленные пользователем →
//! пакеты из встроенных твиков → ручной справочник → шаблоны имён.
//! Всё, что не подошло ни под одно правило, считается неизвестным.

use std::collections::HashMap;

use serde::Serialize;

use crate::packages::is_critical;
use crate::tweaks::{Action, Risk, Tweak};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Мусор: реклама, аналитика, партнёрские приложения, ваши приложения.
    Safe,
    /// Обычное встроенное приложение: удалить можно, если не пользуетесь.
    Optional,
    /// AvG не знает этот пакет.
    Unknown,
    /// Системный компонент: удаление может сломать функции телефона.
    Keep,
    /// Без него телефон не загрузится. Удаление заблокировано.
    Critical,
}

#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub level: Safety,
    pub reason: String,
}

const OVERLAY: &str = "Оверлей: меняет оформление или настройки системы. Не работает в фоне и почти не занимает места — удаление ничего не ускорит, а интерфейс может сломаться.";
const BLOAT: &str = "Предустановленный мусор: реклама, аналитика или партнёрское приложение. Можно удалить.";
const CORE: &str = "Системный компонент (связь, сеть, безопасность, звонки, хранилище…). Удаление может сломать функции телефона.";
const APP: &str = "Обычное встроенное приложение. Если не пользуетесь — можно удалить, система не пострадает.";
const UNKNOWN: &str = "AvG не знает этот пакет. Если сомневаетесь — не удаляйте, а отключите: это легко вернуть.";
const USER: &str = "Установлено вами или магазином. Удаление ничего в системе не сломает.";
const CRITICAL: &str = "Без этого пакета телефон может не загрузиться или перестать звонить.";

/// Ручной справочник: точное имя пакета → оценка.
const KNOWN: &[(&str, Safety, &str)] = &[
    // AOSP
    ("com.android.egg", Safety::Safe, "Пасхалка Android. Можно удалить."),
    ("com.android.dreams.basic", Safety::Safe, "Заставка экрана при зарядке. Можно удалить."),
    ("com.android.dreams.phototable", Safety::Safe, "Заставка экрана при зарядке. Можно удалить."),
    ("com.android.bookmarkprovider", Safety::Safe, "Закладки старого браузера Android. Можно удалить."),
    ("com.android.traceur", Safety::Optional, "Системная трассировка для разработчиков."),
    ("com.android.printspooler", Safety::Optional, "Печать с телефона. Нужна, только если вы печатаете."),
    ("com.android.bips", Safety::Optional, "Встроенная служба печати."),
    ("com.android.wallpaper.livepicker", Safety::Optional, "Выбор живых обоев."),
    ("com.android.wallpaperbackup", Safety::Optional, "Резервная копия обоев."),
    ("com.android.hotwordenrollment.okgoogle", Safety::Optional, "Распознавание «Окей, Google»."),
    ("com.android.hotwordenrollment.xgoogle", Safety::Optional, "Распознавание «Окей, Google»."),
    ("com.android.chrome", Safety::Optional, "Chrome. Удаляйте, только если есть другой браузер. На Android 7–9 он может работать как системный WebView."),
    ("com.android.stk", Safety::Keep, "SIM-меню оператора. Без него могут не работать услуги оператора."),
    ("com.android.mms", Safety::Keep, "Системные SMS. Удаляйте, только если пользуетесь другим приложением сообщений."),
    // Google
    ("com.google.android.feedback", Safety::Safe, "Отправка отзывов в Google. Можно удалить."),
    ("com.google.android.printservice.recommendation", Safety::Safe, "Подсказки служб печати. Можно удалить."),
    ("com.google.android.apps.tachyon", Safety::Optional, "Google Meet (бывший Duo)."),
    ("com.google.android.videos", Safety::Optional, "Google TV (Play Фильмы)."),
    ("com.google.android.apps.youtube.music", Safety::Optional, "YouTube Music."),
    ("com.google.android.youtube", Safety::Optional, "YouTube. Смотреть можно и в браузере."),
    ("com.google.android.apps.podcasts", Safety::Optional, "Google Подкасты."),
    ("com.google.android.apps.subscriptions.red", Safety::Optional, "Google One."),
    ("com.google.android.apps.wellbeing", Safety::Optional, "Цифровое благополучие и родительский контроль."),
    ("com.google.android.projection.gearhead", Safety::Optional, "Android Auto. Нужен, только если подключаете телефон к машине."),
    ("com.google.android.apps.googleassistant", Safety::Optional, "Google Ассистент."),
    ("com.google.android.marvin.talkback", Safety::Optional, "TalkBack — экранный диктор для людей с нарушением зрения."),
    ("com.google.android.tts", Safety::Optional, "Синтез речи Google: озвучка навигатора и диктора."),
    ("com.google.android.apps.turbo", Safety::Optional, "Device Health Services: адаптивная батарея и прогноз заряда."),
    ("com.google.android.apps.maps", Safety::Optional, "Google Карты."),
    ("com.google.android.apps.photos", Safety::Optional, "Google Фото. Перед удалением убедитесь, что фото сохранены."),
    ("com.google.android.apps.docs", Safety::Optional, "Google Диск."),
    ("com.google.android.gm", Safety::Optional, "Gmail."),
    ("com.google.android.keep", Safety::Optional, "Google Keep."),
    ("com.google.android.googlequicksearchbox", Safety::Optional, "Приложение Google и поиск на рабочем столе. На некоторых телефонах от него зависит Ассистент."),
    ("com.google.android.partnersetup", Safety::Keep, "Настройки Google от производителя телефона."),
    ("com.google.android.syncadapters.contacts", Safety::Keep, "Синхронизация контактов с Google."),
    ("com.google.android.syncadapters.calendar", Safety::Keep, "Синхронизация календаря с Google."),
    ("com.google.android.inputmethod.latin", Safety::Keep, "Клавиатура Gboard. Удаляйте, только если установлена другая клавиатура."),
    // OnePlus / OPPO / realme (ColorOS, OxygenOS)
    ("com.heytap.music", Safety::Optional, "Музыка HeyTap."),
    ("com.heytap.browser", Safety::Optional, "Браузер HeyTap."),
    ("com.heytap.market", Safety::Optional, "Магазин приложений OPPO/OnePlus."),
    ("com.heytap.cloud", Safety::Optional, "Облако HeyTap."),
    ("com.heytap.pictorial", Safety::Safe, "Журнал обоев на экране блокировки (с рекламой). Можно удалить."),
    ("com.oplus.statistics.rom", Safety::Safe, "Сбор статистики использования телефона. Можно удалить."),
    ("com.oplus.logkit", Safety::Optional, "Сбор логов для отчётов об ошибках."),
    ("com.oplus.games", Safety::Optional, "Игровое пространство."),
    ("com.coloros.gamespaceui", Safety::Optional, "Игровое пространство."),
    ("com.oneplus.gamespace", Safety::Optional, "Игровое пространство."),
    ("com.coloros.weather2", Safety::Optional, "Погода."),
    ("com.coloros.weather.service", Safety::Optional, "Служба погоды для виджета."),
    ("com.coloros.compass2", Safety::Optional, "Компас."),
    ("com.coloros.soundrecorder", Safety::Optional, "Диктофон."),
    ("com.coloros.calculator", Safety::Optional, "Калькулятор."),
    ("com.oneplus.calculator", Safety::Optional, "Калькулятор."),
    ("com.coloros.note", Safety::Optional, "Заметки."),
    ("com.oneplus.note", Safety::Optional, "Заметки."),
    ("com.coloros.gallery3d", Safety::Optional, "Галерея. Удаляйте, только если есть другая."),
    ("com.oneplus.gallery", Safety::Optional, "Галерея. Удаляйте, только если есть другая."),
    ("andes.oplus.documentsreader", Safety::Optional, "Просмотр документов."),
    // Партнёры
    ("com.netflix.partner.activation", Safety::Safe, "Активация Netflix от производителя. Можно удалить."),
    ("com.netflix.mediaclient", Safety::Optional, "Netflix."),
    ("com.microsoft.appmanager", Safety::Optional, "Связь с Windows."),
];

/// Части имени, которые выдают мусор.
const BLOAT_PARTS: &[&str] = &[
    "facebook", "analytics", "msa", "joyose", "bugreport", "feedback", "partner",
    "booking", "amazon", "linkedin", "glance", "yellowpage", "mipicks", "aliexpress",
    "ads", "adsdk", "advert", "tracking", "telemetry",
];

/// Подстроки, которые выдают системный компонент.
const CORE_SUBSTR: &[&str] = &[
    "framework", "telephony", "telecom", ".ims", "networkstack", "wifi", "bluetooth", "nfc",
    "provider", "permission", "keychain", "certinstaller", "webview", "launcher", "systemui",
    "biometric", "fingerprint", "faceunlock", "secur", "setupwizard", "provision", "carrier",
    "cellbroadcast", "emergency", "modem", "location", "gnss", "documentsui", "externalstorage",
    "mtp", "connectivity", "tethering", "vpn", "extservices", "ext.services", "ext.shared",
    "statsd", "adservices", "modulemetadata", "safetycenter", "uwb", "sdksandbox", "credential",
    "devicelock", "ondevicepersonalization", "wireless", "dialer", "incallui", "contacts",
    "inputmethod", "keyboard", "camera", "power", "battery", "thermal", "sensor", "display",
    "audio", "media", "update", "ota", "backup", "restore", "shell", "server", "gms", "gsf",
    "simcard", "simsettings", "callsettings", "mainline", "apex", "rkp", "healthconnect",
];

/// Отдельные сегменты имени, которые выдают системный компонент.
const CORE_SEGS: &[&str] = &["sim", "radio", "phone", "sau", "fused"];

/// Подстроки, которые выдают обычное приложение.
const APP_SUBSTR: &[&str] = &[
    "music", "video", "browser", "weather", "note", "calculator", "compass", "game", "gallery",
    "calendar", "deskclock", "recorder", "fmradio", "email", "mail", "market", "appstore",
    "wallet", "cloud", "theme", "reader", "translate", "scanner", "lens", "magnifier", "tips",
    "manual", "community", "forum", "livewallpaper", "youtube", "maps", "photos", "assistant",
    "podcast", "radio.fm", "voicememo", "filemanager", "fileexplorer", "health", "fitness",
];

fn is_overlay(p: &str) -> bool {
    p.contains("overlay") || p.contains("auto_generated") || segs(p).any(|s| s == "rro")
}

fn segs(p: &str) -> impl Iterator<Item = &str> {
    p.split(['.', '_'])
}

pub struct Rules {
    tweaks: HashMap<String, (Risk, String)>,
}

impl Rules {
    pub fn new(tweaks: &[Tweak]) -> Self {
        let mut map = HashMap::new();
        for t in tweaks {
            if let Action::RemovePackages { packages } | Action::DisablePackages { packages } = &t.action {
                for p in packages {
                    map.entry(p.clone())
                        .and_modify(|e: &mut (Risk, String)| {
                            if t.risk < e.0 {
                                *e = (t.risk, t.title.clone());
                            }
                        })
                        .or_insert((t.risk, t.title.clone()));
                }
            }
        }
        Rules { tweaks: map }
    }

    pub fn judge(&self, package: &str, system: bool) -> Verdict {
        let v = |level, reason: &str| Verdict { level, reason: reason.to_string() };
        if is_critical(package) {
            return v(Safety::Critical, CRITICAL);
        }
        if !system {
            return v(Safety::Safe, USER);
        }
        if let Some((risk, title)) = self.tweaks.get(package) {
            return match risk {
                Risk::Safe => v(Safety::Safe, &format!("Есть в наборах AvG («{title}»): известный мусор. Можно удалить.")),
                Risk::Medium => v(Safety::Optional, &format!("Есть в наборах AvG («{title}»). Удалить можно, если не пользуетесь.")),
                Risk::High => v(Safety::Keep, &format!("Есть в наборах AvG («{title}»), но с высоким риском. Удаляйте, только если понимаете последствия.")),
            };
        }
        if let Some((_, level, reason)) = KNOWN.iter().find(|(p, _, _)| *p == package) {
            return v(*level, reason);
        }
        let low = package.to_ascii_lowercase();
        if is_overlay(&low) {
            return v(Safety::Keep, OVERLAY);
        }
        if segs(&low).any(|s| BLOAT_PARTS.contains(&s)) || low.starts_with("com.facebook.") {
            return v(Safety::Safe, BLOAT);
        }
        if CORE_SUBSTR.iter().any(|k| low.contains(k)) || segs(&low).skip(1).any(|s| CORE_SEGS.contains(&s)) {
            return v(Safety::Keep, CORE);
        }
        if APP_SUBSTR.iter().any(|k| low.contains(k)) {
            return v(Safety::Optional, APP);
        }
        v(Safety::Unknown, UNKNOWN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lvl(p: &str) -> Safety {
        Rules::new(&[]).judge(p, true).level
    }

    #[test]
    fn rates_packages() {
        assert_eq!(lvl("com.android.systemui"), Safety::Critical);
        assert_eq!(Rules::new(&[]).judge("org.telegram.messenger", false).level, Safety::Safe);
        assert_eq!(lvl("android.frameworkres.overlay.display.product"), Safety::Keep);
        assert_eq!(lvl("android.auto_generated_characteristics_rro"), Safety::Keep);
        assert_eq!(lvl("com.facebook.services"), Safety::Safe);
        assert_eq!(lvl("com.miui.msa.global"), Safety::Safe);
        assert_eq!(lvl("com.android.bluetooth"), Safety::Keep);
        assert_eq!(lvl("com.oplus.wirelesssettings"), Safety::Keep);
        assert_eq!(lvl("com.coloros.weather2"), Safety::Optional);
        assert_eq!(lvl("com.example.musicbox"), Safety::Optional);
        assert_eq!(lvl("com.oplus.xyzzy"), Safety::Unknown);
        // «downloads» не должно считаться рекламой (ads).
        assert_ne!(lvl("com.vendor.downloads"), Safety::Safe);
    }
}
