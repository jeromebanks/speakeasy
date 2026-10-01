//! Publish-input access that never follows symlinks below the input root.
//!
//! Every path component is opened relative to its parent directory handle
//! with `O_NOFOLLOW`, and the final handle is checked to be a regular file.
//! Swapping a file or directory for a symlink between listing and import
//! therefore fails instead of publishing whatever the link points to.

use std::{
    io,
    os::fd::{AsFd, OwnedFd},
    path::Path,
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use n0_future::Stream;
use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, fstat, openat, statat};
use tokio::io::AsyncReadExt;

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

pub struct InputDir {
    root: OwnedFd,
}

impl InputDir {
    /// Open the input root. The root path itself may be a symlink (it is the
    /// user's explicit choice); nothing below it is followed.
    pub fn open(path: &Path) -> Result<Self> {
        let root = openat(
            CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .with_context(|| format!("open input directory {}", path.display()))?;
        Ok(Self { root })
    }

    /// Relative `/`-separated paths of all regular files, sorted. Symlinks and
    /// special files are rejected.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        walk(&self.root, "", &mut out)?;
        out.sort();
        Ok(out)
    }

    /// Open a listed file without following symlinks at any component.
    /// Returns the handle and its size at open time.
    pub fn open_file(&self, rel: &str) -> Result<(std::fs::File, u64)> {
        let mut comps: Vec<&str> = rel.split('/').collect();
        let last = comps.pop().context("empty path")?;
        let mut dir: Option<OwnedFd> = None;
        for comp in comps {
            let parent = dir.as_ref().map(|d| d.as_fd()).unwrap_or(self.root.as_fd());
            let next = openat(parent, comp, DIR_FLAGS, Mode::empty()).with_context(|| {
                format!("{rel}: directory component {comp:?} is not a plain directory")
            })?;
            dir = Some(next);
        }
        let parent = dir.as_ref().map(|d| d.as_fd()).unwrap_or(self.root.as_fd());
        let fd = openat(parent, last, FILE_FLAGS, Mode::empty())
            .with_context(|| format!("{rel}: cannot open without following symlinks"))?;
        let st = fstat(&fd)?;
        if FileType::from_raw_mode(st.st_mode as _) != FileType::RegularFile {
            bail!("{rel} is not a regular file");
        }
        Ok((std::fs::File::from(fd), st.st_size as u64))
    }
}

fn walk(dir: &OwnedFd, rel: &str, out: &mut Vec<String>) -> Result<()> {
    let entries = Dir::read_from(dir).with_context(|| format!("read input directory {rel:?}"))?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let name = name
            .to_str()
            .with_context(|| format!("non-UTF-8 file name in {rel:?}"))?;
        let child = if rel.is_empty() {
            name.to_string()
        } else {
            format!("{rel}/{name}")
        };
        let st = statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
        match FileType::from_raw_mode(st.st_mode as _) {
            FileType::Directory => {
                let sub = openat(dir, name, DIR_FLAGS, Mode::empty())
                    .with_context(|| format!("open directory {child}"))?;
                walk(&sub, &child, out)?;
            }
            FileType::RegularFile => out.push(child),
            FileType::Symlink => bail!("publish input contains a symlink: {child}"),
            _ => bail!("publish input contains a special file: {child}"),
        }
    }
    Ok(())
}

/// Stream a file's bytes in 1 MiB chunks, failing if it grows beyond `limit`.
pub fn file_stream(
    file: std::fs::File,
    limit: u64,
) -> impl Stream<Item = io::Result<Bytes>> + Send + Sync + 'static {
    let file = tokio::fs::File::from_std(file);
    n0_future::stream::unfold(Some((file, 0u64)), move |state| async move {
        let (mut file, read) = state?;
        let mut buf = vec![0u8; 1 << 20];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) if read + n as u64 > limit => Some((
                Err(io::Error::other("input file exceeds artifact size limit")),
                None,
            )),
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), Some((file, read + n as u64))))
            }
            Err(e) => Some((Err(e), None)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_symlinked_files_and_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("in");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.txt"), b"a").unwrap();
        let secret = tmp.path().join("secret");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("a.txt"), b"secret").unwrap();
        let input = InputDir::open(&root).unwrap();
        assert_eq!(input.list().unwrap(), vec!["sub/a.txt"]);

        // Swap the listed file for a symlink after listing.
        std::fs::remove_file(root.join("sub/a.txt")).unwrap();
        std::os::unix::fs::symlink(secret.join("a.txt"), root.join("sub/a.txt")).unwrap();
        assert!(input.open_file("sub/a.txt").is_err());

        // Swap the parent directory for a symlink to another directory.
        std::fs::remove_file(root.join("sub/a.txt")).unwrap();
        std::fs::remove_dir(root.join("sub")).unwrap();
        std::os::unix::fs::symlink(&secret, root.join("sub")).unwrap();
        assert!(input.open_file("sub/a.txt").is_err());
        assert!(input.list().is_err());
    }
}
