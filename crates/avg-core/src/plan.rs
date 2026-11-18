//! Построение плана: какие твики подходят этому телефону и что именно
//! будет выполнено.

use serde::Serialize;

use crate::scan::Scan;
use crate::tweaks::{profile_label, Action, Risk, Tweak, PROFILES};

#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub risk: Risk,
    /// Отмечен ли галочкой по умолчанию.
    pub selected: bool,
    pub reversible: bool,
    /// Пакеты, которые реально есть на телефоне (для удаления/отключения).
    pub packages: Vec<String>,
    /// Команды, которые будут выполнены (для показа пользователю).
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileSummary {
    pub id: String,
    pub label: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategorySummary {
    pub category: String,
    pub count: usize,
    pub max_risk: Risk,
}

/// Подходит ли твик этому телефону по производителю и версии Android.
pub fn matches_device(t: &Tweak, scan: &Scan) -> bool {
    let info = &scan.info;
    if !t.vendors.is_empty() && !t.vendors.iter().any(|v| v == &info.vendor) {
        return false;
    }
    if let Some(min) = t.min_sdk {
        if info.sdk != 0 && info.sdk < min {
            return false;
        }
    }
    if let Some(max) = t.max_sdk {
        if info.sdk > max {
            return false;
        }
    }
    true
}

/// Возвращает пункт плана, если твик применим и ещё не применён.
pub fn evaluate(t: &Tweak, scan: &Scan) -> Option<PlanItem> {
    if !matches_device(t, scan) {
        return None;
    }
    let mut packages = Vec::new();
    let commands: Vec<String> = match &t.action {
        Action::RemovePackages { packages: list } => {
            packages = list.iter().filter(|p| scan.installed.contains(*p)).cloned().collect();
            packages.iter().map(|p| format!("pm uninstall -k --user 0 {p}")).collect()
        }
        Action::DisablePackages { packages: list } => {
            packages = list
                .iter()
                .filter(|p| scan.installed.contains(*p) && !scan.disabled.contains(*p))
                .cloned()
                .collect();
            packages.iter().map(|p| format!("pm disable-user --user 0 {p}")).collect()
        }
        Action::Settings { values } => {
            let changed: Vec<_> = values
                .iter()
                .filter(|v| scan.settings.get(&v.id()).map(String::as_str) != Some(v.value.as_str()))
                .collect();
            if changed.is_empty() {
                return None;
            }
            values
                .iter()
                .map(|v| format!("settings put {} {} {}", v.namespace.as_str(), v.key, v.value))
                .collect()
        }
        Action::Commands { apply, .. } => apply.clone(),
        Action::ScreenScale { factor } => {
            if scan.wm_overridden {
                return None;
            }
            vec![format!("wm size ×{factor}"), format!("wm density ×{factor}")]
        }
    };
    if commands.is_empty() {
        return None;
    }
    Some(PlanItem {
        id: t.id.clone(),
        title: t.title.clone(),
        description: t.description.clone(),
        category: t.category.clone(),
        risk: t.risk,
        selected: t.default_on && t.risk != Risk::High,
        reversible: t.reversible(),
        packages,
        commands,
    })
}

/// План для профиля. `custom` — все применимые твики.
pub fn build(tweaks: &[Tweak], scan: &Scan, profile: &str) -> Vec<PlanItem> {
    let mut items: Vec<PlanItem> = tweaks
        .iter()
        .filter(|t| profile == "custom" || t.profiles.iter().any(|p| p == profile))
        .filter_map(|t| evaluate(t, scan))
        .collect();
    let order = |c: &str| crate::tweaks::CATEGORIES.iter().position(|x| *x == c).unwrap_or(99);
    items.sort_by(|a, b| order(&a.category).cmp(&order(&b.category)).then(a.risk.cmp(&b.risk)));
    items
}

/// Сколько улучшений найдено в каждом профиле (для главного экрана).
pub fn profiles(tweaks: &[Tweak], scan: &Scan) -> Vec<ProfileSummary> {
    PROFILES
        .iter()
        .map(|(id, _)| ProfileSummary {
            id: id.to_string(),
            label: profile_label(id).to_string(),
            count: build(tweaks, scan, id).len(),
        })
        .collect()
}

/// Сводка по категориям для блока «Что нашёл сканер».
pub fn categories(items: &[PlanItem]) -> Vec<CategorySummary> {
    let mut out: Vec<CategorySummary> = Vec::new();
    for it in items {
        let n = it.packages.len().max(1);
        match out.iter_mut().find(|c| c.category == it.category) {
            Some(c) => {
                c.count += n;
                c.max_risk = c.max_risk.max(it.risk);
            }
            None => out.push(CategorySummary { category: it.category.clone(), count: n, max_risk: it.risk }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweaks;

    fn xiaomi_scan() -> Scan {
        let mut s = Scan::default();
        s.info.vendor = "xiaomi".into();
        s.info.sdk = 30;
        for p in ["com.miui.msa.global", "com.miui.analytics", "com.facebook.services"] {
            s.installed.insert(p.into());
        }
        s.settings.insert("global/window_animation_scale".into(), "1.0".into());
        s.settings.insert("global/wifi_scan_always_enabled".into(), "0".into());
        s
    }

    #[test]
    fn plan_for_xiaomi() {
        let t = tweaks::builtin().unwrap();
        let s = xiaomi_scan();
        let plan = build(&t, &s, "weak");
        let ids: Vec<&str> = plan.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"xiaomi.msa"));
        assert!(ids.contains(&"partners.facebook"));
        assert!(ids.contains(&"general.animations"));
        // Wi-Fi-поиск уже выключен — пункта быть не должно.
        assert!(!ids.contains(&"general.wifi_scan"));
        // Samsung-твики не попадают на Xiaomi.
        assert!(!ids.iter().any(|i| i.starts_with("samsung.")));
        // Только реально установленные пакеты.
        let msa = plan.iter().find(|p| p.id == "xiaomi.msa").unwrap();
        assert_eq!(msa.packages, vec!["com.miui.msa.global".to_string()]);
        // Рискованное не выбрано по умолчанию.
        let scale = plan.iter().find(|p| p.id == "general.screen_scale").unwrap();
        assert!(!scale.selected);
    }

    #[test]
    fn custom_has_everything() {
        let t = tweaks::builtin().unwrap();
        let s = xiaomi_scan();
        assert!(build(&t, &s, "custom").len() >= build(&t, &s, "weak").len());
    }
}
