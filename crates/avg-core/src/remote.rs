//! Файл на телефоне как `Read + Seek`: читается блоками через
//! `adb exec-out dd`, поэтому из APK можно достать манифест и иконку,
//! не скачивая его целиком.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

use crate::adb::{ExecOut, Shell};
use crate::{Error, Result};

const BLOCK: u64 = 256 * 1024;

/// Путь безопасен для подстановки в shell внутри одинарных кавычек.
pub fn is_safe_path(p: &str) -> bool {
    p.starts_with('/')
        && p.len() < 1024
        && !p.contains("..")
        && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~=+,@".contains(c))
}

pub struct RemoteFile<'a, D: ExecOut + ?Sized> {
    dev: &'a D,
    path: String,
    size: u64,
    pos: u64,
    cache: HashMap<u64, Vec<u8>>,
    /// Сколько раз ходили на телефон — для тестов и отладки.
    pub fetches: usize,
}

impl<'a, D: ExecOut + Shell + ?Sized> RemoteFile<'a, D> {
    pub fn open(dev: &'a D, path: &str) -> Result<Self> {
        if !is_safe_path(path) {
            return Err(Error::Other(format!("недопустимый путь: {path}")));
        }
        let out = dev.shell(&format!("stat -c %s '{path}'"))?;
        let size = out
            .stdout
            .trim()
            .parse::<u64>()
            .map_err(|_| Error::Adb(format!("не удалось узнать размер {path}: {}", out.text())))?;
        Ok(Self { dev, path: path.to_string(), size, pos: 0, cache: HashMap::new(), fetches: 0 })
    }
}

impl<D: ExecOut + ?Sized> RemoteFile<'_, D> {
    pub fn size(&self) -> u64 {
        self.size
    }

    fn block(&mut self, idx: u64) -> io::Result<&[u8]> {
        if !self.cache.contains_key(&idx) {
            let cmd = format!("dd if='{}' bs={BLOCK} skip={idx} count=1 2>/dev/null", self.path);
            let data = self.dev.exec_out(&cmd).map_err(|e| io::Error::other(e.to_string()))?;
            self.fetches += 1;
            // Ограничиваем память: на один APK хватает и сотни блоков.
            if self.cache.len() > 512 {
                self.cache.clear();
            }
            self.cache.insert(idx, data);
        }
        Ok(&self.cache[&idx])
    }
}

impl<D: ExecOut + ?Sized> Read for RemoteFile<'_, D> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let idx = self.pos / BLOCK;
        let off = (self.pos % BLOCK) as usize;
        let pos = self.pos;
        let size = self.size;
        let data = self.block(idx)?;
        if off >= data.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "файл на телефоне короче ожидаемого"));
        }
        let n = buf.len().min(data.len() - off).min((size - pos) as usize);
        buf[..n].copy_from_slice(&data[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<D: ExecOut + ?Sized> Seek for RemoteFile<'_, D> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.size as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if p < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "позиция до начала файла"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adb::ShellOutput;
    use std::io::Cursor;

    /// «Телефон», у которого есть один файл.
    struct Fake(Vec<u8>);

    impl Shell for Fake {
        fn shell(&self, c: &str) -> Result<ShellOutput> {
            assert!(c.starts_with("stat -c %s"));
            Ok(ShellOutput { stdout: format!("{}\n", self.0.len()), ..Default::default() })
        }
    }

    impl ExecOut for Fake {
        fn exec_out(&self, c: &str) -> Result<Vec<u8>> {
            let skip: u64 = c.split("skip=").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
            let s = (skip * BLOCK) as usize;
            Ok(self.0.get(s..(s + BLOCK as usize).min(self.0.len())).unwrap_or_default().to_vec())
        }
    }

    #[test]
    fn reads_apk_from_phone_in_blocks() {
        // APK больше трёх блоков: манифест и иконка лежат в последнем.
        let apk = crate::apk::testkit::demo_apk_padded(None, 3 * BLOCK as usize);
        assert!(apk.len() as u64 > 3 * BLOCK);
        let dev = Fake(apk.clone());
        let mut f = RemoteFile::open(&dev, "/data/app/~~x==/com.example-1/base.apk").unwrap();
        let m = crate::apk::read_apk(&mut f, "ru").unwrap();
        assert_eq!(m.package, "com.example.demo");
        assert_eq!(m.label.as_deref(), Some("Демо"));
        // Блоки с «шумом» в начале архива читать не нужно.
        assert!(f.fetches <= 2, "лишние чтения с телефона: {}", f.fetches);

        let mut whole = Vec::new();
        let mut f = RemoteFile::open(&dev, "/data/app/x/base.apk").unwrap();
        f.read_to_end(&mut whole).unwrap();
        assert_eq!(whole, apk);
        assert!(crate::apk::read_apk(Cursor::new(whole), "ru").is_ok());
    }

    #[test]
    fn rejects_unsafe_paths() {
        assert!(is_safe_path("/data/app/~~AbC==/com.foo-1/base.apk"));
        assert!(!is_safe_path("/data/app/x'; rm -rf /;'"));
        assert!(!is_safe_path("relative/base.apk"));
        assert!(!is_safe_path("/data/../etc"));
    }
}
