pub mod external_editor;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(windows)]
use warp_util::path::is_network_resource;
use warp_util::path::{CleanPathResult, LineAndColumnArg};

use crate::terminal::model::grid::grid_handler::{ContainsPoint, Link};
use crate::terminal::model::index::Point;
use crate::terminal::ShellLaunchData;

pub use self::external_editor::{open_file_path_in_external_editor, open_file_path_with_editor};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePathType {
    Absolute,
    /// Contains the working directory PathBuf.
    Relative(PathBuf),
}

#[derive(Debug)]
pub enum ShellPathType {
    /// The path comes from the shell and may need to be converted in a shell-aware way.
    ShellNative(String),
    /// The path has already been converted to a OS-native path.
    PlatformNative(PathBuf),
}

/// Zap: a snapshot of the real entries under a given remote directory (cwd).
///
/// Populated from the result returned by the daemon's `ListDirectory` RPC. The
/// terminal link detector uses it for precise validation in remote sessions:
/// slicing the real file name out of a candidate substring in a whole `ls -l`
/// line -- which is exactly what the `fs::metadata` existence check does in
/// local sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDirListing {
    /// The absolute path of the directory (the remote cwd).
    pub dir: PathBuf,
    /// The direct entries under the directory: file name -> whether it is a directory.
    pub entries: HashMap<String, bool>,
}

impl RemoteDirListing {
    pub fn new(dir: PathBuf, entries: HashMap<String, bool>) -> Self {
        Self { dir, entries }
    }
}

/// Zap: the source of truth for validating terminal file links.
///
/// Local sessions use the local file system `fs::metadata` to decide whether a
/// path exists; the files in a remote SSH (remote-server) session are not on
/// the local disk, so local validation is bound to fail. Remote sessions
/// therefore switch to precise validation against the real directory listing
/// cached from the daemon `ListDirectory` RPC.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LinkValidationContext {
    /// Local session: validate whether the path actually exists using the local file system.
    #[default]
    Local,
    /// Remote SSH session: validate precisely against the cached remote cwd directory listing.
    ///
    /// `None` means the directory listing for this cwd has not been cached yet
    /// (an async fetch is in flight or the fetch failed); in that case this
    /// validation pass is treated as "invalid" (not highlighted), and is lit up
    /// on a later re-render once the listing arrives.
    Remote(Option<Arc<RemoteDirListing>>),
}

/// Checks if a file path exists and is valid for a file link.
pub fn absolute_path_if_valid(
    clean_path_result: &CleanPathResult,
    working_directory: ShellPathType,
    shell_launch_data: Option<&ShellLaunchData>,
    validation_ctx: &LinkValidationContext,
) -> Option<PathBuf> {
    let (maybe_absolute_path, relative_path) = match shell_launch_data {
        Some(shell_launch_data) => {
            // Attempt to parse the clean path result as an absolute path.
            let maybe_absolute_path =
                shell_launch_data.maybe_convert_absolute_path(&clean_path_result.path);
            let relative_path = match working_directory {
                ShellPathType::ShellNative(base_path_str) => shell_launch_data
                    .maybe_convert_relative_path(&base_path_str, &clean_path_result.path),
                ShellPathType::PlatformNative(base_path) => {
                    shell_launch_data.join_to_native_path(&base_path, &clean_path_result.path)
                }
            };
            (maybe_absolute_path, relative_path)
        }
        None => {
            // We naively attempt to treat the given paths as platform-native.
            let maybe_absolute_path = PathBuf::from(&clean_path_result.path);
            let relative_path = match working_directory {
                ShellPathType::ShellNative(path_str) => {
                    let mut path_buf = PathBuf::from(path_str);
                    path_buf.push(&clean_path_result.path);
                    path_buf
                }
                ShellPathType::PlatformNative(path_buf) => path_buf.join(&clean_path_result.path),
            };
            (Some(maybe_absolute_path), Some(relative_path))
        }
    };

    if relative_path
        .as_ref()
        .is_some_and(|path| is_path_valid(path, clean_path_result, validation_ctx))
    {
        return relative_path;
    } else if maybe_absolute_path
        .as_ref()
        .is_some_and(|path| is_path_valid(path, clean_path_result, validation_ctx))
    {
        return maybe_absolute_path;
    }

    None
}

fn is_path_valid(
    path: &Path,
    clean_path_result: &CleanPathResult,
    validation_ctx: &LinkValidationContext,
) -> bool {
    // Checking for the existence of a network resource takes a long time (~15s),
    // and hangs the UI, so we skip validating it.
    #[cfg(windows)]
    if is_network_resource(path) {
        return false;
    }

    // Zap: the files in a remote SSH session are not on the local disk, so
    // `fs::metadata` is bound to fail. Instead, validate precisely against the
    // real directory listing cached from the daemon `ListDirectory`: a candidate
    // resolved path is valid iff its parent directory is exactly the cached cwd
    // and its file name is a known entry under that directory. This gives the
    // link detector's substring search a disambiguation basis equivalent to the
    // local `fs::metadata`, allowing it to accurately slice the real file name
    // out of a whole `ls -l` line.
    if let LinkValidationContext::Remote(listing) = validation_ctx {
        // The cwd listing has not been cached yet (async fetch in flight/failed):
        // treat this pass as invalid, and light it up once the listing arrives.
        let Some(listing) = listing else {
            return false;
        };
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        // The parent directory must be exactly the cached cwd.
        if path.parent() != Some(listing.dir.as_path()) {
            return false;
        }
        let Some(&is_dir) = listing.entries.get(file_name) else {
            return false;
        };
        // Consistent with local: it cannot be a directory when a line/column number is attached.
        return !is_dir || clean_path_result.line_and_column_num.is_none();
    }

    // It should only be a valid path if the path links to a file or a folder without
    // line and column number attached.
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() || (metadata.is_dir() && clean_path_result.line_and_column_num.is_none())
}

/// Zap: determines whether a resolved remote path points to a directory.
///
/// Only called when a link is clicked in a remote session and we need to decide
/// "open file vs. `cd` into directory"; the basis is the cached remote cwd
/// directory listing. Returns `false` when the listing is not cached or the
/// path is not in it (treated as a file, consistent with the conservative
/// behavior when nothing is cached).
pub fn remote_path_is_dir(path: &Path, listing: &RemoteDirListing) -> bool {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if path.parent() != Some(listing.dir.as_path()) {
        return false;
    }
    listing.entries.get(file_name).copied().unwrap_or(false)
}

impl FilePathType {
    /// Given a path that we've identified the FilePathType of,
    /// returns the absolute path.
    pub fn absolute_path(&self, path: PathBuf) -> PathBuf {
        match self {
            FilePathType::Absolute => path,
            FilePathType::Relative(directory) => directory.join(&path),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    pub link: Link,
    /// This path has been converted (if needed) into a native path from the shell.
    pub absolute_path: PathBuf,
    pub line_and_column_num: Option<LineAndColumnArg>,
}

impl FileLink {
    pub fn absolute_path(&self) -> Option<PathBuf> {
        Some(self.absolute_path.clone())
    }
}

impl ContainsPoint for FileLink {
    fn contains(&self, point: Point) -> bool {
        self.link.contains(point)
    }
}

/// Creates the file at the given path if it doesn't already exist, opening it
/// in write mode. If any directories in the path are missing, those are created
/// as well.
///
/// This always returns an error for unit tests, as they should not directly
/// interact with the filesystem.
pub fn create_file<P: AsRef<Path>>(_path: P) -> io::Result<fs::File> {
    cfg_if::cfg_if! {
        if #[cfg(test)] {
            Err(io::Error::from_raw_os_error(1))
        } else {
            let path = _path.as_ref();
            fs::create_dir_all(path.parent().ok_or_else(|| {
                io::Error::other(
                    "full_path should never be root directory.",
                )
            })?)?;
            fs::File::create(path)
        }
    }
}
