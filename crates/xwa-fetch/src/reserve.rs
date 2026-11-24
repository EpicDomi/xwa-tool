//! Setting aside a file's space before writing it.
//!
//! A file written bit by bit gets its clusters as it grows, and on a card
//! whose free space is in pieces (files deleted over time) it ends up in
//! pieces too. Asked for the whole size first, FAT and exFAT drivers give
//! it one unbroken run where there is one, which the reader then maps in a
//! single piece. Only space is set aside: nothing is written, so it costs
//! no time, and the file keeps its length until written.

use std::fs::File;

/// Asks the file system to set aside `len` bytes for `file`, which is
/// empty. False when it cannot (a file system without the call, or not
/// enough room): the copy then goes on as before.
pub fn reserve(file: &File, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    imp::reserve(file, len)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use std::os::fd::AsRawFd;

    pub fn reserve(file: &std::fs::File, len: u64) -> bool {
        let Ok(len) = libc::off_t::try_from(len) else {
            return false;
        };
        // KEEP_SIZE: the only kind of fallocate Linux's FAT driver does.
        // Never posix_fallocate, which writes zeros where the file system
        // cannot set space aside: twice the writing, on a slow card.
        unsafe { libc::fallocate(file.as_raw_fd(), libc::FALLOC_FL_KEEP_SIZE, 0, len) == 0 }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::os::fd::AsRawFd;

    pub fn reserve(file: &std::fs::File, len: u64) -> bool {
        let Ok(len) = libc::off_t::try_from(len) else {
            return false;
        };
        // In one run if the file system can, else wherever it can.
        for flags in [
            libc::F_ALLOCATECONTIG | libc::F_ALLOCATEALL,
            libc::F_ALLOCATEALL,
        ] {
            let mut store = libc::fstore_t {
                fst_flags: flags,
                fst_posmode: libc::F_PEOFPOSMODE,
                fst_offset: 0,
                fst_length: len,
                fst_bytesalloc: 0,
            };
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PREALLOCATE, &mut store) } != -1 {
                return true;
            }
        }
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALLOCATION_INFO, FileAllocationInfo, SetFileInformationByHandle,
    };

    pub fn reserve(file: &std::fs::File, len: u64) -> bool {
        let Ok(len) = i64::try_from(len) else {
            return false;
        };
        // The allocation size, not the end of the file: FAT has no valid
        // data length, so moving the end there would make Windows write
        // zeros all the way.
        let info = FILE_ALLOCATION_INFO {
            AllocationSize: len,
        };
        unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileAllocationInfo,
                (&info as *const FILE_ALLOCATION_INFO).cast(),
                std::mem::size_of::<FILE_ALLOCATION_INFO>() as u32,
            ) != 0
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
mod imp {
    pub fn reserve(_: &std::fs::File, _: u64) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn space_set_aside_leaves_the_file_empty_until_written() {
        let path = std::env::temp_dir().join(format!("xwa-reserve-{}", std::process::id()));
        let mut f = File::create(&path).unwrap();
        // Whether the file system can is its own business; the length
        // must not change either way.
        let _ = reserve(&f, 8 << 20);
        assert_eq!(f.metadata().unwrap().len(), 0);
        f.write_all(b"xwa").unwrap();
        assert_eq!(f.metadata().unwrap().len(), 3);
        drop(f);
        let _ = std::fs::remove_file(path);
    }
}
