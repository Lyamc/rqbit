use std::{
    fs::OpenOptions,
    io::IoSlice,
    path::{Path, PathBuf},
};

use anyhow::Context;
use parking_lot::RwLock;
use tracing::warn;

use crate::{
    storage::{StorageFactoryExt, filesystem::opened_file::OurFileExt},
    torrent_state::{ManagedTorrentShared, TorrentMetadata},
};

use crate::storage::{StorageFactory, TorrentStorage};

use super::opened_file::OpenedFile;

#[derive(Default, Clone, Copy)]
pub struct FilesystemStorageFactory {}

impl StorageFactory for FilesystemStorageFactory {
    type Storage = FilesystemStorage;

    fn create(
        &self,
        shared: &ManagedTorrentShared,
        _metadata: &TorrentMetadata,
    ) -> anyhow::Result<FilesystemStorage> {
        Ok(FilesystemStorage {
            output_folder: RwLock::new(shared.output_folder()),
            opened_files: Default::default(),
        })
    }

    fn clone_box(&self) -> crate::storage::BoxStorageFactory {
        self.boxed()
    }
}

pub struct FilesystemStorage {
    pub(crate) output_folder: RwLock<PathBuf>,
    pub(crate) opened_files: Vec<OpenedFile>,
}

impl FilesystemStorage {
    fn output_folder(&self) -> PathBuf {
        self.output_folder.read().clone()
    }

    #[allow(dead_code)]
    pub(crate) fn take_fs(&self) -> anyhow::Result<Self> {
        Ok(Self {
            opened_files: self
                .opened_files
                .iter()
                .map(|f| f.take_clone())
                .collect::<anyhow::Result<Vec<_>>>()?,
            output_folder: RwLock::new(self.output_folder()),
        })
    }

    fn open_file(shared: &ManagedTorrentShared, full_path: &Path) -> anyhow::Result<std::fs::File> {
        if shared.options.allow_overwrite {
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(full_path)
                .with_context(|| format!("error opening {full_path:?} in read/write mode"))
        } else {
            if !full_path.exists() {
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(full_path)
                    .with_context(|| {
                        format!(
                            "error creating a new file (because allow_overwrite = false) {:?}",
                            full_path
                        )
                    })?;
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(full_path)
                .with_context(|| format!("error opening {full_path:?}"))
        }
    }

    fn relative_path(shared: &ManagedTorrentShared, metadata: &TorrentMetadata, file_id: usize) -> PathBuf {
        shared
            .file_rename(file_id)
            .unwrap_or_else(|| metadata.file_infos[file_id].relative_filename.clone())
    }
}

impl TorrentStorage for FilesystemStorage {
    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        self.opened_files
            .get(file_id)
            .context("no such file")?
            .lock_read()?
            .pread_exact(offset, buf)
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        let of = self.opened_files.get(file_id).context("no such file")?;
        #[cfg(windows)]
        return of.try_mark_sparse()?.pwrite_all(offset, buf);
        #[cfg(not(windows))]
        return of.lock_read()?.pwrite_all(offset, buf);
    }

    fn pwrite_all_vectored(
        &self,
        file_id: usize,
        offset: u64,
        bufs: [IoSlice<'_>; 2],
    ) -> anyhow::Result<usize> {
        let of = self.opened_files.get(file_id).context("no such file")?;
        #[cfg(windows)]
        return of.try_mark_sparse()?.pwrite_all_vectored(offset, bufs);
        #[cfg(not(windows))]
        return of.lock_read()?.pwrite_all_vectored(offset, bufs);
    }

    fn remove_file(&self, _file_id: usize, filename: &Path) -> anyhow::Result<()> {
        Ok(std::fs::remove_file(self.output_folder().join(filename))?)
    }

    fn ensure_file_length(&self, file_id: usize, len: u64) -> anyhow::Result<()> {
        let f = &self.opened_files.get(file_id).context("no such file")?;
        #[cfg(windows)]
        f.try_mark_sparse()?;
        Ok(f.lock_read()?.set_len(len)?)
    }

    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        Ok(Box::new(Self {
            opened_files: self
                .opened_files
                .iter()
                .map(|f| f.take_clone())
                .collect::<anyhow::Result<Vec<_>>>()?,
            output_folder: RwLock::new(self.output_folder()),
        }))
    }

    fn remove_directory_if_empty(&self, path: &Path) -> anyhow::Result<()> {
        let path = self.output_folder().join(path);
        if !path.is_dir() {
            anyhow::bail!("cannot remove dir: {path:?} is not a directory")
        }
        if std::fs::read_dir(&path)?.count() == 0 {
            std::fs::remove_dir(&path).with_context(|| format!("error removing {path:?}"))
        } else {
            warn!("did not remove {path:?} as it was not empty");
            Ok(())
        }
    }

    fn init(
        &mut self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        let output_folder = shared.output_folder();
        *self.output_folder.write() = output_folder.clone();
        let mut files = Vec::<OpenedFile>::new();
        for (file_id, file_details) in metadata.file_infos.iter().enumerate() {
            let relative_path = Self::relative_path(shared, metadata, file_id);
            let mut full_path = output_folder.clone();
            full_path.push(&relative_path);

            if file_details.attrs.padding {
                files.push(OpenedFile::new_dummy());
                continue;
            };
            std::fs::create_dir_all(full_path.parent().context("bug: no parent")?)?;
            let f = Self::open_file(shared, &full_path)?;
            files.push(OpenedFile::new(full_path.clone(), f));
        }

        self.opened_files = files;
        Ok(())
    }

    fn rename_file(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
        file_id: usize,
        new_relative_path: &Path,
    ) -> anyhow::Result<()> {
        let _ = metadata; // unused; relative path is caller-provided
        let of = self.opened_files.get(file_id).context("no such file")?;
        if of.is_dummy() {
            anyhow::bail!("cannot rename padding file");
        }
        let new_full = self.output_folder().join(new_relative_path);
        if let Some(parent) = new_full.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("error creating parent dir for {new_full:?}"))?;
        }
        of.close_and_rename(&new_full)
            .with_context(|| format!("error renaming file to {new_full:?}"))?;
        // Reopen so seeding can continue from the new path.
        let _ = shared;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&new_full)
            .with_context(|| format!("error reopening {new_full:?}"))?;
        of.reopen(new_full, f)?;
        Ok(())
    }

    fn replace_file(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
        file_id: usize,
        f: &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let of = self.opened_files.get(file_id).context("no such file")?;
        if of.is_dummy() {
            anyhow::bail!("cannot replace padding file");
        }
        let full = self
            .output_folder()
            .join(Self::relative_path(shared, metadata, file_id));
        of.close_fd()?;
        let result = f();
        // Reopen whatever is at the path now: the replacement, or the untouched original.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&full)
            .with_context(|| format!("error reopening {full:?}"))?;
        of.reopen(full, file)?;
        result
    }

    fn relocate_files(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
        f: &mut dyn FnMut() -> crate::relocate::Outcome,
    ) -> anyhow::Result<crate::relocate::Outcome> {
        // Peers' reads and our writes wait for the move rather than failing (a failure
        // would count as an I/O error and could mark files damaged).
        let mut guards: Vec<_> = self.opened_files.iter().map(|of| of.lock_for_move()).collect();
        for g in guards.iter_mut() {
            g.close();
        }
        let outcome = f();
        *self.output_folder.write() = outcome.output_folder.clone();
        let new_paths: std::collections::HashMap<usize, PathBuf> =
            outcome.paths.iter().cloned().collect();
        let old_folder = shared.output_folder();
        let mut first_err = None;
        for (file_id, fi) in metadata.file_infos.iter().enumerate() {
            if fi.attrs.padding {
                continue;
            }
            let full = new_paths
                .get(&file_id)
                .cloned()
                .unwrap_or_else(|| old_folder.join(Self::relative_path(shared, metadata, file_id)));
            let r = (|| -> anyhow::Result<std::fs::File> {
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                Self::open_file(shared, &full)
            })();
            match r {
                Ok(f) => guards[file_id].reopen(full, f),
                Err(e) => {
                    first_err.get_or_insert(e.context(format!("error reopening {full:?}")));
                }
            }
        }
        drop(guards);
        if let Some(e) = first_err {
            return Err(e);
        }
        Ok(outcome)
    }

    fn move_one_file(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
        file_id: usize,
        f: &mut dyn FnMut(&Path) -> anyhow::Result<PathBuf>,
    ) -> anyhow::Result<PathBuf> {
        let of = self.opened_files.get(file_id).context("no such file")?;
        let mut g = of.lock_for_move();
        if g.is_dummy() {
            anyhow::bail!("cannot move padding file");
        }
        let current = self
            .output_folder()
            .join(Self::relative_path(shared, metadata, file_id));
        g.close();
        let result = f(&current);
        let path = match &result {
            Ok(p) => p.clone(),
            Err(_) => current,
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("error reopening {path:?}"))?;
        g.reopen(path, file);
        result
    }
}
