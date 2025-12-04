//! FFmpeg build script for CI environments
//!
//! Builds FFmpeg 8.0.1 and all codec dependencies from source for Linux/FreeBSD targets.
//! Usage: cargo run --release --bin build-ffmpeg -- [OPTIONS]

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// Version constants
const FFMPEG_VERSION: &str = "n8.0.1";
const X264_REPO: &str = "https://code.videolan.org/videolan/x264.git";
const X265_REPO: &str = "https://bitbucket.org/multicoreware/x265_git.git";
const X265_BRANCH: &str = "Release_4.1";
const VPX_REPO: &str = "https://chromium.googlesource.com/webm/libvpx";
const VPX_BRANCH: &str = "v1.15.2";
const AOM_REPO: &str = "https://aomedia.googlesource.com/aom";
const AOM_BRANCH: &str = "v3.13.1";
const OPUS_REPO: &str = "https://github.com/xiph/opus.git";
const OPUS_BRANCH: &str = "v1.5.2";
const LAME_URL: &str = "https://sourceforge.net/projects/lame/files/lame/3.100/lame-3.100.tar.gz";
const WEBP_REPO: &str = "https://chromium.googlesource.com/webm/libwebp";
const WEBP_BRANCH: &str = "v1.6.0";
const NV_CODEC_HEADERS_REPO: &str = "https://github.com/FFmpeg/nv-codec-headers.git";
const NV_CODEC_HEADERS_BRANCH: &str = "n13.0.19.0";
const FFMPEG_REPO: &str = "https://github.com/FFmpeg/FFmpeg.git";

/// Build context containing all configuration
struct BuildContext {
  /// Installation prefix for all libraries
  prefix: PathBuf,
  /// Directory for source code
  source_dir: PathBuf,
  /// Target triple for cross-compilation
  target: Option<String>,
  /// Number of parallel build jobs
  jobs: usize,
  /// Enable verbose output
  verbose: bool,
  /// Skip building dependencies
  skip_deps: bool,
  /// Use zig as compiler (if available and not overridden)
  use_zig: bool,
  /// Use system CC/CXX from environment variables (disables zig)
  use_system_cc: bool,
  /// Directory containing zig wrapper scripts (created lazily)
  zig_wrapper_dir: Option<PathBuf>,
}

/// Cross-compilation configuration (architecture/OS detection only, zig handles toolchain)
struct CrossCompileConfig {
  /// Target architecture (e.g., "aarch64", "x86_64")
  arch: String,
  /// Target OS (e.g., "linux")
  os: String,
}

impl CrossCompileConfig {
  fn from_target(target: &str) -> Option<Self> {
    // Parse Rust target triple: arch-vendor-os-env
    let parts: Vec<&str> = target.split('-').collect();
    if parts.len() < 3 {
      return None;
    }

    Some(Self {
      arch: parts[0].to_string(),
      os: parts[2].to_string(),
    })
  }

  /// Get FFmpeg architecture name
  fn ffmpeg_arch(&self) -> &str {
    match self.arch.as_str() {
      "aarch64" => "aarch64",
      "armv7" | "arm" => "arm",
      "x86_64" => "x86_64",
      "i686" | "i386" => "x86",
      _ => &self.arch,
    }
  }

  /// Get FFmpeg target OS name
  fn ffmpeg_os(&self) -> &str {
    match self.os.as_str() {
      "linux" => "linux",
      "freebsd" => "freebsd",
      _ => "linux",
    }
  }
}

impl BuildContext {
  fn new(
    prefix: PathBuf,
    source_dir: PathBuf,
    target: Option<String>,
    jobs: usize,
    verbose: bool,
    skip_deps: bool,
    use_system_cc: bool,
  ) -> Self {
    // Detect if zig is available (only if not using system CC)
    let use_zig = if use_system_cc {
      false
    } else {
      Command::new("zig")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    };

    Self {
      prefix,
      source_dir,
      target,
      jobs,
      verbose,
      skip_deps,
      use_zig,
      use_system_cc,
      zig_wrapper_dir: None,
    }
  }

  /// Ensure zig wrappers are created and return the wrapper directory
  fn ensure_zig_wrappers(&mut self) -> io::Result<PathBuf> {
    if let Some(ref dir) = self.zig_wrapper_dir {
      return Ok(dir.clone());
    }
    let dir = self.create_zig_wrappers()?;
    self.zig_wrapper_dir = Some(dir.clone());
    Ok(dir)
  }

  /// Create zig wrapper scripts that filter out incompatible flags (like -march=armv8)
  /// Returns the directory containing the wrapper scripts (absolute path)
  fn create_zig_wrappers(&self) -> io::Result<PathBuf> {
    let wrapper_dir = self.source_dir.join("zig-wrappers");
    fs::create_dir_all(&wrapper_dir)?;
    // Canonicalize to absolute path so it works from any directory
    let wrapper_dir = wrapper_dir.canonicalize()?;

    let zig_target = self.zig_target();
    let target_arg = zig_target
      .map(|t| format!("-target {}", t))
      .unwrap_or_default();

    // Create CC wrapper that transforms arch flags to zig-compatible format
    let cc_wrapper = wrapper_dir.join("cc");
    let cc_content = format!(
      r#"#!/bin/sh
# Zig CC wrapper - transforms arch flags to zig-compatible format
# Also filters out -target/--target flags that cmake/configure might add
args=""
cpu_features=""
skip_next=0
for arg in "$@"; do
  if [ "$skip_next" = "1" ]; then
    skip_next=0
    continue
  fi
  case "$arg" in
    -target|--target)
      # Skip this flag and the next argument (cmake/configure sometimes adds these)
      skip_next=1
      ;;
    -target=*|--target=*)
      # Skip combined target flag
      ;;
    -march=*|-mcpu=*|-mtune=*)
      # Extract features from flags like -march=armv8.2-a+dotprod+i8mm+crc
      case "$arg" in
        *+dotprod*) cpu_features="${{cpu_features}}+dotprod" ;;
      esac
      case "$arg" in
        *+i8mm*) cpu_features="${{cpu_features}}+i8mm" ;;
      esac
      case "$arg" in
        *+crc*) cpu_features="${{cpu_features}}+crc" ;;
      esac
      case "$arg" in
        *+sve2*) cpu_features="${{cpu_features}}+sve2" ;;
        *+sve*) cpu_features="${{cpu_features}}+sve" ;;
      esac
      case "$arg" in
        *+crypto*) cpu_features="${{cpu_features}}+crypto" ;;
      esac
      case "$arg" in
        *+aes*) cpu_features="${{cpu_features}}+aes" ;;
      esac
      case "$arg" in
        *+sha2*) cpu_features="${{cpu_features}}+sha2" ;;
      esac
      case "$arg" in
        *+sha3*) cpu_features="${{cpu_features}}+sha3" ;;
      esac
      ;; # Don't pass the original flag
    *) args="$args $arg" ;;
  esac
done
# Add extracted features via -mcpu if any were found
if [ -n "$cpu_features" ]; then
  args="-mcpu=generic$cpu_features $args"
fi
exec zig cc {} $args
"#,
      target_arg
    );
    fs::write(&cc_wrapper, cc_content)?;
    self.make_executable(&cc_wrapper)?;

    // Create CXX wrapper with same logic
    let cxx_wrapper = wrapper_dir.join("c++");
    let cxx_content = format!(
      r#"#!/bin/sh
# Zig C++ wrapper - transforms arch flags to zig-compatible format
# Also filters out -target/--target flags that cmake/configure might add
args=""
cpu_features=""
skip_next=0
for arg in "$@"; do
  if [ "$skip_next" = "1" ]; then
    skip_next=0
    continue
  fi
  case "$arg" in
    -target|--target)
      # Skip this flag and the next argument (cmake/configure sometimes adds these)
      skip_next=1
      ;;
    -target=*|--target=*)
      # Skip combined target flag
      ;;
    -march=*|-mcpu=*|-mtune=*)
      # Extract features from flags like -march=armv8.2-a+dotprod+i8mm+crc
      case "$arg" in
        *+dotprod*) cpu_features="${{cpu_features}}+dotprod" ;;
      esac
      case "$arg" in
        *+i8mm*) cpu_features="${{cpu_features}}+i8mm" ;;
      esac
      case "$arg" in
        *+crc*) cpu_features="${{cpu_features}}+crc" ;;
      esac
      case "$arg" in
        *+sve2*) cpu_features="${{cpu_features}}+sve2" ;;
        *+sve*) cpu_features="${{cpu_features}}+sve" ;;
      esac
      case "$arg" in
        *+crypto*) cpu_features="${{cpu_features}}+crypto" ;;
      esac
      case "$arg" in
        *+aes*) cpu_features="${{cpu_features}}+aes" ;;
      esac
      case "$arg" in
        *+sha2*) cpu_features="${{cpu_features}}+sha2" ;;
      esac
      case "$arg" in
        *+sha3*) cpu_features="${{cpu_features}}+sha3" ;;
      esac
      ;; # Don't pass the original flag
    *) args="$args $arg" ;;
  esac
done
# Add extracted features via -mcpu if any were found
if [ -n "$cpu_features" ]; then
  args="-mcpu=generic$cpu_features $args"
fi
exec zig c++ {} $args
"#,
      target_arg
    );
    fs::write(&cxx_wrapper, cxx_content)?;
    self.make_executable(&cxx_wrapper)?;

    Ok(wrapper_dir)
  }

  /// Make a file executable (Unix only)
  #[cfg(unix)]
  fn make_executable(&self, path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
  }

  #[cfg(not(unix))]
  fn make_executable(&self, _path: &Path) -> io::Result<()> {
    Ok(())
  }

  /// Fix the prefix in a .pc file to point to the install directory
  fn fix_pc_file_prefix(&self, content: &str, prefix: &str) -> String {
    let mut result = String::new();
    for line in content.lines() {
      if line.starts_with("prefix=") {
        result.push_str(&format!("prefix={}\n", prefix));
      } else {
        result.push_str(line);
        result.push('\n');
      }
    }
    result
  }

  /// Ensure a .pc file exists in pkgconfig directory
  /// If not found, try to copy from build dir or generate manually
  fn ensure_pc_file(
    &self,
    name: &str,
    build_dir: &Path,
    version: &str,
    description: &str,
    extra_cflags: &str,
    extra_libs: &str,
  ) -> io::Result<()> {
    let pkgconfig_dir = self.prefix.join("lib").join("pkgconfig");
    fs::create_dir_all(&pkgconfig_dir)?;

    let pc_filename = format!("{}.pc", name);
    let dest_pc = pkgconfig_dir.join(&pc_filename);
    let prefix_str = self.prefix.to_string_lossy().to_string();

    // If already exists with correct prefix, we're done
    if dest_pc.exists() {
      let content = fs::read_to_string(&dest_pc)?;
      if content.contains(&format!("prefix={}", prefix_str)) {
        self.log(&format!(
          "{} already exists with correct prefix",
          pc_filename
        ));
        return Ok(());
      }
      // Fix prefix if needed
      let fixed = self.fix_pc_file_prefix(&content, &prefix_str);
      fs::write(&dest_pc, fixed)?;
      self.log(&format!("Fixed prefix in {}", pc_filename));
      return Ok(());
    }

    // Try to find .pc in common build locations
    let possible_sources = [
      build_dir.join(&pc_filename),
      build_dir.join("lib").join("pkgconfig").join(&pc_filename),
      build_dir.join("pkgconfig").join(&pc_filename),
    ];

    for src in &possible_sources {
      if src.exists() {
        let content = fs::read_to_string(src)?;
        let fixed = self.fix_pc_file_prefix(&content, &prefix_str);
        fs::write(&dest_pc, fixed)?;
        self.log(&format!("Copied and fixed {} from {:?}", pc_filename, src));
        return Ok(());
      }
    }

    // Generate manually if not found
    let pc_content = format!(
      r#"prefix={}
exec_prefix=${{prefix}}
libdir=${{exec_prefix}}/lib
includedir=${{prefix}}/include

Name: {}
Description: {}
Version: {}
Libs: -L${{libdir}} -l{}{}
Cflags: -I${{includedir}}{}
"#,
      prefix_str,
      name,
      description,
      version,
      name,
      if extra_libs.is_empty() {
        String::new()
      } else {
        format!(" {}", extra_libs)
      },
      if extra_cflags.is_empty() {
        String::new()
      } else {
        format!(" {}", extra_cflags)
      },
    );
    fs::write(&dest_pc, pc_content)?;
    self.log(&format!("Generated {} manually", pc_filename));
    Ok(())
  }

  /// Get the zig target triple for the current target
  fn zig_target(&self) -> Option<String> {
    self.target.as_ref().map(|t| {
      // Convert Rust target to zig target
      // e.g., "aarch64-unknown-linux-gnu" -> "aarch64-linux-gnu.2.17.0"
      // e.g., "x86_64-unknown-linux-musl" -> "x86_64-linux-musl"
      // e.g., "armv7-unknown-linux-gnueabihf" -> "arm-linux-gnueabihf.2.17.0"
      // Key: Remove "unknown" vendor, keep dot before glibc version
      let parts: Vec<&str> = t.split('-').collect();

      // Convert architecture name to zig format
      // Zig uses "arm" instead of "armv7" for 32-bit ARM
      let arch = match parts[0] {
        "armv7" => "arm",
        other => other,
      };

      let zig_target = if parts.len() >= 4 {
        // arch-vendor-os-env -> arch-os-env (remove vendor like "unknown")
        format!("{}-{}-{}", arch, parts[2], parts[3])
      } else if parts.len() == 3 {
        // arch-os-env -> arch-os-env
        format!("{}-{}-{}", arch, parts[1], parts[2])
      } else {
        t.clone()
      };

      // For gnu targets, specify glibc 2.17.0 for compatibility with older distros
      // Format: "aarch64-linux-gnu.2.17.0" (dot before version, full 3-part version)
      if zig_target.ends_with("-gnu") || zig_target.ends_with("-gnueabihf") {
        format!("{}.2.17.0", zig_target)
      } else {
        zig_target
      }
    })
  }

  /// Get C compiler command (uses zig wrapper for cross-compilation, or system CC)
  fn get_cc(&self) -> String {
    // If using system CC, check environment variable first
    if self.use_system_cc {
      return env::var("CC").unwrap_or_else(|_| "cc".to_string());
    }

    if self.use_zig {
      if let Some(ref wrapper_dir) = self.zig_wrapper_dir {
        // Use wrapper script that filters incompatible flags
        wrapper_dir.join("cc").to_string_lossy().to_string()
      } else if let Some(zig_target) = self.zig_target() {
        format!("zig cc -target {}", zig_target)
      } else {
        "zig cc".to_string()
      }
    } else {
      "cc".to_string()
    }
  }

  /// Get C++ compiler command (uses zig wrapper for cross-compilation, or system CXX)
  fn get_cxx(&self) -> String {
    // If using system CC, check environment variable first
    if self.use_system_cc {
      return env::var("CXX").unwrap_or_else(|_| "c++".to_string());
    }

    if self.use_zig {
      if let Some(ref wrapper_dir) = self.zig_wrapper_dir {
        // Use wrapper script that filters incompatible flags
        wrapper_dir.join("c++").to_string_lossy().to_string()
      } else if let Some(zig_target) = self.zig_target() {
        format!("zig c++ -target {}", zig_target)
      } else {
        "zig c++".to_string()
      }
    } else {
      "c++".to_string()
    }
  }

  /// Get AR command
  fn get_ar(&self) -> String {
    // If using system CC, check environment variable first
    if self.use_system_cc {
      return env::var("AR").unwrap_or_else(|_| "ar".to_string());
    }

    if self.use_zig {
      "zig ar".to_string()
    } else {
      "ar".to_string()
    }
  }

  /// Get RANLIB command
  fn get_ranlib(&self) -> String {
    // If using system CC, check environment variable first
    if self.use_system_cc {
      return env::var("RANLIB").unwrap_or_else(|_| "ranlib".to_string());
    }

    if self.use_zig {
      "zig ranlib".to_string()
    } else {
      "ranlib".to_string()
    }
  }

  /// Get AS (assembler) command
  fn get_as(&self) -> String {
    // If using system CC, check environment variable first
    if self.use_system_cc {
      return env::var("AS").unwrap_or_else(|_| "as".to_string());
    }

    // When using zig, use CC as assembler (zig cc handles .S files)
    self.get_cc()
  }

  /// Get STRIP command
  fn get_strip(&self) -> String {
    // If using system CC, check environment variable first
    if self.use_system_cc {
      return env::var("STRIP").unwrap_or_else(|_| "strip".to_string());
    }

    // For zig cross-compilation, use no-op to avoid "unable to recognise format" warnings
    // zig doesn't have a strip command, and host strip can't handle cross-compiled objects
    if self.target.is_some() && self.use_zig {
      "true".to_string() // no-op command
    } else {
      "strip".to_string()
    }
  }

  /// Get cross-compilation config if cross-compiling
  fn cross_config(&self) -> Option<CrossCompileConfig> {
    self
      .target
      .as_ref()
      .and_then(|t| CrossCompileConfig::from_target(t))
  }

  /// Get common cmake arguments for cross-compilation
  fn cmake_cross_args(&self) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(cross) = self.cross_config() {
      args.push(format!(
        "-DCMAKE_SYSTEM_NAME={}",
        match cross.os.as_str() {
          "linux" => "Linux",
          "freebsd" => "FreeBSD",
          _ => "Linux",
        }
      ));
      args.push(format!("-DCMAKE_SYSTEM_PROCESSOR={}", cross.arch));

      // For system CC (GCC cross-toolchain), provide pthread/math hints
      // to avoid cmake test failures during cross-compilation
      if self.use_system_cc {
        // Tell cmake the compiler can link basic executables (skip try_compile tests)
        args.push("-DCMAKE_C_COMPILER_WORKS=ON".to_string());
        args.push("-DCMAKE_CXX_COMPILER_WORKS=ON".to_string());
        // Tell cmake that pthread works (it's built into glibc)
        args.push("-DTHREADS_PREFER_PTHREAD_FLAG=ON".to_string());
        // Pre-set pthread test result (0 = success, avoids try_run)
        args.push("-DTHREADS_PTHREAD_ARG=0".to_string());
        // Tell cmake not to try running test executables
        args.push("-DCMAKE_CROSSCOMPILING_EMULATOR=".to_string());
      }
    }
    args
  }

  /// Log a message if verbose mode is enabled
  fn log(&self, msg: &str) {
    if self.verbose {
      println!("[build-ffmpeg] {}", msg);
    }
  }

  /// Log a message always
  fn info(&self, msg: &str) {
    println!("[build-ffmpeg] {}", msg);
  }

  /// Run a command and return its output
  fn run_command(&self, cmd: &mut Command) -> io::Result<()> {
    self.log(&format!("Running: {:?}", cmd));

    let status = if self.verbose {
      cmd.status()?
    } else {
      cmd.stdout(Stdio::null()).stderr(Stdio::null()).status()?
    };

    if status.success() {
      Ok(())
    } else {
      Err(io::Error::new(
        io::ErrorKind::Other,
        format!("Command failed with status: {}", status),
      ))
    }
  }

  /// Run a command with visible output (always show)
  fn run_command_visible(&self, cmd: &mut Command) -> io::Result<()> {
    self.log(&format!("Running: {:?}", cmd));
    let status = cmd.status()?;
    if status.success() {
      Ok(())
    } else {
      Err(io::Error::new(
        io::ErrorKind::Other,
        format!("Command failed with status: {}", status),
      ))
    }
  }

  /// Git clone a repository
  fn git_clone(&self, url: &str, branch: Option<&str>, dest: &Path) -> io::Result<()> {
    if dest.exists() {
      self.log(&format!("Source already exists: {}", dest.display()));
      return Ok(());
    }

    let mut cmd = Command::new("git");
    cmd.args(["clone", "--depth=1"]);

    if let Some(b) = branch {
      cmd.args(["-b", b]);
    }

    cmd.arg(url).arg(dest);
    self.run_command_visible(&mut cmd)
  }

  /// Download and extract a tarball
  fn download_tarball(&self, url: &str, dest: &Path) -> io::Result<()> {
    if dest.exists() {
      self.log(&format!("Source already exists: {}", dest.display()));
      return Ok(());
    }

    let tarball = self.source_dir.join("download.tar.gz");

    // Download
    self.info(&format!("Downloading {}", url));
    let mut cmd = Command::new("curl");
    cmd
      .args(["-L", "-o"])
      .arg(&tarball)
      .arg(url)
      .current_dir(&self.source_dir);
    self.run_command_visible(&mut cmd)?;

    // Extract
    let mut cmd = Command::new("tar");
    cmd
      .args(["xzf"])
      .arg(&tarball)
      .current_dir(&self.source_dir);
    self.run_command(&mut cmd)?;

    // Clean up
    fs::remove_file(&tarball)?;

    Ok(())
  }

  /// Run configure script
  fn run_configure(&self, dir: &Path, args: &[&str]) -> io::Result<()> {
    let mut cmd = Command::new("./configure");
    cmd.args(args).current_dir(dir);

    // Set PKG_CONFIG_PATH to find our built dependencies
    // Check multiple possible locations (lib, lib64, share)
    let mut pkg_paths = vec![
      self.prefix.join("lib").join("pkgconfig"),
      self.prefix.join("lib64").join("pkgconfig"),
      self.prefix.join("share").join("pkgconfig"),
    ];
    let existing_pkg_path = env::var("PKG_CONFIG_PATH").unwrap_or_default();
    if !existing_pkg_path.is_empty() {
      pkg_paths.push(PathBuf::from(&existing_pkg_path));
    }
    let full_pkg_path = pkg_paths
      .iter()
      .map(|p| p.to_string_lossy().to_string())
      .collect::<Vec<_>>()
      .join(":");
    cmd.env("PKG_CONFIG_PATH", &full_pkg_path);

    // For cross-compilation, set PKG_CONFIG_LIBDIR to only search our prefix
    // Note: Don't set PKG_CONFIG_SYSROOT_DIR because our .pc files already have
    // absolute paths, and sysroot would cause doubled paths
    if self.target.is_some() {
      cmd.env("PKG_CONFIG_LIBDIR", &full_pkg_path);
    }

    // Set compiler environment variables
    cmd.env("CC", self.get_cc());
    cmd.env("CXX", self.get_cxx());
    cmd.env("AR", self.get_ar());
    cmd.env("RANLIB", self.get_ranlib());

    // Pass through CFLAGS/CXXFLAGS from environment
    if let Ok(cflags) = env::var("CFLAGS") {
      cmd.env("CFLAGS", &cflags);
    }
    if let Ok(cxxflags) = env::var("CXXFLAGS") {
      cmd.env("CXXFLAGS", &cxxflags);
    }

    // Only set AS/CCAS for ARM targets where .S files use the C compiler
    // For x86, don't override AS so build systems can find nasm
    let is_arm = self
      .target
      .as_ref()
      .map(|t| t.contains("arm") || t.contains("aarch64"))
      .unwrap_or(false);
    if is_arm || self.use_system_cc {
      cmd.env("AS", self.get_as());
      cmd.env("CCAS", self.get_as());
    }

    // Set STRIP to no-op for zig cross-compilation to avoid "unable to recognise format" warnings
    // Stripping static libraries is unnecessary; final strip happens on the Rust binary
    cmd.env("STRIP", self.get_strip());

    self.run_command_visible(&mut cmd)
  }

  /// Run make
  fn run_make(&self, dir: &Path) -> io::Result<()> {
    let mut cmd = Command::new("make");
    cmd.arg(format!("-j{}", self.jobs)).current_dir(dir);
    self.run_command_visible(&mut cmd)
  }

  /// Run make install
  fn run_make_install(&self, dir: &Path) -> io::Result<()> {
    let mut cmd = Command::new("make");
    cmd.arg("install").current_dir(dir);
    self.run_command(&mut cmd)
  }

  /// Run cmake
  fn run_cmake(&self, source_dir: &Path, build_dir: &Path, args: &[&str]) -> io::Result<()> {
    fs::create_dir_all(build_dir)?;

    // Canonicalize source_dir to absolute path (cmake runs from build_dir)
    let source_dir_abs = source_dir.canonicalize()?;

    let mut cmd = Command::new("cmake");
    cmd.arg("-G").arg("Unix Makefiles").args(args);

    // Pass CFLAGS/CXXFLAGS to cmake if set in environment
    if let Ok(cflags) = env::var("CFLAGS") {
      cmd.arg(format!("-DCMAKE_C_FLAGS={}", cflags));
    }
    if let Ok(cxxflags) = env::var("CXXFLAGS") {
      cmd.arg(format!("-DCMAKE_CXX_FLAGS={}", cxxflags));
    }

    cmd.arg(&source_dir_abs).current_dir(build_dir);

    // Set compiler via environment variables
    cmd.env("CC", self.get_cc());
    cmd.env("CXX", self.get_cxx());
    cmd.env("AR", self.get_ar());
    cmd.env("RANLIB", self.get_ranlib());

    // Only set AS/CCAS for ARM targets where .S files use the C compiler
    // For x86, don't override AS so build systems can find nasm
    let is_arm = self
      .target
      .as_ref()
      .map(|t| t.contains("arm") || t.contains("aarch64"))
      .unwrap_or(false);
    if is_arm || self.use_system_cc {
      cmd.env("AS", self.get_as());
      cmd.env("CCAS", self.get_as());
    }

    // Set STRIP to no-op for zig cross-compilation to avoid "unable to recognise format" warnings
    // Stripping static libraries is unnecessary; final strip happens on the Rust binary
    cmd.env("STRIP", self.get_strip());

    self.run_command_visible(&mut cmd)
  }

  /// Run cmake --build
  fn run_cmake_build(&self, build_dir: &Path) -> io::Result<()> {
    let mut cmd = Command::new("cmake");
    cmd
      .args(["--build", "."])
      .arg(format!("-j{}", self.jobs))
      .current_dir(build_dir);
    self.run_command_visible(&mut cmd)
  }

  /// Run cmake --install
  fn run_cmake_install(&self, build_dir: &Path) -> io::Result<()> {
    let mut cmd = Command::new("cmake");
    cmd.args(["--install", "."]).current_dir(build_dir);
    self.run_command(&mut cmd)
  }

  /// Build x264
  fn build_x264(&self) -> io::Result<()> {
    self.info("Building x264...");

    let source = self.source_dir.join("x264");
    self.git_clone(X264_REPO, None, &source)?;

    let prefix_str = self.prefix.to_string_lossy();
    let mut args = vec![
      format!("--prefix={}", prefix_str),
      "--enable-static".to_string(),
      "--disable-cli".to_string(),
      "--disable-opencl".to_string(),
      "--enable-pic".to_string(),
    ];

    // Cross-compilation: zig handles toolchain via CC/CXX, just need --host for config detection
    if let Some(target) = &self.target {
      args.push(format!("--host={}", target));
    }

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_configure(&source, &args_refs)?;
    self.run_make(&source)?;
    self.run_make_install(&source)?;

    self.info("x264 built successfully");
    Ok(())
  }

  /// Build x265
  fn build_x265(&self) -> io::Result<()> {
    self.info("Building x265...");

    let source = self.source_dir.join("x265_git");
    self.git_clone(X265_REPO, Some(X265_BRANCH), &source)?;

    // Fetch tags for version detection (shallow clone doesn't include tags)
    let mut cmd = Command::new("git");
    cmd
      .args(["fetch", "--tags", "--depth=1"])
      .current_dir(&source);
    let _ = self.run_command(&mut cmd); // Ignore errors if already fetched

    let build_dir = source.join("build").join("custom");
    fs::create_dir_all(&build_dir)?;

    let prefix_str = self.prefix.to_string_lossy().to_string();
    let cmake_source = source.join("source");

    let mut args = vec![
      format!("-DCMAKE_INSTALL_PREFIX={}", prefix_str),
      "-DENABLE_SHARED=OFF".to_string(),
      "-DENABLE_CLI=OFF".to_string(),
      "-DHIGH_BIT_DEPTH=ON".to_string(),
      "-DCMAKE_POSITION_INDEPENDENT_CODE=ON".to_string(),
    ];

    // Add cross-compilation hints for CMake
    args.extend(self.cmake_cross_args());

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_cmake(&cmake_source, &build_dir, &args_refs)?;
    self.run_cmake_build(&build_dir)?;
    self.run_cmake_install(&build_dir)?;

    // Generate x265.pc manually (CMake doesn't always generate it)
    let pkgconfig_dir = self.prefix.join("lib").join("pkgconfig");
    fs::create_dir_all(&pkgconfig_dir)?;
    let prefix_str = self.prefix.to_string_lossy().to_string();
    let x265_pc = format!(
      r#"prefix={}
exec_prefix=${{prefix}}
libdir=${{exec_prefix}}/lib
includedir=${{prefix}}/include

Name: x265
Description: H.265/HEVC video encoder
Version: {}
Libs: -L${{libdir}} -lx265
Libs.private: -lstdc++ -lm -lpthread
Cflags: -I${{includedir}}
"#,
      prefix_str,
      X265_BRANCH.trim_start_matches("Release_")
    );
    fs::write(pkgconfig_dir.join("x265.pc"), x265_pc)?;
    self.log("Generated x265.pc");

    self.info("x265 built successfully");
    Ok(())
  }

  /// Build libvpx
  fn build_vpx(&self) -> io::Result<()> {
    self.info("Building libvpx...");

    let source = self.source_dir.join("libvpx");
    self.git_clone(VPX_REPO, Some(VPX_BRANCH), &source)?;

    let prefix_str = self.prefix.to_string_lossy();
    let mut args = vec![
      format!("--prefix={}", prefix_str),
      "--enable-static".to_string(),
      "--disable-shared".to_string(),
      "--disable-examples".to_string(),
      "--disable-tools".to_string(),
      "--disable-docs".to_string(),
      "--disable-unit-tests".to_string(), // Avoid gtest ABI issues with zig
      "--enable-pic".to_string(),
      "--enable-vp8".to_string(),
      "--enable-vp9".to_string(),
      "--enable-vp9-highbitdepth".to_string(),
    ];

    // Cross-compilation: set libvpx target based on architecture
    if let Some(cross) = self.cross_config() {
      let vpx_target = match cross.arch.as_str() {
        "aarch64" => "arm64-linux-gcc",
        // armv7 uses generic-gnu to avoid -march=armv7-a ASFLAGS issues with zig
        // This uses C implementations instead of ARM assembly, but works reliably
        "armv7" | "arm" => "generic-gnu",
        "x86_64" => "x86_64-linux-gcc",
        _ => "generic-gnu",
      };
      args.push(format!("--target={}", vpx_target));

      // ARM SIMD optimizations - zig wrapper transforms -march flags to -mcpu=generic+feature
      // Note: armv7 uses generic-gnu target so NEON is not available via assembly
      match cross.arch.as_str() {
        "aarch64" => {
          args.push("--enable-neon".to_string());
          args.push("--enable-neon-dotprod".to_string());
          args.push("--enable-neon-i8mm".to_string());
          args.push("--enable-sve".to_string());
          args.push("--enable-sve2".to_string());
        }
        _ => {}
      }
    }

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_configure(&source, &args_refs)?;
    self.run_make(&source)?;
    self.run_make_install(&source)?;

    self.info("libvpx built successfully");
    Ok(())
  }

  /// Build libaom
  fn build_aom(&self) -> io::Result<()> {
    self.info("Building libaom...");

    let source = self.source_dir.join("aom");
    self.git_clone(AOM_REPO, Some(AOM_BRANCH), &source)?;

    let build_dir = source.join("build");
    fs::create_dir_all(&build_dir)?;

    let prefix_str = self.prefix.to_string_lossy().to_string();

    let mut args = vec![
      format!("-DCMAKE_INSTALL_PREFIX={}", prefix_str),
      "-DBUILD_SHARED_LIBS=OFF".to_string(),
      "-DENABLE_EXAMPLES=OFF".to_string(),
      "-DENABLE_TOOLS=OFF".to_string(),
      "-DENABLE_DOCS=OFF".to_string(),
      "-DENABLE_TESTS=OFF".to_string(),
      "-DCONFIG_PIC=1".to_string(),
      "-DCMAKE_POSITION_INDEPENDENT_CODE=ON".to_string(),
    ];

    // Add cross-compilation hints for CMake
    args.extend(self.cmake_cross_args());

    // Target-specific SIMD optimization
    // Zig wrapper transforms -march flags to -mcpu=generic+feature
    if let Some(cross) = self.cross_config() {
      match cross.arch.as_str() {
        "aarch64" => {
          args.push("-DAOM_TARGET_CPU=arm64".to_string());
          args.push("-DENABLE_NEON=ON".to_string());
          args.push("-DENABLE_SVE=ON".to_string());
          args.push("-DENABLE_SVE2=ON".to_string());
        }
        "armv7" | "arm" => {
          args.push("-DAOM_TARGET_CPU=arm".to_string());
          args.push("-DENABLE_NEON=ON".to_string());
        }
        _ => {}
      }
    }

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_cmake(&source, &build_dir, &args_refs)?;
    self.run_cmake_build(&build_dir)?;
    self.run_cmake_install(&build_dir)?;

    // Ensure aom.pc exists
    self.ensure_pc_file(
      "aom",
      &build_dir,
      AOM_BRANCH.trim_start_matches('v'),
      "AV1 codec library",
      "",
      "-lm -lpthread",
    )?;

    self.info("libaom built successfully");
    Ok(())
  }

  /// Build opus
  fn build_opus(&self) -> io::Result<()> {
    self.info("Building opus...");

    let source = self.source_dir.join("opus");
    self.git_clone(OPUS_REPO, Some(OPUS_BRANCH), &source)?;

    let build_dir = source.join("build");
    fs::create_dir_all(&build_dir)?;

    let prefix_str = self.prefix.to_string_lossy().to_string();

    let mut args = vec![
      format!("-DCMAKE_INSTALL_PREFIX={}", prefix_str),
      "-DBUILD_SHARED_LIBS=OFF".to_string(),
      "-DCMAKE_POSITION_INDEPENDENT_CODE=ON".to_string(),
    ];

    // Add cross-compilation hints for CMake
    args.extend(self.cmake_cross_args());

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_cmake(&source, &build_dir, &args_refs)?;
    self.run_cmake_build(&build_dir)?;
    self.run_cmake_install(&build_dir)?;

    // Ensure opus.pc exists
    self.ensure_pc_file(
      "opus",
      &build_dir,
      OPUS_BRANCH.trim_start_matches('v'),
      "Opus audio codec",
      "-I${includedir}/opus",
      "-lm",
    )?;

    self.info("opus built successfully");
    Ok(())
  }

  /// Build lame
  fn build_lame(&self) -> io::Result<()> {
    self.info("Building lame...");

    let source = self.source_dir.join("lame-3.100");
    self.download_tarball(LAME_URL, &source)?;

    let prefix_str = self.prefix.to_string_lossy();
    let mut args = vec![
      format!("--prefix={}", prefix_str),
      "--enable-static".to_string(),
      "--disable-shared".to_string(),
      "--disable-frontend".to_string(),
      "--with-pic".to_string(),
    ];

    // Cross-compilation: zig handles toolchain via CC/CXX, just need --host for config detection
    if let Some(target) = &self.target {
      args.push(format!("--host={}", target));
    }

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_configure(&source, &args_refs)?;
    self.run_make(&source)?;
    self.run_make_install(&source)?;

    // Generate mp3lame.pc (lame doesn't generate one)
    let pkgconfig_dir = self.prefix.join("lib").join("pkgconfig");
    fs::create_dir_all(&pkgconfig_dir)?;
    let lame_pc = format!(
      r#"prefix={}
exec_prefix=${{prefix}}
libdir=${{exec_prefix}}/lib
includedir=${{prefix}}/include

Name: mp3lame
Description: LAME MP3 encoder library
Version: 3.100
Libs: -L${{libdir}} -lmp3lame
Cflags: -I${{includedir}}
"#,
      self.prefix.to_string_lossy()
    );
    fs::write(pkgconfig_dir.join("mp3lame.pc"), lame_pc)?;
    self.log("Generated mp3lame.pc");

    self.info("lame built successfully");
    Ok(())
  }

  /// Build libwebp
  fn build_webp(&self) -> io::Result<()> {
    self.info("Building libwebp...");

    let source = self.source_dir.join("libwebp");
    self.git_clone(WEBP_REPO, Some(WEBP_BRANCH), &source)?;

    let build_dir = source.join("build");
    fs::create_dir_all(&build_dir)?;

    let prefix_str = self.prefix.to_string_lossy().to_string();

    let mut args = vec![
      format!("-DCMAKE_INSTALL_PREFIX={}", prefix_str),
      "-DBUILD_SHARED_LIBS=OFF".to_string(),
      "-DWEBP_BUILD_ANIM_UTILS=OFF".to_string(),
      "-DWEBP_BUILD_CWEBP=OFF".to_string(),
      "-DWEBP_BUILD_DWEBP=OFF".to_string(),
      "-DWEBP_BUILD_GIF2WEBP=OFF".to_string(),
      "-DWEBP_BUILD_IMG2WEBP=OFF".to_string(),
      "-DWEBP_BUILD_VWEBP=OFF".to_string(),
      "-DWEBP_BUILD_WEBPINFO=OFF".to_string(),
      "-DWEBP_BUILD_WEBPMUX=OFF".to_string(),
      "-DWEBP_BUILD_EXTRAS=OFF".to_string(),
      "-DCMAKE_POSITION_INDEPENDENT_CODE=ON".to_string(),
    ];

    // Add cross-compilation hints for CMake
    args.extend(self.cmake_cross_args());

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_cmake(&source, &build_dir, &args_refs)?;
    self.run_cmake_build(&build_dir)?;
    self.run_cmake_install(&build_dir)?;

    // CMake doesn't always install .pc files, copy and fix them manually
    // Each .pc file is generated in a different subdirectory
    let pkgconfig_dir = self.prefix.join("lib").join("pkgconfig");
    fs::create_dir_all(&pkgconfig_dir)?;
    let prefix_str = self.prefix.to_string_lossy().to_string();
    let pc_files = [
      ("libwebp.pc", "src"),
      ("libwebpdecoder.pc", "src"),
      ("libwebpmux.pc", "src/mux"),
      ("libwebpdemux.pc", "src/demux"),
      ("libsharpyuv.pc", "sharpyuv"),
    ];
    for (pc_file, subdir) in pc_files {
      let src = build_dir.join(subdir).join(pc_file);
      if src.exists() {
        let dest = pkgconfig_dir.join(pc_file);
        // Read, fix prefix and add -lm for math functions, and write
        let content = fs::read_to_string(&src)?;
        let mut fixed = self.fix_pc_file_prefix(&content, &prefix_str);
        // libsharpyuv uses math functions (pow, sqrtf) so needs -lm
        // Replace the entire Libs.private line (may have trailing whitespace)
        if pc_file == "libsharpyuv.pc" {
          let mut lines: Vec<&str> = fixed.lines().collect();
          for line in &mut lines {
            if line.starts_with("Libs.private:") {
              *line = "Libs.private: -lm";
            }
          }
          fixed = lines.join("\n") + "\n";
        }
        fs::write(&dest, fixed)?;
        self.log(&format!("Copied and fixed {} to pkgconfig", pc_file));
      } else {
        self.log(&format!(
          "Warning: {} not found at {}, skipping",
          pc_file,
          src.display()
        ));
      }
    }

    self.info("libwebp built successfully");
    Ok(())
  }

  /// Install NVIDIA codec headers (for NVENC/NVDEC support)
  fn install_nv_codec_headers(&self) -> io::Result<()> {
    self.info("Installing nv-codec-headers...");

    let source = self.source_dir.join("nv-codec-headers");
    self.git_clone(
      NV_CODEC_HEADERS_REPO,
      Some(NV_CODEC_HEADERS_BRANCH),
      &source,
    )?;

    // nv-codec-headers uses a simple Makefile with PREFIX
    let mut cmd = Command::new("make");
    cmd
      .arg(format!("PREFIX={}", self.prefix.display()))
      .arg("install")
      .current_dir(&source);
    self.run_command_visible(&mut cmd)?;

    self.info("nv-codec-headers installed successfully");
    Ok(())
  }

  /// Build FFmpeg
  fn build_ffmpeg(&self) -> io::Result<()> {
    self.info("Building FFmpeg...");

    let source = self.source_dir.join("FFmpeg");
    self.git_clone(FFMPEG_REPO, Some(FFMPEG_VERSION), &source)?;

    let prefix_str = self.prefix.to_string_lossy().to_string();
    let include_dir = self.prefix.join("include");
    let lib_dir = self.prefix.join("lib");

    let mut args = vec![
      format!("--prefix={}", prefix_str),
      "--enable-static".to_string(),
      "--disable-shared".to_string(),
      "--enable-pic".to_string(),
      "--disable-programs".to_string(),
      "--disable-doc".to_string(),
      "--disable-autodetect".to_string(),
      "--enable-gpl".to_string(),
      "--enable-version3".to_string(),
      // Core libraries
      "--enable-avcodec".to_string(),
      "--enable-avutil".to_string(),
      "--enable-swscale".to_string(),
      "--enable-swresample".to_string(),
      // Disabled libraries
      "--disable-avformat".to_string(),
      "--disable-avfilter".to_string(),
      "--disable-avdevice".to_string(),
      "--disable-network".to_string(),
      // Video codecs
      "--enable-libx264".to_string(),
      "--enable-libx265".to_string(),
      "--enable-libvpx".to_string(),
      "--enable-libaom".to_string(),
      // Audio codecs
      "--enable-libopus".to_string(),
      "--enable-libmp3lame".to_string(),
      // Image codecs
      "--enable-libwebp".to_string(),
      // Include/lib paths
      format!("--extra-cflags=-I{} -fPIC", include_dir.display()),
      format!("--extra-ldflags=-L{}", lib_dir.display()),
      "--pkg-config-flags=--static".to_string(),
    ];

    // Hardware acceleration (Linux only, runtime detection)
    let is_linux = self
      .target
      .as_ref()
      .map(|t| t.contains("linux"))
      .unwrap_or(cfg!(target_os = "linux"));

    if is_linux {
      // Check architecture for hardware acceleration support
      let is_armv7 = self
        .target
        .as_ref()
        .map(|t| t.starts_with("armv7") || t.starts_with("arm-"))
        .unwrap_or(false);
      let is_arm = self
        .target
        .as_ref()
        .map(|t| t.contains("arm") || t.contains("aarch64"))
        .unwrap_or(false);

      // VAAPI - Intel/AMD hardware acceleration (not for ARM)
      if !is_arm && self.check_vaapi_available() {
        self.info("Enabling VAAPI hardware acceleration...");
        args.push("--enable-vaapi".to_string());
      }

      // NVENC/NVDEC - NVIDIA hardware acceleration (x86_64 and aarch64 only, not armv7)
      if !is_armv7 {
        if !self.skip_deps {
          self.install_nv_codec_headers()?;
        }
        self.info("Enabling NVENC/NVDEC hardware acceleration...");
        args.push("--enable-ffnvcodec".to_string());
        args.push("--enable-nvenc".to_string());
        args.push("--enable-nvdec".to_string());
      } else {
        self.info("Skipping NVENC/NVDEC (not supported on armv7)");
      }

      // V4L2 M2M - ARM/embedded hardware acceleration
      if is_arm {
        self.info("Enabling V4L2 M2M for ARM target...");
        args.push("--enable-v4l2-m2m".to_string());
      }
    }

    // Cross-compilation: zig handles toolchain via CC/CXX, just need arch/os for FFmpeg config
    if let Some(cross) = self.cross_config() {
      args.push(format!("--arch={}", cross.ffmpeg_arch()));
      args.push(format!("--target-os={}", cross.ffmpeg_os()));
      args.push("--enable-cross-compile".to_string());
    }

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    self.run_configure(&source, &args_refs)?;
    self.run_make(&source)?;
    self.run_make_install(&source)?;

    self.info("FFmpeg built successfully");
    Ok(())
  }

  /// Check if VAAPI development headers are available
  fn check_vaapi_available(&self) -> bool {
    // Check via pkg-config
    Command::new("pkg-config")
      .args(["--exists", "libva"])
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()
      .map(|s| s.success())
      .unwrap_or(false)
  }

  /// Check prerequisites
  fn check_prerequisites(&self) -> io::Result<()> {
    let tools = ["git", "make", "cmake", "nasm"];

    for tool in &tools {
      if Command::new("which")
        .arg(tool)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success()
      {
        self.log(&format!("Found: {}", tool));
      } else {
        // nasm is optional, warn but don't fail
        if *tool == "nasm" {
          self.info(&format!(
            "Warning: {} not found, some optimizations may be disabled",
            tool
          ));
        } else {
          return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Required tool '{}' not found", tool),
          ));
        }
      }
    }

    Ok(())
  }

  /// Build all dependencies and FFmpeg
  fn build_all(&mut self) -> io::Result<()> {
    // Create directories
    fs::create_dir_all(&self.prefix)?;
    fs::create_dir_all(&self.source_dir)?;

    // Canonicalize paths to resolve ./ and symlinks
    // This ensures .pc files have clean absolute paths that pkg-config can resolve
    self.prefix = self.prefix.canonicalize()?;
    self.source_dir = self.source_dir.canonicalize()?;

    // Check prerequisites
    self.check_prerequisites()?;

    // Create zig wrapper scripts if using zig (filters incompatible flags like -march=armv8)
    if self.use_zig {
      self.ensure_zig_wrappers()?;
    }

    if !self.skip_deps {
      // Build dependencies in order
      self.build_x264()?;
      self.build_x265()?;
      self.build_vpx()?;
      self.build_aom()?;
      self.build_opus()?;
      self.build_lame()?;
      self.build_webp()?;
    }

    // Build FFmpeg
    self.build_ffmpeg()?;

    Ok(())
  }
}

/// Print usage information
fn print_usage() {
  eprintln!(
    r#"build-ffmpeg - Build FFmpeg and dependencies from source

USAGE:
    build-ffmpeg [OPTIONS]

OPTIONS:
    -o, --output <DIR>        Output installation directory [default: ./ffmpeg-build]
    -t, --target <TARGET>     Cross-compilation target [default: host]
    -s, --source-dir <DIR>    Directory for source code [default: ./ffmpeg-src]
    -j, --jobs <N>            Parallel build jobs [default: num_cpus]
    -v, --verbose             Enable verbose output
    --skip-deps               Skip building dependencies
    --use-system-cc           Use system CC/CXX from environment instead of zig
    -h, --help                Show this help message

SUPPORTED TARGETS:
    x86_64-unknown-linux-gnu
    x86_64-unknown-linux-musl
    aarch64-unknown-linux-gnu
    aarch64-unknown-linux-musl
    armv7-unknown-linux-gnueabihf
    x86_64-unknown-freebsd

ENVIRONMENT VARIABLES (when --use-system-cc is set):
    CC, CXX, AR, RANLIB, AS   Override default compilers/tools

EXAMPLE:
    cargo run --release --bin build-ffmpeg -- -o ./ffmpeg -v

    # Cross-compile for armv7 with GCC toolchain:
    export CC=arm-linux-gnueabihf-gcc CXX=arm-linux-gnueabihf-g++
    cargo run --release --bin build-ffmpeg -- -t armv7-unknown-linux-gnueabihf --use-system-cc
"#
  );
}

/// Parse command line arguments
fn parse_args() -> Result<BuildContext, String> {
  let args: Vec<String> = env::args().collect();

  let mut output = PathBuf::from("./ffmpeg-build");
  let mut source_dir = PathBuf::from("./ffmpeg-src");
  let mut target: Option<String> = None;
  let mut jobs = num_cpus::get();
  let mut verbose = false;
  let mut skip_deps = false;
  let mut use_system_cc = false;

  let mut i = 1;
  while i < args.len() {
    match args[i].as_str() {
      "-o" | "--output" => {
        i += 1;
        if i >= args.len() {
          return Err("Missing argument for --output".to_string());
        }
        output = PathBuf::from(&args[i]);
      }
      "-s" | "--source-dir" => {
        i += 1;
        if i >= args.len() {
          return Err("Missing argument for --source-dir".to_string());
        }
        source_dir = PathBuf::from(&args[i]);
      }
      "-t" | "--target" => {
        i += 1;
        if i >= args.len() {
          return Err("Missing argument for --target".to_string());
        }
        target = Some(args[i].clone());
      }
      "-j" | "--jobs" => {
        i += 1;
        if i >= args.len() {
          return Err("Missing argument for --jobs".to_string());
        }
        jobs = args[i]
          .parse()
          .map_err(|_| "Invalid number for --jobs".to_string())?;
      }
      "-v" | "--verbose" => {
        verbose = true;
      }
      "--skip-deps" => {
        skip_deps = true;
      }
      "--use-system-cc" => {
        use_system_cc = true;
      }
      "-h" | "--help" => {
        print_usage();
        std::process::exit(0);
      }
      arg => {
        return Err(format!("Unknown argument: {}", arg));
      }
    }
    i += 1;
  }

  // Make paths absolute
  let cwd = env::current_dir().map_err(|e| e.to_string())?;
  let output = if output.is_relative() {
    cwd.join(output)
  } else {
    output
  };
  let source_dir = if source_dir.is_relative() {
    cwd.join(source_dir)
  } else {
    source_dir
  };

  Ok(BuildContext::new(
    output,
    source_dir,
    target,
    jobs,
    verbose,
    skip_deps,
    use_system_cc,
  ))
}

fn main() {
  let mut ctx = match parse_args() {
    Ok(ctx) => ctx,
    Err(e) => {
      eprintln!("Error: {}", e);
      print_usage();
      std::process::exit(1);
    }
  };

  println!("========================================");
  println!("FFmpeg Build Script");
  println!("========================================");
  println!("Output directory: {}", ctx.prefix.display());
  println!("Source directory: {}", ctx.source_dir.display());
  println!("Jobs: {}", ctx.jobs);
  if let Some(ref t) = ctx.target {
    println!("Target: {}", t);
  }
  if ctx.use_system_cc {
    println!("Compiler: system (from environment)");
    println!("  CC:  {}", ctx.get_cc());
    println!("  CXX: {}", ctx.get_cxx());
  } else if ctx.use_zig {
    println!("Compiler: zig");
  } else {
    println!("Compiler: system default");
  }
  println!("========================================");

  if let Err(e) = ctx.build_all() {
    eprintln!("Build failed: {}", e);
    std::process::exit(1);
  }

  println!();
  println!("========================================");
  println!("Build completed successfully!");
  println!("========================================");
  println!();
  println!("To use the built FFmpeg, set:");
  println!("  export FFMPEG_DIR={}", ctx.prefix.display());
  println!();
}
