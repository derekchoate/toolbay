//! Phase-1-hardcoded install flow for headroom-ai.
//!
//! Functions: download_and_verify, install_standalone_python,
//! install_headroom_wheel, report_install_size.

use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use tokio::task;

use crate::manifest_headroom as M;

/// Error types for the install flow.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("Download failed: {0}")]
    Download(String),
    #[error("SHA-256 mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Wheel unpack failed: {0}")]
    Unpack(String),
    #[error("Python install failed: {0}")]
    PythonInstall(String),
    #[error("Wheel install failed: {0}")]
    WheelInstall(String),
}

// ---------------------------------------------------------------------------
// download_and_verify
// ---------------------------------------------------------------------------

/// Stream a URL through SHA-256, write to disk. Deletes partial file on mismatch.
///
/// Returns the absolute path to the verified destination file.
pub async fn download_and_verify(
    url: &str,
    expected_sha256: &str,
    dest_path: &Path,
) -> Result<PathBuf, InstallError> {
    // Ensure parent directory exists
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let resp = reqwest::get(url).await.map_err(|e| InstallError::Download(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(InstallError::Download(format!(
            "HTTP {} from {}",
            resp.status(),
            url
        )));
    }

    let mut hasher = Sha256::new();
    let mut file = File::create(dest_path)?;

    // Stream through both the file writer and SHA-256 chunk-by-chunk
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| InstallError::Download(e.to_string()))?;
        hasher.update(&chunk);
        file.write_all(&chunk)?;
    }

    file.flush()?;
    drop(file);

    // Compute final digest
    let computed_hex = format!("{:x}", hasher.finalize());
    let expected_clean = expected_sha256.trim().to_lowercase();
    let computed_clean = computed_hex.clone();

    if expected_clean != computed_clean {
        // Delete partial/unverified file — never leave an unverified artifact usable
        let _ = fs::remove_file(dest_path);
        return Err(InstallError::HashMismatch {
            expected: expected_clean,
            got: computed_clean,
        });
    }

    Ok(dest_path.to_path_buf())
}

// ---------------------------------------------------------------------------
// install_standalone_python
// ---------------------------------------------------------------------------

/// Install the vendored Python standalone into `install_dir`.
/// Returns the absolute path to the Python interpreter.
pub async fn install_standalone_python(
    install_dir: &Path,
) -> Result<PathBuf, InstallError> {
    let python_archive_path = install_dir.join("python-standalone.tar.gz");

    // Download and verify the standalone Python archive
    download_and_verify(
        M::PYTHON_STANDALONE_URL,
        M::PYTHON_STANDALONE_SHA256,
        &python_archive_path,
    )
    .await?;

    // Extract — python-build-standalone "install-only" releases extract directly.
    // Use spawn_blocking since tar extraction is sync I/O.
    let install_dir_clone = install_dir.to_path_buf();
    let archive_path_clone = python_archive_path.clone();

    task::spawn_blocking(move || -> Result<(), InstallError> {
        use std::process::Command;

        // Try using the system `tar` (macOS ships with GNU tar or BSD tar)
        let status = Command::new("tar")
            .args(["-xzf", &archive_path_clone.to_string_lossy(), "-C", &install_dir_clone.to_string_lossy()])
            .status()
            .map_err(|e| InstallError::PythonInstall(format!("Failed to run tar: {}", e)))?;

        if !status.success() {
            return Err(InstallError::PythonInstall(format!(
                "tar extraction failed with status {:?}",
                status.code()
            )));
        }

        Ok(())
    })
    .await
    .map_err(|e| InstallError::PythonInstall(format!("Join error: {}", e)))??;

    // Clean up the archive after successful extraction
    let _ = fs::remove_file(&python_archive_path);

    // Resolve the interpreter path — python-build-standalone "install-only" puts
    // it at <dir>/opt/python/<version>/bin/python (or similar).
    // We search for the actual binary.
    find_python_interpreter(install_dir)
}

/// Find the Python interpreter within an extracted standalone install directory.
fn find_python_interpreter(install_dir: &Path) -> Result<PathBuf, InstallError> {
    // python-build-standalone install-only layout: <dir>/opt/python/<version>/bin/python
    for entry in fs::read_dir(install_dir).map_err(|e| InstallError::PythonInstall(e.to_string()))? {
        let entry = entry.map_err(|e| InstallError::PythonInstall(e.to_string()))?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        if file_name_str.starts_with("python-") && file_name_str.contains("macos") {
            // This is the extracted top-level dir (for non-install-only)
            let candidate = entry.path().join("bin").join("python3");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    // Also check for install-only layout: <dir>/opt/python/*/bin/python3
    let opt_dir = install_dir.join("opt").join("python");
    if opt_dir.exists() {
        if let Ok(entries) = fs::read_dir(&opt_dir) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("bin").join("python3");
                if candidate.exists() {
                    return Ok(candidate);
                }
                // Also check for `python` (symlink target)
                let candidate_py = entry.path().join("bin").join("python");
                if candidate_py.exists() {
                    return Ok(candidate_py);
                }
            }
        }
    }

    Err(InstallError::PythonInstall(format!(
        "Could not find Python interpreter in {}",
        install_dir.display()
    )))
}

// ---------------------------------------------------------------------------
// install_headroom_wheel
// ---------------------------------------------------------------------------

/// Download and unpack the headroom-ai wheel into `install_dir`.
pub async fn install_headroom_wheel(
    python_interpreter: &Path,
    install_dir: &Path,
) -> Result<(), InstallError> {
    let wheel_archive_path = install_dir.join("headroom-ai.whl");

    // Download and verify the wheel
    download_and_verify(
        M::WHEEL_URL,
        M::WHEEL_SHA256,
        &wheel_archive_path,
    )
    .await?;

    let python_path = python_interpreter.to_string_lossy().to_string();
    let install_dir_str = install_dir.to_string_lossy().to_string();
    let wheel_path_str = wheel_archive_path.to_string_lossy().to_string();
    // Clone for post-cleanup (wheel_for_cleanup is moved into closure)
    let wheel_for_cleanup = wheel_archive_path.clone();

    // Use pip to install into the target directory (or manual unpack if deps are small)
    task::spawn_blocking(move || -> Result<(), InstallError> {
        use std::process::Command;

        // Try pip install --target first
        let output = Command::new(&python_path)
            .args([
                "-m", "pip", "install",
                "--target", &install_dir_str,
                "--no-compile",
                &wheel_path_str,
            ])
            .output()
            .map_err(|e| InstallError::WheelInstall(format!("Failed to run pip: {}", e)))?;

        if !output.status.success() {
            // Fall back to manual unpack if pip fails
            manual_unpack_wheel(&wheel_for_cleanup, Path::new(&install_dir_str))?;
        }

        Ok(())
    })
    .await
    .map_err(|e| InstallError::WheelInstall(format!("Join error: {}", e)))??;

    // Clean up the wheel archive (use the original path, not the moved clone)
    let _ = fs::remove_file(&wheel_archive_path);

    Ok(())
}

/// Manual unpack of a wheel file (which is just a ZIP archive).
fn manual_unpack_wheel(wheel_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
    use zip::ZipArchive;

    let file = File::open(wheel_path).map_err(|e| InstallError::Unpack(e.to_string()))?;
    let mut archive = ZipArchive::new(BufReader::new(file))
        .map_err(|e| InstallError::Unpack(e.to_string()))?;

    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| InstallError::Unpack(e.to_string()))?;
        let outpath = target_dir.join(f.name().replace('\\', "/"));

        if f.name().ends_with('/') {
            fs::create_dir_all(&outpath).map_err(|e| InstallError::Unpack(e.to_string()))?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|e| InstallError::Unpack(e.to_string()))?;
            }
            let mut outfile = File::create(&outpath).map_err(|e| InstallError::Unpack(e.to_string()))?;
            io::copy(&mut f, &mut outfile).map_err(|e| InstallError::Unpack(e.to_string()))?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// report_install_size
// ---------------------------------------------------------------------------

/// Walk the install directory and sum file sizes.
pub fn report_install_size(dir: &Path) -> Result<u64, InstallError> {
    let mut total: u64 = 0;
    for entry in walk_dir(dir)? {
        if entry.path().is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

fn walk_dir(dir: &Path) -> Result<Vec<std::fs::DirEntry>, InstallError> {
    let mut entries = Vec::new();
    walk_recursive(dir, &mut entries)?;
    Ok(entries)
}

fn walk_recursive(dir: &Path, entries: &mut Vec<std::fs::DirEntry>) -> Result<(), InstallError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_recursive(&path, entries)?;
        } else {
            entries.push(entry);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public install flow (convenience wrapper)
// ---------------------------------------------------------------------------

/// Full Phase-1 install flow for headroom-ai:
/// 1. Create runtime directory
/// 2. Install standalone Python
/// 3. Install headroom-ai wheel
/// Returns the absolute path to the Python interpreter and total install size.
pub async fn install_headroom_ai(home: &Path) -> Result<(PathBuf, u64), InstallError> {
    let runtime = crate::paths::runtime_dir(home, M::TOOL_ID);
    fs::create_dir_all(&runtime)?;

    let python_path = install_standalone_python(&runtime).await?;
    install_headroom_wheel(&python_path, &runtime).await?;

    let size = report_install_size(&runtime)?;

    Ok((python_path, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_install_size_on_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(report_install_size(temp.path()).unwrap(), 0);
    }

    #[test]
    fn test_report_install_size_with_files() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, b"hello world").unwrap();
        assert_eq!(report_install_size(temp.path()).unwrap(), 11);
    }

    #[test]
    fn test_report_install_size_nested() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("sub");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("a.txt"), b"aaa").unwrap();
        fs::write(temp.path().join("b.txt"), b"bbbb").unwrap();
        assert_eq!(report_install_size(temp.path()).unwrap(), 7);
    }

    #[test]
    fn test_build_launch_command_produces_valid_vec() {
        use crate::manifest_headroom;
        let cmd = manifest_headroom::build_launch_command("/usr/bin/python");
        assert_eq!(cmd.len(), 3);
        assert_eq!(cmd[0], "/usr/bin/python");
        assert_eq!(cmd[1], "-m");
        assert_eq!(cmd[2], "headroom.ai");
    }
}