use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=ERIKA_NATIVE_PROFILE");
    println!("cargo:rerun-if-env-changed=ERIKA_NATIVE_TARGET");
    println!("cargo:rerun-if-env-changed=ERIKA_FFMPEG_DIR");

    let dist_dir = ffmpeg_dist_dir();
    let include_dir = dist_dir.join("include");
    let lib_dir = dist_dir.join("lib");

    if !include_dir.join("libavformat/avformat.h").exists() {
        panic!(
            "FFmpeg headers were not found at {}. Run `cargo run -p xtask -- deps build --profile {}` first, or set ERIKA_FFMPEG_DIR.",
            include_dir.display(),
            native_profile()
        );
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    let link_static = env::var("ERIKA_FFMPEG_STATIC").as_deref() == Ok("1");
    let link_prefix = if link_static { "static=" } else { "" };

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib={}avformat", link_prefix);
        println!("cargo:rustc-link-lib={}avcodec", link_prefix);
        println!("cargo:rustc-link-lib={}swresample", link_prefix);
        println!("cargo:rustc-link-lib={}swscale", link_prefix);
        println!("cargo:rustc-link-lib={}avutil", link_prefix);
        if lib_dir.join("avdevice.lib").exists() {
            println!("cargo:rustc-link-lib={}avdevice", link_prefix);
        }
        if lib_dir.join("avfilter.lib").exists() {
            println!("cargo:rustc-link-lib={}avfilter", link_prefix);
        }
    } else {
        println!("cargo:rustc-link-lib={}avdevice", link_prefix);
        println!("cargo:rustc-link-lib={}avfilter", link_prefix);
        println!("cargo:rustc-link-lib={}avformat", link_prefix);
        println!("cargo:rustc-link-lib={}avcodec", link_prefix);
        println!("cargo:rustc-link-lib={}swresample", link_prefix);
        println!("cargo:rustc-link-lib={}swscale", link_prefix);
        println!("cargo:rustc-link-lib={}avutil", link_prefix);
    }

    if matches!(
        env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("macos" | "ios")
    ) {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=CoreVideo");
        println!("cargo:rustc-link-lib=framework=VideoToolbox");
        println!("cargo:rustc-link-lib=iconv");
        println!("cargo:rustc-link-lib=bz2");
        println!("cargo:rustc-link-lib=z");
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=bcrypt");
        println!("cargo:rustc-link-lib=secur32");
        println!("cargo:rustc-link-lib=Mfplat");
        println!("cargo:rustc-link-lib=wmcodecdspuuid");
        println!("cargo:rustc-link-lib=strmiids");
        println!("cargo:rustc-link-lib=ole32");
        println!("cargo:rustc-link-lib=user32");
        println!("cargo:rustc-link-lib=ws2_32");
        println!("cargo:rustc-link-lib=secur32");
        println!("cargo:rustc-link-lib=shlwapi");
        println!("cargo:rustc-link-lib=shell32");
        println!("cargo:rustc-link-lib=d3d11");
        println!("cargo:rustc-link-lib=dxgi");
        println!("cargo:rustc-link-lib=strmiids");
    }

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", include_dir.display()))
        .allowlist_function("av_.*")
        .allowlist_function("avio_.*")
        .allowlist_function("avcodec_.*")
        .allowlist_function("avsubtitle_.*")
        .allowlist_function("avformat_.*")
        .allowlist_function("swr_.*")
        .allowlist_type("AV.*")
        .allowlist_type("Swr.*")
        .allowlist_var("AV.*")
        .allowlist_var("FF_.*")
        .allowlist_var("AVERROR.*")
        .blocklist_item("FP_.*")
        .generate_comments(false)
        .derive_debug(true)
        .derive_default(true)
        .generate()
        .expect("generate FFmpeg bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set"));
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("write FFmpeg bindings");
}

fn ffmpeg_dist_dir() -> PathBuf {
    if let Ok(path) = env::var("ERIKA_FFMPEG_DIR") {
        return PathBuf::from(path);
    }
    if let Ok(target) = env::var("ERIKA_NATIVE_TARGET") {
        return workspace_root()
            .join("third_party/dist")
            .join(target)
            .join(native_profile())
            .join("ffmpeg");
    }
    let mut dist = workspace_root().join("third_party/dist");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        dist = dist.join("ios");
    }
    dist.join(native_profile()).join("ffmpeg")
}

fn native_profile() -> String {
    env::var("ERIKA_NATIVE_PROFILE").unwrap_or_else(|_| "lgpl".to_string())
}

fn workspace_root() -> PathBuf {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    manifest_dir
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .expect("crate lives under workspace/crates/name")
}
