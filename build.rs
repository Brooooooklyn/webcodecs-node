//! Build script for webcodec-node
//!
//! Handles:
//! 1. NAPI-RS setup
//! 2. Compiling the C accessor library via `cc`
//! 3. Static linking of FFmpeg libraries
//! 4. Downloading pre-built FFmpeg from GitHub Releases (Linux only)

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
  // NAPI-RS build setup
  napi_build::setup();

  // Skip FFmpeg operations when building standalone binaries (e.g., build-ffmpeg)
  // This is needed because the build-ffmpeg binary doesn't need FFmpeg to compile,
  // and FFmpeg might not exist yet (we're building the tool that creates it!)
  if env::var("SKIP_FFMPEG_BUILD").unwrap_or_default() == "1" {
    println!("cargo:warning=Skipping FFmpeg build steps (SKIP_FFMPEG_BUILD=1)");
    return;
  }

  // Get target information
  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
  let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

  // Get FFmpeg directory
  let ffmpeg_dir = get_ffmpeg_dir(&target_os, &target_arch);

  // Compile C accessor library
  compile_accessors(&ffmpeg_dir);

  // Link FFmpeg libraries
  link_ffmpeg(&ffmpeg_dir, &target_os);

  // Re-run if these files change
  println!("cargo:rerun-if-changed=src/ffi/accessors.c");
  println!("cargo:rerun-if-changed=build.rs");
  println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
  println!("cargo:rerun-if-env-changed=FFMPEG_GITHUB_REPO");
  println!("cargo:rerun-if-env-changed=FFMPEG_RELEASE_TAG");
  println!("cargo:rerun-if-env-changed=FFMPEG_SKIP_DOWNLOAD");
  println!("cargo:rerun-if-env-changed=SKIP_FFMPEG_BUILD");
}

/// Get FFmpeg installation directory
fn get_ffmpeg_dir(target_os: &str, target_arch: &str) -> PathBuf {
  // 1. Check for custom FFMPEG_DIR environment variable
  if let Ok(dir) = env::var("FFMPEG_DIR") {
    return PathBuf::from(dir);
  }

  // 2. Check for pkg-config on Unix systems
  #[cfg(unix)]
  {
    if let Ok(output) = Command::new("pkg-config")
      .args(["--variable=prefix", "libavcodec"])
      .output()
    {
      if output.status.success() {
        let prefix = String::from_utf8_lossy(&output.stdout);
        let path = PathBuf::from(prefix.trim());
        if path.exists() {
          return path;
        }
      }
    }
  }

  // 3. Try common installation paths
  let common_paths = match target_os {
    "macos" => vec![
      "/opt/homebrew", // Apple Silicon Homebrew
      "/usr/local",    // Intel Homebrew / manual install
      "/opt/local",    // MacPorts
    ],
    "linux" => vec!["/usr", "/usr/local", "/opt/ffmpeg"],
    "windows" => vec!["C:\\ffmpeg", "C:\\Program Files\\ffmpeg"],
    _ => vec![],
  };

  for path in common_paths {
    let p = PathBuf::from(path);
    if p.join("include/libavcodec/avcodec.h").exists() {
      return p;
    }
  }

  // 4. Try bundled FFmpeg in project directory
  let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
  let platform = match (target_os, target_arch) {
    ("macos", "aarch64") => "darwin-arm64",
    ("macos", "x86_64") => "darwin-x64",
    ("linux", "x86_64") => "linux-x64",
    ("linux", "aarch64") => "linux-arm64",
    ("windows", "x86_64") => "win32-x64",
    ("windows", "aarch64") => "win32-arm64",
    _ => "unknown",
  };

  let bundled = manifest_dir.join("ffmpeg").join(platform);
  if bundled.exists() {
    return bundled;
  }

  // 5. Try downloading from GitHub Releases (Linux and Windows)
  let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
  if let Some(downloaded) = download_ffmpeg_from_release(target_os, target_arch, &target_env) {
    return downloaded;
  }

  // 6. FAIL - no fallback to building from source
  let repo =
    env::var("FFMPEG_GITHUB_REPO").unwrap_or_else(|_| "Brooooooklyn/webcodec-node".to_string());
  panic!(
    "FFmpeg not found for target {}-{}.\n\
     \n\
     Options:\n\
     1. Set FFMPEG_DIR environment variable to point to FFmpeg installation\n\
     2. Install FFmpeg via package manager (e.g., brew install ffmpeg)\n\
     3. Create a GitHub Release with pre-built FFmpeg:\n\
        - Run the 'Build FFmpeg Static' workflow in GitHub Actions\n\
        - Or download from: https://github.com/{}/releases\n\
     \n\
     For CI: Ensure the ffmpeg-build.yml workflow has been run to create a release.",
    target_os, target_arch, repo
  );
}

/// Download FFmpeg from GitHub Releases (Linux and Windows)
fn download_ffmpeg_from_release(
  target_os: &str,
  target_arch: &str,
  target_env: &str,
) -> Option<PathBuf> {
  // Skip if FFMPEG_SKIP_DOWNLOAD is set
  if env::var("FFMPEG_SKIP_DOWNLOAD").unwrap_or_default() == "1" {
    return None;
  }

  // Map to Rust target triple and determine archive format
  let (target, is_windows) = match target_os {
    "linux" => {
      let t = match (target_arch, target_env) {
        ("x86_64", "gnu") => "x86_64-unknown-linux-gnu",
        ("x86_64", "musl") => "x86_64-unknown-linux-musl",
        ("aarch64", "gnu") => "aarch64-unknown-linux-gnu",
        ("aarch64", "musl") => "aarch64-unknown-linux-musl",
        ("arm", "gnueabihf") => "armv7-unknown-linux-gnueabihf",
        _ => return None,
      };
      (t, false)
    }
    "windows" => {
      let t = match target_arch {
        "x86_64" => "x86_64-pc-windows-msvc",
        "aarch64" => "aarch64-pc-windows-msvc",
        _ => return None,
      };
      (t, true)
    }
    _ => return None,
  };

  let repo =
    env::var("FFMPEG_GITHUB_REPO").unwrap_or_else(|_| "Brooooooklyn/webcodec-node".to_string());

  // Determine release tag
  let release_tag = match env::var("FFMPEG_RELEASE_TAG") {
    Ok(tag) => tag,
    Err(_) => find_latest_ffmpeg_release(&repo)?,
  };

  let (archive_ext, archive_name) = if is_windows {
    ("zip", format!("ffmpeg-{}.zip", target))
  } else {
    ("tar.gz", format!("ffmpeg-{}.tar.gz", target))
  };

  let download_url = format!(
    "https://github.com/{}/releases/download/{}/{}",
    repo, release_tag, archive_name
  );

  // Download to OUT_DIR
  let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
  let ffmpeg_dir = out_dir.join("ffmpeg");
  let archive_path = out_dir.join(&archive_name);

  // Skip if already extracted - check for platform-specific library
  let lib_check = if is_windows {
    ffmpeg_dir.join("lib").join("avcodec.lib")
  } else {
    ffmpeg_dir.join("lib").join("libavcodec.a")
  };

  if lib_check.exists() {
    println!(
      "cargo:warning=Using cached FFmpeg at {}",
      ffmpeg_dir.display()
    );
    return Some(ffmpeg_dir);
  }

  println!("cargo:warning=Downloading FFmpeg from {}", download_url);

  // Download using curl (available on all CI runners)
  let status = Command::new("curl")
    .args(["-L", "-f", "-o"])
    .arg(&archive_path)
    .arg(&download_url)
    .status();

  match status {
    Ok(s) if s.success() => {}
    _ => {
      println!(
        "cargo:warning=Failed to download FFmpeg from {}",
        download_url
      );
      return None;
    }
  }

  // Create extraction directory
  if let Err(e) = fs::create_dir_all(&ffmpeg_dir) {
    println!("cargo:warning=Failed to create directory: {}", e);
    return None;
  }

  // Extract archive based on format
  let extract_status = if is_windows {
    // Use PowerShell to extract zip on Windows
    Command::new("powershell")
      .args([
        "-NoProfile",
        "-Command",
        &format!(
          "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
          archive_path.display(),
          ffmpeg_dir.display()
        ),
      ])
      .status()
  } else {
    // Use tar for Linux
    Command::new("tar")
      .arg("xzf")
      .arg(&archive_path)
      .arg("-C")
      .arg(&ffmpeg_dir)
      .status()
  };

  match extract_status {
    Ok(s) if s.success() => {
      // Clean up archive
      let _ = fs::remove_file(&archive_path);
      println!("cargo:warning=FFmpeg extracted to {}", ffmpeg_dir.display());
      Some(ffmpeg_dir)
    }
    _ => {
      println!(
        "cargo:warning=Failed to extract FFmpeg {} archive",
        archive_ext
      );
      None
    }
  }
}

/// Find the latest ffmpeg-* release tag from GitHub
fn find_latest_ffmpeg_release(repo: &str) -> Option<String> {
  // Use GitHub API to list releases
  let api_url = format!("https://api.github.com/repos/{}/releases", repo);

  let output = Command::new("curl")
    .args(["-s", "-f", "-H", "Accept: application/vnd.github+json"])
    .arg(&api_url)
    .output()
    .ok()?;

  if !output.status.success() {
    return None;
  }

  let body = String::from_utf8_lossy(&output.stdout);

  // Simple JSON parsing - find first "tag_name": "ffmpeg-*"
  // This is a minimal parser to avoid adding dependencies
  for line in body.lines() {
    if let Some(pos) = line.find("\"tag_name\"") {
      let rest = &line[pos..];
      if let Some(start) = rest.find("ffmpeg-") {
        let tag_str = &rest[start..];
        if let Some(end) = tag_str.find('"') {
          let tag = &tag_str[..end];
          return Some(tag.to_string());
        }
      }
    }
  }

  None
}

/// Compile the C accessor library
fn compile_accessors(ffmpeg_dir: &Path) {
  let include_dir = ffmpeg_dir.join("include");

  let mut build = cc::Build::new();
  build
    .file("src/ffi/accessors.c")
    .include(&include_dir)
    .warnings(true)
    .extra_warnings(true);

  // Platform-specific flags
  #[cfg(target_os = "macos")]
  {
    build.flag("-Wno-deprecated-declarations");
  }

  // Compile
  build.compile("ffmpeg_accessors");

  println!("cargo:rerun-if-changed=src/ffi/accessors.c");
}

/// Link FFmpeg libraries (static only)
fn link_ffmpeg(ffmpeg_dir: &Path, target_os: &str) {
  let lib_dir = ffmpeg_dir.join("lib");

  // Always use static linking - panic if static libs not found
  link_static_ffmpeg(&lib_dir, target_os);

  // Platform-specific system libraries
  link_platform_libraries(target_os);
}

/// Link FFmpeg statically using full paths to .a files
fn link_static_ffmpeg(lib_dir: &Path, target_os: &str) {
  // Get codec library paths, with lib_dir as highest priority
  let mut codec_lib_paths = get_codec_library_paths(target_os);
  // Insert lib_dir at the front so downloaded/bundled FFmpeg libs are found first
  codec_lib_paths.insert(0, lib_dir.to_path_buf());

  // FFmpeg core libraries - link using full paths
  let ffmpeg_libs = ["avcodec", "avutil", "swscale", "swresample"];

  for lib in &ffmpeg_libs {
    // Try different naming conventions (Unix .a and Windows .lib)
    let possible_names = [
      format!("lib{}.a", lib), // Unix: libavcodec.a
      format!("{}.lib", lib),  // Windows MSVC: avcodec.lib
    ];

    let static_lib = possible_names
      .iter()
      .map(|name| lib_dir.join(name))
      .find(|path| path.exists());

    if let Some(path) = static_lib {
      // Use link-arg to specify full path - this forces static linking
      println!("cargo:rustc-link-arg={}", path.display());
    } else {
      panic!(
        "Static library for {} not found at {}. \
         Static linking is required. Please install FFmpeg with static libraries \
         or set FFMPEG_DIR to point to an FFmpeg installation with static libs. \
         Looked for: {:?}",
        lib,
        lib_dir.display(),
        possible_names
      );
    }
  }

  // Codec libraries - order matters for linking
  // Required libraries first, then optional
  let codec_libs = [
    // Core video codec libraries (required)
    ("x264", true), // H.264
    ("x265", true), // H.265/HEVC
    ("vpx", true),  // VP8/VP9
    ("aom", true),  // AV1
    // Optional codec libraries
    ("dav1d", false),     // AV1 decoder
    ("rav1e", false),     // AV1 encoder (Rust-based)
    ("SvtAv1Enc", false), // SVT-AV1 encoder
    ("xvidcore", false),  // Xvid MPEG-4
    // Image format libraries
    ("webp", false),      // WebP support
    ("webpmux", false),   // WebP muxer
    ("webpdemux", false), // WebP demuxer
    ("sharpyuv", false),  // WebP YUV conversion
    // Text/subtitle libraries
    ("aribb24", false), // ARIB STD-B24 decoder
    // Other optional libraries
    ("mp3lame", false),   // MP3 encoder
    ("opus", false),      // Opus audio codec
    ("vorbis", false),    // Vorbis audio
    ("vorbisenc", false), // Vorbis encoder
    ("ogg", false),       // Ogg container (required by vorbis)
    ("theora", false),    // Theora video
    ("speex", false),     // Speex audio
    ("soxr", false),      // SoX resampler
    ("snappy", false),    // Snappy compression
    ("zimg", false),      // Z image processing library
  ];

  let mut linked_x265 = false;

  for (lib, required) in &codec_libs {
    if let Some(path) = find_static_lib_path(lib, &codec_lib_paths) {
      println!("cargo:rustc-link-arg={}", path.display());
      if *lib == "x265" {
        linked_x265 = true;
      }
    } else if *required {
      panic!(
        "Required static library for '{}' not found. \
         Static linking is required. Searched paths: {:?}",
        lib, codec_lib_paths
      );
    }
    // Optional libraries are silently skipped if not found
  }

  // x265 requires C++ runtime
  if linked_x265 {
    match target_os {
      "macos" => println!("cargo:rustc-link-lib=c++"),
      "linux" => println!("cargo:rustc-link-lib=static=c++"),
      "windows" => {
        // MSVC uses msvcrt automatically, but we need to ensure C++ runtime is linked
        // For static linking with MSVC, the runtime is usually already included
      }
      _ => {}
    }
  }
}

/// Get codec library search paths
fn get_codec_library_paths(target_os: &str) -> Vec<PathBuf> {
  let mut paths = Vec::new();

  // Add paths from LIBRARY_PATH environment variable (Unix) or LIB (Windows)
  let (lib_path_var, separator) = if target_os == "windows" {
    ("LIB", ';')
  } else {
    ("LIBRARY_PATH", ':')
  };

  if let Ok(lib_path) = env::var(lib_path_var) {
    for path in lib_path.split(separator) {
      paths.push(PathBuf::from(path));
    }
  }

  // Add common paths based on OS
  match target_os {
    "macos" => {
      paths.push(PathBuf::from("/opt/homebrew/lib"));
      paths.push(PathBuf::from("/usr/local/lib"));
      paths.push(PathBuf::from("/opt/local/lib"));
    }
    "linux" => {
      paths.push(PathBuf::from("/usr/lib"));
      paths.push(PathBuf::from("/usr/local/lib"));
      paths.push(PathBuf::from("/usr/lib/x86_64-linux-gnu"));
      paths.push(PathBuf::from("/usr/lib/aarch64-linux-gnu"));
    }
    "windows" => {
      // Common Windows FFmpeg install locations
      paths.push(PathBuf::from("C:\\ffmpeg\\lib"));
      paths.push(PathBuf::from("C:\\Program Files\\ffmpeg\\lib"));
    }
    _ => {}
  }

  if let Ok(brew_prefix) = env::var("HOMEBREW_PREFIX") {
    paths.push(PathBuf::from(brew_prefix).join("lib"));
  }

  // Add FFmpeg lib dir if set
  if let Ok(ffmpeg_dir) = env::var("FFMPEG_DIR") {
    paths.push(PathBuf::from(ffmpeg_dir).join("lib"));
  }

  paths
}

/// Find static library path if it exists (handles both Unix .a and Windows .lib formats)
fn find_static_lib_path(name: &str, paths: &[PathBuf]) -> Option<PathBuf> {
  // Try different naming conventions
  let possible_names = [
    format!("lib{}.a", name),   // Unix: libfoo.a
    format!("{}.lib", name),    // Windows MSVC: foo.lib
    format!("{}.a", name),      // Some libs: foo.a
    format!("lib{}.lib", name), // Rare: libfoo.lib
  ];

  for path in paths {
    for static_name in &possible_names {
      let full_path = path.join(static_name);
      if full_path.exists() {
        return Some(full_path);
      }
    }
  }
  None
}

/// Link platform-specific system libraries
fn link_platform_libraries(target_os: &str) {
  match target_os {
    "macos" => {
      // macOS frameworks for hardware acceleration
      let frameworks = [
        "VideoToolbox",
        "CoreMedia",
        "CoreVideo",
        "CoreFoundation",
        "Security",
        "AudioToolbox",
        "CoreServices",
      ];

      for framework in &frameworks {
        println!("cargo:rustc-link-lib=framework={}", framework);
      }

      // System libraries
      println!("cargo:rustc-link-lib=z");
      println!("cargo:rustc-link-lib=bz2");
      println!("cargo:rustc-link-lib=iconv");
      println!("cargo:rustc-link-lib=lzma");
    }

    "linux" => {
      // Basic system libraries
      println!("cargo:rustc-link-lib=z");
      println!("cargo:rustc-link-lib=m");
      println!("cargo:rustc-link-lib=pthread");
      println!("cargo:rustc-link-lib=dl");

      // VAAPI for hardware acceleration (if available)
      #[cfg(feature = "hwaccel")]
      {
        println!("cargo:rustc-link-lib=va");
        println!("cargo:rustc-link-lib=va-drm");
        println!("cargo:rustc-link-lib=va-x11");
      }
    }

    "windows" => {
      // Windows system libraries required by FFmpeg
      // Based on rust-ffmpeg-sys requirements
      let libs = [
        "bcrypt",   // Cryptography
        "ole32",    // COM/OLE
        "oleaut32", // OLE Automation
        "user32",   // Windows API
        "gdi32",    // Graphics Device Interface
        "vfw32",    // Video for Windows
        "strmiids", // DirectShow GUIDs
        "shlwapi",  // Shell Lightweight API
        "shell32",  // Shell API
        "ws2_32",   // Windows Sockets
        "secur32",  // Security API
        "advapi32", // Advanced Windows API
      ];

      for lib in &libs {
        println!("cargo:rustc-link-lib={}", lib);
      }

      // Media Foundation for hardware acceleration
      #[cfg(feature = "hwaccel")]
      {
        println!("cargo:rustc-link-lib=mfplat");
        println!("cargo:rustc-link-lib=mfuuid");
      }
    }

    _ => {
      println!("cargo:warning=Unknown target OS: {}", target_os);
    }
  }
}
