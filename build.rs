//! Build script for webcodecs-node
//!
//! Handles:
//! 1. NAPI-RS setup
//! 2. Compiling the C accessor library via `cc`
//! 3. Static linking of FFmpeg libraries
//! 4. Downloading pre-built FFmpeg from GitHub Releases (Linux/Windows/macOS)

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
  let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS not set");
  let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH not set");
  let target_env = env::var("CARGO_CFG_TARGET_ENV").expect("CARGO_CFG_TARGET_ENV not set");

  // Get FFmpeg directory
  let ffmpeg_dir = get_ffmpeg_dir(&target_os, &target_arch, &target_env);

  // Compile C accessor library
  compile_accessors(&ffmpeg_dir, &target_os, &target_arch, &target_env);

  // Link FFmpeg libraries
  link_ffmpeg(&ffmpeg_dir, &target_os, None);

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
fn get_ffmpeg_dir(target_os: &str, target_arch: &str, target_env: &str) -> PathBuf {
  // 1. Check for custom FFMPEG_DIR environment variable (explicit override)
  if let Ok(dir) = env::var("FFMPEG_DIR") {
    return PathBuf::from(dir);
  }

  let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
  let target_triple = get_target_triple(target_os, target_arch, target_env);

  // 2. Try bundled FFmpeg in project directory (e.g., ffmpeg-aarch64-apple-darwin/)
  let bundled_target = manifest_dir.join(format!("ffmpeg-{}", target_triple));
  if bundled_target.join("lib/libavcodec.a").exists()
    || bundled_target.join("lib/avcodec.lib").exists()
  {
    println!(
      "cargo:warning=Using bundled FFmpeg at {}",
      bundled_target.display()
    );
    return bundled_target;
  }

  // Also check legacy platform naming (e.g., ffmpeg/darwin-arm64/)
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

  // 3. Try downloading from GitHub Releases (preferred over system packages)
  // This ensures consistent builds with libaom instead of system FFmpeg (which may use SVT-AV1)
  if let Some(downloaded) = download_ffmpeg_from_release(target_os, target_arch, target_env) {
    return downloaded;
  }

  // 4. Fall back to pkg-config on Unix systems (for local development without release)
  #[cfg(unix)]
  {
    if let Ok(output) = Command::new("pkg-config")
      .args(["--variable=prefix", "libavcodec"])
      .output()
      && output.status.success()
    {
      let prefix = String::from_utf8_lossy(&output.stdout);
      let path = PathBuf::from(prefix.trim());
      if path.exists() {
        println!(
          "cargo:warning=Using system FFmpeg from pkg-config: {}",
          path.display()
        );
        return path;
      }
    }
  }

  // 5. Try common installation paths (last resort for local development)
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
      println!(
        "cargo:warning=Using system FFmpeg from common path: {}",
        p.display()
      );
      return p;
    }
  }

  // 6. FAIL - no fallback to building from source
  let repo =
    env::var("FFMPEG_GITHUB_REPO").unwrap_or_else(|_| "Brooooooklyn/webcodecs-node".to_string());
  panic!(
    "FFmpeg not found for target {}-{}.\n\
     \n\
     Options:\n\
     1. Set FFMPEG_DIR environment variable to point to FFmpeg installation\n\
     2. Place pre-built FFmpeg in ./ffmpeg-{} directory\n\
     3. Run 'Build FFmpeg Static' workflow to create a GitHub release\n\
     4. Install FFmpeg via package manager (e.g., brew install ffmpeg)\n\
     \n\
     Download from: https://github.com/{}/releases",
    target_os, target_arch, target_triple, repo
  );
}

/// Get target triple string
fn get_target_triple(target_os: &str, target_arch: &str, target_env: &str) -> String {
  match target_os {
    "linux" => match (target_arch, target_env) {
      ("x86_64", "gnu") => "x86_64-unknown-linux-gnu",
      ("x86_64", "musl") => "x86_64-unknown-linux-musl",
      ("aarch64", "gnu") => "aarch64-unknown-linux-gnu",
      ("aarch64", "musl") => "aarch64-unknown-linux-musl",
      // For armv7-unknown-linux-gnueabihf, CARGO_CFG_TARGET_ENV is "gnu" (not "gnueabihf")
      // The "gnueabihf" part is the ABI, not the env
      ("arm", "gnu") => "armv7-unknown-linux-gnueabihf",
      _ => "unknown",
    },
    "windows" => match target_arch {
      "x86_64" => "x86_64-pc-windows-msvc",
      "aarch64" => "aarch64-pc-windows-msvc",
      _ => "unknown",
    },
    "macos" => match target_arch {
      "x86_64" => "x86_64-apple-darwin",
      "aarch64" => "aarch64-apple-darwin",
      _ => "unknown",
    },
    _ => "unknown",
  }
  .to_string()
}

/// Download FFmpeg from GitHub Releases (Linux, Windows, and macOS)
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
        // For armv7-unknown-linux-gnueabihf, CARGO_CFG_TARGET_ENV is "gnu" (not "gnueabihf")
        ("arm", "gnu") => "armv7-unknown-linux-gnueabihf",
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
    "macos" => {
      let t = match target_arch {
        "x86_64" => "x86_64-apple-darwin",
        "aarch64" => "aarch64-apple-darwin",
        _ => return None,
      };
      (t, false)
    }
    _ => return None,
  };

  let repo =
    env::var("FFMPEG_GITHUB_REPO").unwrap_or_else(|_| "Brooooooklyn/webcodecs-node".to_string());

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

  // Build curl command - add auth header if GITHUB_TOKEN is available
  let mut cmd = Command::new("curl");
  cmd.args(["-s", "-f", "-H", "Accept: application/vnd.github+json"]);

  // Add authorization header if token is available (for higher rate limits in CI)
  if let Ok(token) = env::var("GITHUB_TOKEN") {
    cmd.args(["-H", &format!("Authorization: Bearer {}", token)]);
  }

  let output = cmd.arg(&api_url).output().ok()?;

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
fn compile_accessors(ffmpeg_dir: &Path, target_os: &str, target_arch: &str, target_env: &str) {
  let include_dir = ffmpeg_dir.join("include");

  let mut build = cc::Build::new();
  build
    .file("src/ffi/accessors.c")
    .include(&include_dir)
    .warnings(true)
    .extra_warnings(true);

  match target_os {
    "macos" => {
      build.flag("-Wno-deprecated-declarations");
    }
    "linux" => {
      build
        .flag_if_supported("-static")
        .cpp_link_stdlib_static(true)
        .cpp_set_stdlib(if target_arch == "arm" || target_env == "musl" {
          "stdc++"
        } else {
          "c++"
        });
    }
    _ => {}
  }

  // Compile
  build.compile("ffmpeg_accessors");

  println!("cargo:rerun-if-changed=src/ffi/accessors.c");
}

/// Link FFmpeg libraries (static only)
fn link_ffmpeg(ffmpeg_dir: &Path, target_os: &str, extra_lib_dir: Option<&PathBuf>) {
  let lib_dir = ffmpeg_dir.join("lib");

  // Always use static linking - panic if static libs not found
  link_static_ffmpeg(&lib_dir, target_os, extra_lib_dir);

  // Platform-specific system libraries
  link_platform_libraries(target_os);
}

/// Link FFmpeg statically using full paths to .a files
fn link_static_ffmpeg(lib_dir: &Path, target_os: &str, extra_lib_dir: Option<&PathBuf>) {
  // Get codec library paths, with lib_dir as highest priority
  let mut codec_lib_paths = get_codec_library_paths(target_os);
  // Insert lib_dir at the front so downloaded/bundled FFmpeg libs are found first
  codec_lib_paths.insert(0, lib_dir.to_path_buf());
  // Add extra lib dir (e.g., downloaded macOS static libs) if provided
  if let Some(extra_dir) = extra_lib_dir {
    codec_lib_paths.insert(1, extra_dir.join("lib"));
  }

  // FFmpeg core libraries - link using full paths
  let ffmpeg_libs = ["avformat", "avcodec", "avutil", "swscale", "swresample"];

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

  // Get target architecture for platform-specific library selection
  let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
  let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

  // Codec libraries - order matters for linking
  // Required libraries first, then optional
  //
  // Note: On Windows x64 MSVC, we use rav1e instead of libaom for AV1 encoding
  // because libaom crashes with CVE-2025-8879 related issues on MSVC.
  // See: https://github.com/aspect-build/rules_js/issues/2376
  let is_windows_msvc_x64 =
    target_os == "windows" && target_arch == "x86_64" && target_env == "msvc";

  // Work around duplicate Rust runtime symbols when linking rav1e.lib on Windows MSVC.
  // rav1e is a Rust staticlib that includes rust_eh_personality, which conflicts with
  // our own Rust runtime. /FORCE:MULTIPLE tells MSVC linker to accept duplicate symbols.
  // See: https://github.com/rust-lang/rust/issues/129020
  if is_windows_msvc_x64 {
    println!("cargo:rustc-link-arg=/FORCE:MULTIPLE");
  }

  let codec_libs: Vec<(&str, bool)> = vec![
    // Core video codec libraries (required)
    ("x264", true), // H.264
    ("x265", true), // H.265/HEVC
    ("vpx", true),  // VP8/VP9
    // AV1: Use rav1e on Windows x64 MSVC (libaom crashes), aom elsewhere
    ("aom", !is_windows_msvc_x64), // AV1 (required except on Windows x64 MSVC)
    ("rav1e", is_windows_msvc_x64), // AV1 encoder (required on Windows x64 MSVC)
    // Optional codec libraries
    ("dav1d", is_windows_msvc_x64), // AV1 decoder (required on Windows x64 MSVC)
    ("SvtAv1Enc", false),           // SVT-AV1 encoder
    ("xvidcore", false),            // Xvid MPEG-4
    // Image format libraries
    ("webp", false),      // WebP support
    ("webpmux", false),   // WebP muxer
    ("webpdemux", false), // WebP demuxer
    ("sharpyuv", false),  // WebP YUV conversion
    // JPEG XL support - built on all platforms via build-ffmpeg tool
    // Link order matters: jxl depends on jxl_threads, hwy, brotli, lcms2
    ("jxl", true),          // libjxl core
    ("jxl_threads", true),  // libjxl threading
    ("jxl_cms", false),     // libjxl color management - optional
    ("hwy", true),          // Highway SIMD library
    ("brotlienc", true),    // Brotli encoder
    ("brotlidec", true),    // Brotli decoder
    ("brotlicommon", true), // Brotli common
    ("lcms2", true),        // Little CMS 2
    // Text/subtitle libraries
    ("aribb24", false), // ARIB STD-B24 decoder
    // Other optional libraries
    ("mp3lame", false),   // MP3 encoder
    ("mpghip", false),    // MP3 decoder (required by mp3lame on Windows/vcpkg)
    ("opus", false),      // Opus audio codec
    ("vorbis", false),    // Vorbis audio
    ("vorbisenc", false), // Vorbis encoder
    ("ogg", false),       // Ogg container (required by vorbis)
    ("theora", false),    // Theora video
    ("speex", false),     // Speex audio
    ("soxr", false),      // SoX resampler
    ("snappy", false),    // Snappy compression
    ("zimg", false),      // Z image processing library
    // Compression library (built by build-ffmpeg for Linux, vcpkg for Windows)
    ("zlib", false), // zlib compression (required for PNG decoder)
    ("z", false),    // zlib on some systems uses 'libz.a' naming
  ];

  let mut linked_x265 = false;
  let mut linked_jxl = false;

  // On Linux, we need --whole-archive for codec libraries because FFmpeg uses
  // function pointer tables to reference codecs. Without --whole-archive, the
  // linker won't pull in symbols like VP8GetInfo that aren't directly referenced.
  // This applies to both GNU (glibc) and musl environments.
  let use_whole_archive = target_os == "linux";

  // Collect all codec library paths first (for --whole-archive grouping)
  let mut codec_paths: Vec<PathBuf> = Vec::new();

  for (lib, required) in &codec_libs {
    if let Some(path) = find_static_lib_path(lib, &codec_lib_paths) {
      codec_paths.push(path);
      if *lib == "x265" {
        linked_x265 = true;
      }
      if *lib == "jxl" {
        linked_jxl = true;
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

  // Link libgcc FIRST for armv7 - must come BEFORE --whole-archive block.
  // libaom uses Highway SIMD library which generates f16 conversion code on ARM NEON.
  // On ARMv7, these conversions require __gnu_f2h_ieee from libgcc.
  // Static linker is order-sensitive: libgcc must appear BEFORE libaom.
  if target_arch == "arm" && target_os == "linux" {
    let cc = env::var("CC").unwrap_or_else(|_| "gcc".to_string());
    if let Ok(output) = Command::new(&cc).arg("-print-libgcc-file-name").output()
      && output.status.success()
    {
      let libgcc_path = String::from_utf8_lossy(&output.stdout);
      let libgcc_path = libgcc_path.trim();
      if !libgcc_path.is_empty() && Path::new(libgcc_path).exists() {
        println!("cargo:rustc-link-arg={}", libgcc_path);
      }
    }
  }

  // Link codec libraries with --whole-archive on Linux
  if use_whole_archive && !codec_paths.is_empty() {
    println!("cargo:rustc-link-arg=-Wl,--whole-archive");
  }

  for path in &codec_paths {
    println!("cargo:rustc-link-arg={}", path.display());
  }

  if use_whole_archive && !codec_paths.is_empty() {
    println!("cargo:rustc-link-arg=-Wl,--no-whole-archive");
  }

  // x265 requires C++ runtime
  // Note: FFmpeg static libs are built with zig (libc++) but final linking uses
  // NAPI-RS GCC cross-toolchain which only has libstdc++. This works because
  // the C++ symbols needed are ABI-compatible for static linking.
  if linked_x265 || linked_jxl {
    match target_os {
      "macos" => println!("cargo:rustc-link-lib=c++"),
      "linux" => {
        if target_arch == "arm" || target_env == "musl" {
          println!("cargo:rustc-link-lib=stdc++");
        } else {
          // Link libc++ and its ABI dependency statically using explicit full paths.
          // We can't use rustc-link-lib because the napi-rs cross-compilation toolchain
          // has its own sysroot and doesn't respect our -L paths.
          // Using rustc-link-arg with full paths bypasses library search entirely.
          //
          // Try multiarch path first (what cross toolchains expect), then LLVM path
          let multiarch_dir = if target_arch == "x86_64" {
            "/usr/lib/x86_64-linux-gnu"
          } else {
            "/usr/lib/aarch64-linux-gnu"
          };
          let llvm_dir = "/usr/lib/llvm-18/lib";

          // Find libc++.a
          let libcpp_multiarch = format!("{}/libc++.a", multiarch_dir);
          let libcpp_llvm = format!("{}/libc++.a", llvm_dir);
          let libcpp = if Path::new(&libcpp_multiarch).exists() {
            &libcpp_multiarch
          } else if Path::new(&libcpp_llvm).exists() {
            &libcpp_llvm
          } else {
            panic!(
              "libc++.a not found at {} or {}. Install libc++-dev package.",
              libcpp_multiarch, libcpp_llvm
            );
          };

          // Find libc++abi.a
          let libcppabi_multiarch = format!("{}/libc++abi.a", multiarch_dir);
          let libcppabi_llvm = format!("{}/libc++abi.a", llvm_dir);
          let libcppabi = if Path::new(&libcppabi_multiarch).exists() {
            &libcppabi_multiarch
          } else if Path::new(&libcppabi_llvm).exists() {
            &libcppabi_llvm
          } else {
            panic!(
              "libc++abi.a not found at {} or {}. Install libc++abi-dev package.",
              libcppabi_multiarch, libcppabi_llvm
            );
          };

          println!("cargo:warning=Using libc++ from: {}", libcpp);
          println!("cargo:rustc-link-arg={}", libcpp);
          println!("cargo:rustc-link-arg={}", libcppabi);

          // On aarch64, LLVM uses outline atomics by default for compatibility with
          // older ARM64 CPUs without LSE (Large System Extensions). This generates
          // calls to helper functions like __aarch64_ldadd4_acq_rel.
          //
          // IMPORTANT: GCC's libatomic does NOT provide these symbols!
          // - GCC libatomic uses: __atomic_fetch_add_4 (C11 atomics API)
          // - LLVM generates: __aarch64_ldadd4_acq_rel (LLVM outline atomics)
          // These are incompatible ABIs. We MUST use LLVM's libclang_rt.builtins.
          if target_arch == "aarch64" {
            let clang_rt_path =
              "/usr/lib/llvm-18/lib/clang/18/lib/linux/libclang_rt.builtins-aarch64.a";
            if Path::new(clang_rt_path).exists() {
              println!(
                "cargo:warning=Using compiler-rt builtins from: {}",
                clang_rt_path
              );
              println!("cargo:rustc-link-arg={}", clang_rt_path);
            } else {
              panic!(
                "libclang_rt.builtins-aarch64.a not found at {}. \
                 Install libclang-rt-18-dev package.",
                clang_rt_path
              );
            }
          }
        }
      }
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

  paths.push(PathBuf::from("ffmpeg-build/lib"));

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
  let possible_names = vec![
    format!("lib{}.a", name),          // Unix: libfoo.a
    format!("{}.lib", name),           // Windows MSVC: foo.lib
    format!("{}.a", name),             // Some libs: foo.a
    format!("lib{}.lib", name),        // Windows: libfoo.lib
    format!("{}-static.lib", name),    // vcpkg static: foo-static.lib
    format!("lib{}-static.lib", name), // vcpkg static: libfoo-static.lib
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

      // Force-link the compiler runtime function for __builtin_available.
      // When LTO is enabled, this symbol can be incorrectly marked as unused
      // and not linked, causing crashes in FFmpeg's VideoToolbox code that
      // uses __builtin_available(macOS 12.0, ...) for API version checks.
      // We link libclang_rt.osx.a which contains ___isPlatformVersionAtLeast.
      //
      // Find the clang runtime library path dynamically using xcode-select
      let mut clang_rt_found = false;

      // Get the Xcode developer directory dynamically
      let developer_dir = Command::new("xcode-select")
        .args(["-p"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "/Applications/Xcode.app/Contents/Developer".to_string());

      // Get the clang version from the toolchain
      if let Ok(clang_output) = Command::new("xcrun").args(["clang", "--version"]).output()
        && clang_output.status.success()
      {
        let version_str = String::from_utf8_lossy(&clang_output.stdout);
        // Extract version number (e.g., "clang version 17.0.0" -> "17")
        if let Some(ver_line) = version_str.lines().next()
          && let Some(ver_start) = ver_line.find("version ")
        {
          let ver_part = &ver_line[ver_start + 8..];
          if let Some(major_end) = ver_part.find('.') {
            let major_version = &ver_part[..major_end];
            let clang_rt_path = format!(
              "{}/Toolchains/XcodeDefault.xctoolchain/usr/lib/clang/{}/lib/darwin/libclang_rt.osx.a",
              developer_dir, major_version
            );
            if Path::new(&clang_rt_path).exists() {
              println!("cargo:rustc-link-arg={}", clang_rt_path);
              clang_rt_found = true;
            }
          }
        }
      }

      // Fallback: try to find any version in the clang lib directory
      if !clang_rt_found {
        let clang_lib_base = format!(
          "{}/Toolchains/XcodeDefault.xctoolchain/usr/lib/clang",
          developer_dir
        );
        if let Ok(entries) = fs::read_dir(&clang_lib_base) {
          for entry in entries.flatten() {
            let path = entry.path().join("lib/darwin/libclang_rt.osx.a");
            if path.exists() {
              println!("cargo:rustc-link-arg={}", path.display());
              clang_rt_found = true;
              break;
            }
          }
        }
      }

      if !clang_rt_found {
        println!(
          "cargo:warning=Could not find libclang_rt.osx.a - VideoToolbox __builtin_available checks may fail at runtime"
        );
      }
    }

    "linux" => {
      // Basic system libraries
      // Note: zlib is linked via codec_libs (built by build-ffmpeg)
      println!("cargo:rustc-link-lib=m");
      println!("cargo:rustc-link-lib=pthread");
      println!("cargo:rustc-link-lib=dl");
    }

    "windows" => {
      // Windows system libraries required by FFmpeg and rav1e
      // Based on rust-ffmpeg-sys requirements + rav1e dependencies
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
        "mfplat",   // Media Foundation Platform
        "mfuuid",   // Media Foundation GUIDs
        "userenv",  // User environment (required by rav1e)
        "ntdll",    // NT runtime (required by rav1e)
      ];

      for lib in &libs {
        println!("cargo:rustc-link-lib={}", lib);
      }
    }

    _ => {
      println!("cargo:warning=Unknown target OS: {}", target_os);
    }
  }
}
