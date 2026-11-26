//! Serve explicit deliverables from the owning task's managed workspace.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use jeeves_protocol::{
    AcceptanceView, ArtifactView, ErrorObject, TaskFileParams, TaskFileResponse,
};
use jeeves_state::TaskRow;

use crate::{Server, dispatch::internal, rpc_error::invalid};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FILES: usize = 50;

struct Workspace {
    root: PathBuf,
    dir: Dir,
}

impl Workspace {
    fn open(home: &Path, task: &TaskRow) -> Option<Self> {
        let home = home.canonicalize().ok()?;
        let root = PathBuf::from(task.cwd.as_ref()?);
        let name = root.file_name()?;
        if root.parent()? != home.join("workspaces")
            || !name.to_str()?.ends_with(&format!("-{}", task.id))
        {
            return None;
        }
        let dir = Dir::open_ambient_dir(&home, ambient_authority())
            .ok()?
            .open_dir_nofollow("workspaces")
            .ok()?
            .open_dir_nofollow(name)
            .ok()?;
        Some(Self { root, dir })
    }

    fn relative<'a>(&self, path: &'a str) -> Option<&'a Path> {
        if path.len() > 1024 || path.contains(['\r', '\n', '\0']) {
            return None;
        }
        let path = Path::new(path);
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.root).ok()?
        } else {
            path
        };
        if relative.as_os_str().is_empty() || !relative.components().all(|part| {
            matches!(part, Component::Normal(name) if !name.to_string_lossy().starts_with('.'))
        }) {
            return None;
        }
        Some(relative)
    }

    fn parent<'a>(&self, path: &'a Path) -> io::Result<(Dir, &'a OsStr)> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("Missing filename"))?;
        let mut dir = self.dir.try_clone()?;
        for part in path.parent().into_iter().flat_map(Path::components) {
            dir = dir.open_dir_nofollow(part.as_os_str())?;
        }
        Ok((dir, name))
    }

    fn size(&self, path: &Path) -> Option<i32> {
        let (dir, name) = self.parent(path).ok()?;
        let metadata = dir.symlink_metadata(name).ok()?;
        downloadable(&metadata).then(|| metadata.len() as i32)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let (dir, name) = self.parent(path)?;
        if !downloadable(&dir.symlink_metadata(name)?) {
            return Err(io::Error::other("File is unavailable or exceeds 8 MiB"));
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = dir.open_with(name, &options)?;
        if !downloadable(&file.metadata()?) {
            return Err(io::Error::other("File changed before it could be opened"));
        }
        let before = file.metadata()?;
        let mut data = Vec::new();
        (&file).take(MAX_FILE_BYTES + 1).read_to_end(&mut data)?;
        let after = file.metadata()?;
        if data.len() as u64 > MAX_FILE_BYTES
            || before.len() != data.len() as u64
            || before.modified()? != after.modified()?
            || before.len() != after.len()
        {
            return Err(io::Error::other("File changed or exceeds 8 MiB"));
        }
        Ok(data)
    }
}

fn downloadable(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.is_file() && metadata.nlink() == 1 && metadata.len() <= MAX_FILE_BYTES
}

fn acceptance_paths(task: &TaskRow) -> impl Iterator<Item = String> {
    serde_json::from_str::<Vec<AcceptanceView>>(&task.acceptance_json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|condition| condition.path)
}

pub(super) async fn list(
    home: PathBuf,
    task: TaskRow,
    mut files: Vec<ArtifactView>,
) -> Result<Vec<ArtifactView>, ErrorObject> {
    tokio::task::spawn_blocking(move || {
        let Some(workspace) = Workspace::open(&home, &task) else {
            return files;
        };
        for file in &mut files {
            if let Some(size) = workspace
                .relative(&file.path)
                .and_then(|path| workspace.size(path))
            {
                file.workspace_file = Some(true);
                file.bytes = size;
            }
        }
        for path in acceptance_paths(&task) {
            if files.len() == MAX_FILES {
                break;
            }
            let Some(relative) = workspace.relative(&path) else {
                continue;
            };
            let path = workspace.root.join(relative).to_string_lossy().into_owned();
            if files.iter().any(|file| file.path == path) {
                continue;
            }
            if let Some(bytes) = workspace.size(relative) {
                files.push(ArtifactView {
                    path,
                    bytes,
                    at: task.updated_at.clone(),
                    written_text: None,
                    workspace_file: Some(true),
                });
            }
        }
        files
    })
    .await
    .map_err(internal)
}

pub(crate) async fn read(
    server: &Server,
    p: TaskFileParams,
) -> Result<TaskFileResponse, ErrorObject> {
    let task = server
        .tasks
        .get(&p.task_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| invalid("Task not found"))?;
    let entries = server
        .artifacts
        .list(&p.task_id, MAX_FILES as i64)
        .await
        .map_err(internal)?;
    let home = server.settings().home;
    tokio::task::spawn_blocking(move || {
        let workspace = Workspace::open(&home, &task)
            .ok_or_else(|| invalid("Task has no managed workspace files"))?;
        let path = workspace
            .relative(&p.path)
            .ok_or_else(|| invalid("Select a file from this task's work card"))?;
        let listed = entries
            .iter()
            .filter_map(|file| workspace.relative(&file.path))
            .any(|entry| entry == path)
            || acceptance_paths(&task).any(|entry| workspace.relative(&entry) == Some(path));
        if !listed {
            return Err(invalid("Select a file from this task's work card"));
        }
        let data = workspace
            .read(path)
            .map_err(|_| invalid("Task file is unavailable or exceeds 8 MiB"))?;
        let filename = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| invalid("Task filename is unavailable"))?;
        Ok(TaskFileResponse {
            filename: filename.into(),
            data: STANDARD.encode(data),
        })
    })
    .await
    .map_err(internal)?
}
