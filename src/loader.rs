//! Locating, downloading, and loading the native openDAQ libraries.
//!
//! An end-user build of this crate carries no native code.  On first use the
//! loader searches for the openDAQ shared libraries in this order:
//!
//! 1. the directory named by the `OPENDAQ_RUST_NATIVE_DIR` environment variable,
//! 2. `bin/<platform>/` inside the crate source directory (a development
//!    checkout with locally built binaries),
//! 3. the per-user download cache, downloading the prebuilt archive pinned in
//!    [`crate::native_manifest`] into it when necessary.
//!
//! Set `OPENDAQ_NO_DOWNLOAD` to forbid the automatic download (e.g. in offline
//! or CI environments); [`install_native_libraries`] still triggers it
//! explicitly.  Set `OPENDAQ_NATIVE_ARCHIVE_URL` to fetch the archive from a
//! mirror instead of GitHub (checksum verification is skipped for mirrors).
//!
//! Downloading and unpacking shell out to `curl` and `tar`, which ship with
//! Linux, macOS, and Windows 10+; doing it natively would drag an HTTPS client
//! plus an archive library into every dependent's build.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::native_manifest;

pub const NATIVE_DIR_ENV_VAR: &str = "OPENDAQ_RUST_NATIVE_DIR";
pub const NO_DOWNLOAD_ENV_VAR: &str = "OPENDAQ_NO_DOWNLOAD";
pub const ARCHIVE_URL_ENV_VAR: &str = "OPENDAQ_NATIVE_ARCHIVE_URL";
pub const MODULES_PATH_ENV_VAR: &str = "OPENDAQ_MODULES_PATH";

/// Exact file names of the native libraries, in dependency load order.
#[cfg(target_os = "linux")]
pub const NATIVE_LIBRARY_FILE_NAMES: &[&str] = &[
    "libdaqcoretypes-64-3.so",
    "libdaqcoreobjects-64-3.so",
    "libopendaq-64-3.so",
    "libcopendaq.so",
];
#[cfg(target_os = "macos")]
pub const NATIVE_LIBRARY_FILE_NAMES: &[&str] = &[
    "libdaqcoretypes-64-3.dylib",
    "libdaqcoreobjects-64-3.dylib",
    "libopendaq-64-3.dylib",
    "libcopendaq.dylib",
];
#[cfg(target_os = "windows")]
pub const NATIVE_LIBRARY_FILE_NAMES: &[&str] = &[
    "daqcoretypes-64-3.dll",
    "daqcoreobjects-64-3.dll",
    "opendaq-64-3.dll",
    "copendaq.dll",
];

/// An error locating, downloading, or loading the native openDAQ libraries.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LoadError {
    #[error(
        "unable to locate the openDAQ native libraries; checked {checked:?}.{hint} \
         Set {NATIVE_DIR_ENV_VAR} to override the search path."
    )]
    NotFound { checked: Vec<PathBuf>, hint: String },
    #[error("the {NATIVE_DIR_ENV_VAR} override points to a missing directory: {0}")]
    OverrideMissing(PathBuf),
    #[error(
        "could not find {file} in {dir}; build openDAQ with OPENDAQ_GENERATE_C_BINDINGS=ON \
         so the C wrapper library is produced"
    )]
    MissingLibrary { file: String, dir: PathBuf },
    #[error("no prebuilt openDAQ binaries are published for this platform ({0})")]
    UnsupportedPlatform(String),
    #[error("failed to download {url}: {reason}")]
    DownloadFailed { url: String, reason: String },
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("failed to unpack {archive}: {reason}")]
    UnpackFailed { archive: PathBuf, reason: String },
    #[error("failed to load {library}: {reason}")]
    LibraryLoadFailed { library: PathBuf, reason: String },
    #[error("symbol {symbol} not found in any loaded openDAQ library")]
    MissingSymbol { symbol: String },
    #[error("io error at {path}: {message}")]
    Io { path: PathBuf, message: String },
}

fn io_error(path: &Path, source: std::io::Error) -> LoadError {
    LoadError::Io {
        path: path.to_path_buf(),
        message: source.to_string(),
    }
}

/// Name of the `bin/` subdirectory holding this host's native libraries,
/// e.g. `"windows-x64"`.
pub fn platform_directory_name() -> Result<&'static str, LoadError> {
    let name = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("windows", "x86_64") => "windows-x64",
        (os, arch) => return Err(LoadError::UnsupportedPlatform(format!("{os}-{arch}"))),
    };
    Ok(name)
}

fn manifest_archive() -> Result<(&'static str, &'static str, &'static str), LoadError> {
    let platform = platform_directory_name()?;
    native_manifest::ARCHIVES
        .iter()
        .copied()
        .find(|(p, _, _)| *p == platform)
        .ok_or_else(|| LoadError::UnsupportedPlatform(platform.to_string()))
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn downloads_disabled() -> bool {
    env_non_empty(NO_DOWNLOAD_ENV_VAR).is_some()
}

/// Per-user cache root, e.g. `~/.cache/opendaq-rs` or `%LOCALAPPDATA%\opendaq-rs\cache`.
fn cache_root() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        env_non_empty("LOCALAPPDATA").map(|d| PathBuf::from(d).join("opendaq-rs").join("cache"))
    }
    #[cfg(target_os = "macos")]
    {
        env_non_empty("HOME").map(|d| PathBuf::from(d).join("Library/Caches/opendaq-rs"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        env_non_empty("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env_non_empty("HOME").map(|d| PathBuf::from(d).join(".cache")))
            .map(|d| d.join("opendaq-rs"))
    }
}

/// The cache directory the pinned release unpacks into.
fn cache_platform_directory() -> Result<Option<PathBuf>, LoadError> {
    let platform = platform_directory_name()?;
    Ok(cache_root().map(|root| root.join(native_manifest::TAG).join(platform)))
}

fn dir_if_complete(dir: &Path) -> Option<PathBuf> {
    if NATIVE_LIBRARY_FILE_NAMES
        .iter()
        .all(|f| dir.join(f).is_file())
    {
        Some(dir.to_path_buf())
    } else {
        None
    }
}

/// Candidate directories that may already hold the libraries, without
/// triggering a download.  The `bin/<platform>` entry only exists in a
/// development checkout of this crate (`CARGO_MANIFEST_DIR` points into the
/// cargo registry for a released build, where no `bin/` is packaged).
fn search_candidates() -> Result<Vec<PathBuf>, LoadError> {
    let mut candidates = Vec::new();
    if let Some(dir) = env_non_empty(NATIVE_DIR_ENV_VAR) {
        let dir = PathBuf::from(dir);
        if !dir.is_dir() {
            return Err(LoadError::OverrideMissing(dir));
        }
        candidates.push(dir);
    }
    let platform = platform_directory_name()?;
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("bin")
            .join(platform),
    );
    if let Some(dir) = cache_platform_directory()? {
        candidates.push(dir);
    }
    Ok(candidates)
}

fn sha256_hex(path: &Path) -> Result<String, LoadError> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| io_error(path, e))?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn run_tool(program: &str, args: &[&str]) -> Result<(), String> {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("could not run {program}: {e}")),
    }
}

/// Download and unpack the prebuilt native libraries for this platform into
/// the per-user cache, verifying the archive checksum, and return the
/// directory holding them.  Called automatically on first use unless
/// `OPENDAQ_NO_DOWNLOAD` is set; call it yourself to prefetch.
pub fn install_native_libraries() -> Result<PathBuf, LoadError> {
    let (_, file, expected_sha) = manifest_archive()?;
    let target_dir = cache_platform_directory()?.ok_or_else(|| LoadError::NotFound {
        checked: vec![],
        hint: " No per-user cache directory could be determined (HOME/LOCALAPPDATA unset).".into(),
    })?;

    if let Some(dir) = dir_if_complete(&target_dir) {
        return Ok(dir);
    }

    let (url, verify) = match env_non_empty(ARCHIVE_URL_ENV_VAR) {
        Some(mirror) => (mirror, false),
        None => (format!("{}{}", native_manifest::BASE_URL, file), true),
    };

    std::fs::create_dir_all(&target_dir).map_err(|e| io_error(&target_dir, e))?;
    let archive_path = target_dir.join(file);
    let archive_str = archive_path.to_string_lossy().into_owned();

    run_tool("curl", &["-fsSL", "--retry", "3", "-o", &archive_str, &url]).map_err(|reason| {
        LoadError::DownloadFailed {
            url: url.clone(),
            reason,
        }
    })?;

    if verify {
        let actual = sha256_hex(&archive_path)?;
        if actual != expected_sha {
            let _ = std::fs::remove_file(&archive_path);
            return Err(LoadError::ChecksumMismatch {
                file: file.to_string(),
                expected: expected_sha.to_string(),
                actual,
            });
        }
    }

    let target_str = target_dir.to_string_lossy().into_owned();
    run_tool("tar", &["-xzf", &archive_str, "-C", &target_str]).map_err(|reason| {
        LoadError::UnpackFailed {
            archive: archive_path.clone(),
            reason,
        }
    })?;
    let _ = std::fs::remove_file(&archive_path);

    dir_if_complete(&target_dir).ok_or_else(|| LoadError::NotFound {
        checked: vec![target_dir],
        hint: " The downloaded archive did not contain the expected libraries.".into(),
    })
}

/// The directory holding the native openDAQ libraries, downloading them into
/// the per-user cache when nothing local is found (and downloads are allowed).
pub fn native_library_directory() -> Result<PathBuf, LoadError> {
    let candidates = search_candidates()?;
    for dir in &candidates {
        if let Some(found) = dir_if_complete(dir) {
            return Ok(found);
        }
    }
    if downloads_disabled() {
        return Err(LoadError::NotFound {
            checked: candidates,
            hint: format!(
                " Automatic download is disabled because {NO_DOWNLOAD_ENV_VAR} is set; unset it \
                 or call opendaq::install_native_libraries() yourself."
            ),
        });
    }
    install_native_libraries().map_err(|e| match e {
        LoadError::NotFound { mut checked, hint } => {
            checked.extend(candidates);
            LoadError::NotFound { checked, hint }
        }
        other => other,
    })
}

/// The loaded libraries, in dependency order, plus the directory they came
/// from.  The `libloading::Library` handles are intentionally leaked by the
/// caller: openDAQ libraries are not designed to be unloaded from a process.
pub struct LoadedLibraries {
    #[allow(dead_code)]
    pub directory: PathBuf,
    pub libraries: Vec<libloading::Library>,
}

/// Load the native libraries from `directory` in dependency order.
pub fn load_from(directory: &Path) -> Result<LoadedLibraries, LoadError> {
    let mut libraries = Vec::with_capacity(NATIVE_LIBRARY_FILE_NAMES.len());
    for file in NATIVE_LIBRARY_FILE_NAMES {
        let path = directory.join(file);
        if !path.is_file() {
            return Err(LoadError::MissingLibrary {
                file: file.to_string(),
                dir: directory.into(),
            });
        }
        let library = unsafe { libloading::Library::new(&path) }.map_err(|e| {
            LoadError::LibraryLoadFailed {
                library: path,
                reason: e.to_string(),
            }
        })?;
        libraries.push(library);
    }
    Ok(LoadedLibraries {
        directory: directory.to_path_buf(),
        libraries,
    })
}

/// Resolve `symbol` (a NUL-terminated name) against the loaded libraries,
/// searching in reverse load order (`copendaq` first, where nearly every
/// symbol lives; the error-info and memory helpers live in `daqcoretypes`).
pub fn resolve_symbol(
    libraries: &[libloading::Library],
    symbol: &[u8],
) -> Result<*mut std::ffi::c_void, LoadError> {
    for library in libraries.iter().rev() {
        if let Ok(found) = unsafe { library.get::<*mut std::ffi::c_void>(symbol) } {
            // For T = *mut c_void, dereferencing the Symbol yields the raw
            // address of the symbol itself.
            let ptr: *mut std::ffi::c_void = *found;
            if !ptr.is_null() {
                return Ok(ptr);
            }
        }
    }
    Err(LoadError::MissingSymbol {
        symbol: String::from_utf8_lossy(symbol.strip_suffix(b"\0").unwrap_or(symbol)).into_owned(),
    })
}
