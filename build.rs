//! Build script for webcodec-node
//!
//! Handles:
//! 1. NAPI-RS setup
//! 2. Compiling the C accessor library via `cc`
//! 3. Static linking of FFmpeg libraries

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    // NAPI-RS build setup
    napi_build::setup();

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
}

/// Get FFmpeg installation directory
fn get_ffmpeg_dir(target_os: &str, target_arch: &str) -> PathBuf {
    // Check for custom FFMPEG_DIR environment variable
    if let Ok(dir) = env::var("FFMPEG_DIR") {
        return PathBuf::from(dir);
    }

    // Check for pkg-config on Unix systems
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("pkg-config")
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

    // Try common installation paths
    let common_paths = match target_os {
        "macos" => vec![
            "/opt/homebrew", // Apple Silicon Homebrew
            "/usr/local",    // Intel Homebrew / manual install
            "/opt/local",    // MacPorts
        ],
        "linux" => vec![
            "/usr",
            "/usr/local",
            "/opt/ffmpeg",
        ],
        "windows" => vec![
            "C:\\ffmpeg",
            "C:\\Program Files\\ffmpeg",
        ],
        _ => vec![],
    };

    for path in common_paths {
        let p = PathBuf::from(path);
        if p.join("include/libavcodec/avcodec.h").exists() {
            return p;
        }
    }

    // Try bundled FFmpeg in project directory
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let platform = match (target_os, target_arch) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("windows", "x86_64") => "win32-x64",
        _ => "unknown",
    };

    let bundled = manifest_dir.join("ffmpeg").join(platform);
    if bundled.exists() {
        return bundled;
    }

    // Fallback: assume FFmpeg is in system paths
    println!("cargo:warning=FFmpeg not found. Set FFMPEG_DIR environment variable or install FFmpeg.");
    PathBuf::from("/usr/local")
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

/// Link FFmpeg libraries
fn link_ffmpeg(ffmpeg_dir: &Path, target_os: &str) {
    let lib_dir = ffmpeg_dir.join("lib");

    // Determine if we should use static or dynamic linking
    // Check for explicit preference first
    let use_static = match env::var("FFMPEG_STATIC").as_deref() {
        Ok("0") | Ok("false") | Ok("no") => false,
        Ok("1") | Ok("true") | Ok("yes") => true,
        _ => {
            // Auto-detect: Prefer static linking when available
            lib_dir.join("libavcodec.a").exists()
        }
    };

    println!("cargo:warning=Using {} linking for FFmpeg", if use_static { "static" } else { "dynamic" });

    if use_static {
        // For true static linking, we need to link the .a files directly
        // This prevents the linker from finding dylibs first
        link_static_ffmpeg(&lib_dir, target_os);
    } else {
        // Add library search path for dynamic linking
        if lib_dir.exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
        }

        // FFmpeg core libraries
        let ffmpeg_libs = ["avcodec", "avutil", "swscale", "swresample"];
        for lib in &ffmpeg_libs {
            println!("cargo:rustc-link-lib=dylib={}", lib);
        }
    }

    // Platform-specific system libraries
    link_platform_libraries(target_os);
}

/// Link FFmpeg statically using full paths to .a files
fn link_static_ffmpeg(lib_dir: &Path, target_os: &str) {
    // Get codec library paths
    let codec_lib_paths = get_codec_library_paths(target_os);

    // FFmpeg core libraries - link using full paths
    let ffmpeg_libs = ["avcodec", "avutil", "swscale", "swresample"];

    for lib in &ffmpeg_libs {
        let static_lib = lib_dir.join(format!("lib{}.a", lib));
        if static_lib.exists() {
            // Use link-arg to specify full path - this forces static linking
            println!("cargo:rustc-link-arg={}", static_lib.display());
        } else {
            // Fallback to normal linking
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
            println!("cargo:rustc-link-lib=static={}", lib);
        }
    }

    // Codec libraries - order matters for linking
    // Required libraries first, then optional
    let codec_libs = [
        // Core video codec libraries (required)
        ("x264", true),      // H.264
        ("x265", true),      // H.265/HEVC
        ("vpx", true),       // VP8/VP9
        ("aom", true),       // AV1
        // Optional codec libraries
        ("dav1d", false),    // AV1 decoder
        ("rav1e", false),    // AV1 encoder (Rust-based)
        ("SvtAv1Enc", false), // SVT-AV1 encoder
        // Image format libraries
        ("webp", false),     // WebP support
        ("webpmux", false),  // WebP muxer
        ("webpdemux", false), // WebP demuxer
        ("sharpyuv", false), // WebP YUV conversion
        // Text/subtitle libraries
        ("aribb24", false),  // ARIB STD-B24 decoder
        // Other optional libraries
        ("mp3lame", false),  // MP3 encoder
        ("opus", false),     // Opus audio codec
        ("vorbis", false),   // Vorbis audio
        ("vorbisenc", false), // Vorbis encoder
        ("theora", false),   // Theora video
        ("speex", false),    // Speex audio
        ("soxr", false),     // SoX resampler
        ("snappy", false),   // Snappy compression
    ];

    let mut linked_x265 = false;

    for (lib, required) in &codec_libs {
        if let Some(path) = find_static_lib_path(lib, &codec_lib_paths) {
            println!("cargo:rustc-link-arg={}", path.display());
            if *lib == "x265" {
                linked_x265 = true;
            }
        } else if *required {
            // Try dynamic as fallback for required libs
            for search_path in &codec_lib_paths {
                println!("cargo:rustc-link-search=native={}", search_path.display());
            }
            println!("cargo:rustc-link-lib={}", lib);
            if *lib == "x265" {
                linked_x265 = true;
            }
        }
    }

    // x265 requires C++ runtime
    if linked_x265 {
        match target_os {
            "macos" => println!("cargo:rustc-link-lib=c++"),
            "linux" => println!("cargo:rustc-link-lib=stdc++"),
            _ => {}
        }
    }
}

/// Get codec library search paths
fn get_codec_library_paths(target_os: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Add paths from LIBRARY_PATH environment variable
    if let Ok(lib_path) = env::var("LIBRARY_PATH") {
        for path in lib_path.split(':') {
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
        _ => {}
    }

    // Add FFmpeg lib dir if set
    if let Ok(ffmpeg_dir) = env::var("FFMPEG_DIR") {
        paths.push(PathBuf::from(ffmpeg_dir).join("lib"));
    }

    paths
}

/// Find static library path if it exists
fn find_static_lib_path(name: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    let static_name = format!("lib{}.a", name);

    for path in paths {
        let full_path = path.join(&static_name);
        if full_path.exists() {
            return Some(full_path);
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
            // Windows system libraries
            let libs = [
                "bcrypt",
                "ole32",
                "oleaut32",
                "user32",
                "ws2_32",
                "secur32",
                "advapi32",
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
