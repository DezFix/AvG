//! Точки отката. Хранятся в одном JSON-файле в папке данных программы
//! и сохраняются после каждого изменения, чтобы откат работал даже
//! после сбоя посреди применения.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::engine::Change;
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: u64,
    /// Порядковый номер для этого телефона (№1, №2, …).
    pub number: u32,
    /// Unix-время создания, секунды.
    pub created: u64,
    pub serial: String,
    pub device: String,
    pub profile: String,
    pub profile_label: String,
    pub changes: Vec<Change>,
}

impl Snapshot {
    pub fn active_changes(&self) -> usize {
        self.changes.iter().filter(|c| !c.reverted && !c.revert.is_empty()).count()
    }
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    items: Vec<Snapshot>,
}

fn now() -> (u64, u64) {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (d.as_millis() as u64, d.as_secs())
}

impl Store {
    /// Загружает хранилище (или создаёт пустое, если файла ещё нет).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let items = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| Error::Other(format!("файл точек отката повреждён: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, items })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&self.items).map_err(|e| Error::Other(e.to_string()))?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn create(&mut self, serial: &str, device: &str, profile: &str, profile_label: &str) -> Result<u64> {
        let (mut id, secs) = now();
        while self.items.iter().any(|s| s.id == id) {
            id += 1;
        }
        let number = self
            .items
            .iter()
            .filter(|s| s.serial == serial)
            .map(|s| s.number)
            .max()
            .unwrap_or(0)
            + 1;
        self.items.push(Snapshot {
            id,
            number,
            created: secs,
            serial: serial.into(),
            device: device.into(),
            profile: profile.into(),
            profile_label: profile_label.into(),
            changes: Vec::new(),
        });
        self.save()?;
        Ok(id)
    }

    pub fn push(&mut self, id: u64, change: Change) -> Result<()> {
        let s = self.get_mut(id)?;
        s.changes.push(change);
        self.save()
    }

    pub fn set_reverted(&mut self, id: u64, index: usize) -> Result<()> {
        let s = self.get_mut(id)?;
        let c = s
            .changes
            .get_mut(index)
            .ok_or_else(|| Error::Other("изменение не найдено".into()))?;
        c.reverted = true;
        self.save()
    }

    /// Удаляет пустую точку (если в ней так ничего и не изменилось).
    pub fn drop_if_empty(&mut self, id: u64) -> Result<()> {
        let before = self.items.len();
        self.items.retain(|s| !(s.id == id && s.changes.is_empty()));
        if self.items.len() != before {
            self.save()?;
        }
        Ok(())
    }

    pub fn get(&self, id: u64) -> Result<&Snapshot> {
        self.items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::Other("точка отката не найдена".into()))
    }

    fn get_mut(&mut self, id: u64) -> Result<&mut Snapshot> {
        self.items
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::Other("точка отката не найдена".into()))
    }

    /// Точки отката телефона (или все), новые сверху.
    pub fn list(&self, serial: Option<&str>) -> Vec<Snapshot> {
        let mut v: Vec<Snapshot> = self
            .items
            .iter()
            .filter(|s| serial.is_none_or(|x| s.serial == x))
            .cloned()
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.id));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(id: &str) -> Change {
        Change {
            tweak_id: id.into(),
            title: id.into(),
            ok: true,
            revert: vec!["x".into()],
            reverted: false,
            note: String::new(),
        }
    }

    #[test]
    fn persists_and_numbers() {
        let dir = std::env::temp_dir().join(format!("avg-test-{}", std::process::id()));
        let path = dir.join("snapshots.json");
        let _ = std::fs::remove_file(&path);
        let mut st = Store::open(&path).unwrap();
        let a = st.create("S1", "Phone", "weak", "Слабый телефон").unwrap();
        st.push(a, change("t1")).unwrap();
        let b = st.create("S1", "Phone", "battery", "Батарея").unwrap();
        let c = st.create("S2", "Other", "weak", "Слабый телефон").unwrap();
        st.drop_if_empty(c).unwrap();
        st.set_reverted(a, 0).unwrap();

        let st2 = Store::open(&path).unwrap();
        let list = st2.list(Some("S1"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, b);
        assert_eq!(list[0].number, 2);
        assert!(list[1].changes[0].reverted);
        assert!(st2.list(Some("S2")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
