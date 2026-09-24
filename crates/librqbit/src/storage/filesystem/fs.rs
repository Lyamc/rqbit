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

    fn relocate_output(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
        new_output_folder: &Path,
        copy: bool,
    ) -> anyhow::Result<()> {
        std::fs::create_dir_all(new_output_folder)
            .with_context(|| format!("error creating destination {new_output_folder:?}"))?;

        let old_folder = self.output_folder();
        if old_folder == new_output_folder {
            return Ok(());
        }

        for of in &self.opened_files {
            of.close_fd()?;
        }

        for (file_id, fi) in metadata.file_infos.iter().enumerate() {
            if fi.attrs.padding {
                continue;
            }
            let rel = Self::relative_path(shared, metadata, file_id);
            let src = old_folder.join(&rel);
            let dst = new_output_folder.join(&rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !src.exists() {
                continue;
            }
            if copy {
                std::fs::copy(&src, &dst)
                    .with_context(|| format!("error copying {src:?} -> {dst:?}"))?;
            } else if let Err(e) = std::fs::rename(&src, &dst) {
                warn!(error=?e, ?src, ?dst, "rename failed; falling back to copy");
                std::fs::copy(&src, &dst)
                    .with_context(|| format!("error copying {src:?} -> {dst:?}"))?;
                let _ = std::fs::remove_file(&src);
            }
        }

        *self.output_folder.write() = new_output_folder.to_path_buf();

        for (file_id, fi) in metadata.file_infos.iter().enumerate() {
            if fi.attrs.padding {
                continue;
            }
            let rel = Self::relative_path(shared, metadata, file_id);
            let full = new_output_folder.join(&rel);
            let f = Self::open_file(shared, &full)?;
            self.opened_files[file_id].reopen(full, f)?;
        }

        Ok(())
    }
}
