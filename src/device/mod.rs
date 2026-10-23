//! erasing whole block devices: disks and partitions. only linux is supported for now; elsewhere a
//! device is refused with code 1. the checks that keep an erase away from a device in use are in `linux`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::cli::Cli;
use crate::erase::Options;
use crate::{EXIT_INVALID, NAME};

#[cfg(target_os = "linux")]
mod linux;

/// true when the paths name devices. a run that mixes devices with other paths is refused, and so is a
/// symbolic link to a block device: the error is returned for the caller to print, before anything is done.
pub fn is_device_run(paths: &[PathBuf]) -> Result<bool, String> {
    let mut devices = 0;
    for path in paths {
        if names_device(path)? {
            devices += 1;
        }
    }
    match devices {
        0 => Ok(false),
        count if count == paths.len() => Ok(true),
        _ => Err("devices cannot be mixed with files".to_owned()),
    }
}

/// a block or character device named directly. a symbolic link is not followed to a device, so a link to one
/// is refused. the exception is a link under `/dev/disk/` (by-id, by-uuid and the like) that resolves to a block
/// device: it names that device and is erased through the resolved path. a link to anything else, such as a
/// file, takes the file rules and is removed as a link, which does not touch its target.
#[cfg(unix)]
fn names_device(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::FileTypeExt;
    use std::path::Component;

    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(false);
    };
    if meta.file_type().is_symlink() {
        let Ok(target) = std::fs::metadata(path) else {
            return Ok(false);
        };
        if path.starts_with("/dev/disk") && target.file_type().is_block_device() {
            if path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
            {
                return Err(format!(
                    "{}: a link under /dev/disk/ must be named without '..' or '.'",
                    path.display()
                ));
            }
            return Ok(true);
        }
        if target.file_type().is_block_device() || target.file_type().is_char_device() {
            return Err(format!(
                "{}: a symbolic link to a device is not followed, name the device path itself (for example /dev/sdb)",
                path.display()
            ));
        }
        return Ok(false);
    }
    Ok(meta.file_type().is_block_device() || meta.file_type().is_char_device())
}

/// windows physical drives and volumes are named `\\.\...`.
#[cfg(not(unix))]
fn names_device(path: &Path) -> Result<bool, String> {
    Ok(path.to_string_lossy().starts_with(r"\\.\"))
}

/// erases the devices in `paths`, which `is_device_run` has already found to be all devices.
pub fn run(paths: &[PathBuf], cli: &Cli, opts: &Options) -> ExitCode {
    if cli.recursive {
        eprintln!("{NAME}: -r cannot be used with devices: a device is erased as a whole");
        return ExitCode::from(EXIT_INVALID);
    }
    if cli.verify {
        eprintln!("{NAME}: --verify is not supported for devices yet");
        return ExitCode::from(EXIT_INVALID);
    }
    erase_devices(paths, cli, opts)
}

#[cfg(target_os = "linux")]
fn erase_devices(paths: &[PathBuf], cli: &Cli, opts: &Options) -> ExitCode {
    linux::erase(paths, cli, opts)
}

#[cfg(not(target_os = "linux"))]
fn erase_devices(paths: &[PathBuf], _cli: &Cli, _opts: &Options) -> ExitCode {
    for path in paths {
        eprintln!(
            "{NAME}: {}: erasing devices is supported on linux only for now",
            path.display()
        );
    }
    ExitCode::from(EXIT_INVALID)
}
