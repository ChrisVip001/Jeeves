//! A missing settings file starts empty. A damaged existing file must survive a failed edit.
use serde::{Serialize, de::DeserializeOwned};
use std::{fs::File, io, path::Path};

pub(crate) fn read<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid JSON data at line {}, column {}",
                    e.line(),
                    e.column()
                ),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e),
    }
}

pub(crate) fn write<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let mut tmp = tempfile::Builder::new()
        .prefix(".jeeves-config-")
        .tempfile_in(path.parent().expect("home file"))?;
    serde_json::to_writer_pretty(tmp.as_file_mut(), value)?;
    tmp.persist(path)?;
    Ok(())
}

pub(crate) fn lock(home: &Path, name: &str) -> io::Result<File> {
    std::fs::create_dir_all(home)?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.join(name))?;
    file.lock()?;
    Ok(file)
}
