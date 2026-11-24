//! Finding SD cards and copying archives onto them.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::{Error, Progress, Result, Stage};

/// A mounted removable drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    pub name: String,
    pub mount: PathBuf,
    pub free: u64,
    pub total: u64,
    pub file_system: String,
}

/// The largest file FAT32 can hold.
pub const FAT32_MAX: u64 = (4 << 30) - 1;

/// The folder at the top of the card where the firmware looks for archives.
pub const ARCHIVE_FOLDER: &str = "wiki";

/// Where archives go on a card, or on a folder standing for one: its
/// [`ARCHIVE_FOLDER`], or `card` itself if it already is that folder.
pub fn archive_dir(card: &Path) -> PathBuf {
    if card
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case(ARCHIVE_FOLDER))
    {
        card.to_path_buf()
    } else {
        card.join(ARCHIVE_FOLDER)
    }
}

impl Drive {
    pub fn is_fat32(&self) -> bool {
        matches!(
            self.file_system.to_ascii_lowercase().as_str(),
            "vfat" | "fat32" | "fat" | "fat16" | "msdos"
        )
    }

    /// "SDCARD (E:\) · 12.3 GB free".
    pub fn label(&self) -> String {
        let name = if self.name.is_empty() {
            "Removable drive"
        } else {
            &self.name
        };
        format!(
            "{name} ({}) · {} free",
            self.mount.display(),
            crate::size_text(self.free)
        )
    }
}

/// Free space on the drive holding `path` (a folder or file, which need
/// not exist yet), if it can be told.
pub fn free_space(path: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    let path = std::path::absolute(path).ok()?;
    // Windows compares drive letters in any case.
    let key = |p: &Path| {
        let s = p.to_string_lossy().into_owned();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let path = PathBuf::from(key(&path));
    disks
        .iter()
        .filter(|d| path.starts_with(key(d.mount_point())))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

/// Removable drives that can be written to, such as SD cards in a reader.
pub fn removable() -> Vec<Drive> {
    let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() else {
        return Vec::new();
    };
    let mut out: Vec<Drive> = disks
        .iter()
        .filter(|d| d.is_removable() && !d.is_read_only())
        .map(|d| Drive {
            name: d.name().to_string_lossy().into_owned(),
            mount: d.mount_point().to_path_buf(),
            free: d.available_space(),
            total: d.total_space(),
            file_system: d.file_system().to_string_lossy().into_owned(),
        })
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out.dedup_by(|a, b| a.mount == b.mount);
    out
}

/// Copies `file` into `dir` (a folder on `drive`, if known), checking free
/// space and FAT32's 4 GB limit first. Its space is set aside before
/// writing, so the card can give it one unbroken run. The copy is written
/// under a temporary name, flushed to the card, and renamed when complete.
pub fn copy(
    file: &Path,
    dir: &Path,
    drive: Option<&Drive>,
    progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<PathBuf> {
    let size = file.metadata()?.len();
    if let Some(d) = drive {
        if d.is_fat32() && size > FAT32_MAX {
            return Err(Error::Fat32);
        }
        if d.free < size {
            return Err(Error::NoSpace {
                need_mb: size.div_ceil(1_000_000),
                free_mb: d.free / 1_000_000,
            });
        }
    }
    let name = file
        .file_name()
        .ok_or_else(|| Error::Io(std::io::Error::other("not a file")))?;
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(name);
    let mut part_name = name.to_os_string();
    part_name.push(".part");
    let part = dir.join(part_name);
    let result = (|| -> Result<()> {
        let mut src = File::open(file)?;
        let mut dst = File::create(&part)?;
        // One unbroken run on the card where there is one.
        crate::reserve::reserve(&dst, size);
        let mut buf = vec![0u8; 8 << 20];
        let mut done = 0u64;
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])?;
            done += n as u64;
            if !progress(Progress {
                stage: Stage::Downloading,
                done,
                total: size,
            }) {
                return Err(Error::Cancelled);
            }
        }
        // Cards are often pulled out right after "done": make sure it's on the card.
        dst.sync_all()?;
        if dst.metadata()?.len() != size {
            return Err(Error::Io(std::io::Error::other("the copy is incomplete")));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xwa-drives-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copies_and_refuses_what_will_not_fit() {
        let dir = temp("copy");
        let src = dir.join("wiki.xwa");
        std::fs::write(&src, vec![7u8; 100_000]).unwrap();
        let card = dir.join("card").join("wiki");
        let out = copy(&src, &card, None, &mut |_| true).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&src).unwrap());
        assert!(!card.join("wiki.xwa.part").exists());

        let small = Drive {
            name: "SD".into(),
            mount: dir.clone(),
            free: 10,
            total: 100,
            file_system: "exfat".into(),
        };
        assert!(matches!(
            copy(&src, &card, Some(&small), &mut |_| true),
            Err(Error::NoSpace { .. })
        ));
        let fat = Drive {
            free: u64::MAX,
            file_system: "vfat".into(),
            ..small.clone()
        };
        assert!(fat.is_fat32());
        assert!(
            copy(&src, &card, Some(&fat), &mut |_| true).is_ok(),
            "small files fit on FAT32"
        );

        // Cancelling leaves nothing behind.
        let cancelled = copy(&src, &dir.join("other"), None, &mut |_| false);
        assert!(matches!(cancelled, Err(Error::Cancelled)));
        assert!(
            std::fs::read_dir(dir.join("other"))
                .unwrap()
                .next()
                .is_none()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archives_go_in_the_wiki_folder() {
        assert_eq!(
            archive_dir(Path::new("/media/me/SDCARD")),
            Path::new("/media/me/SDCARD/wiki")
        );
        assert_eq!(
            archive_dir(Path::new("E:\\")),
            Path::new("E:\\").join("wiki")
        );
        assert_eq!(
            archive_dir(Path::new("/media/me/SDCARD/WIKI")),
            Path::new("/media/me/SDCARD/WIKI")
        );
    }

    #[test]
    fn labels_read_naturally() {
        let d = Drive {
            name: "XTEINK".into(),
            mount: PathBuf::from("/media/sd"),
            free: 12_300_000_000,
            total: 0,
            file_system: "exfat".into(),
        };
        assert_eq!(d.label(), "XTEINK (/media/sd) · 12.3 GB free");
    }
}
