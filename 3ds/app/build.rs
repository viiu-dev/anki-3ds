// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::env;
use std::path::PathBuf;

fn main() {
    let devkitpro = env::var("DEVKITPRO").unwrap_or_else(|_| "/opt/devkitpro".to_string());
    let devkitarm = env::var("DEVKITARM").unwrap_or_else(|_| format!("{devkitpro}/devkitARM"));
    let portlibs = PathBuf::from(&devkitpro).join("portlibs/3ds");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../out/3ds/https-sources/cacert.pem");
    println!("cargo:rerun-if-changed=src/ui_renderer.c");
    println!("cargo:rerun-if-changed=src/clock.c");
    println!("cargo:rerun-if-changed=src/ui_renderer.h");
    println!("cargo:rerun-if-changed=src/stb_truetype.h");
    println!("cargo:rerun-if-changed=src/stb_image.h");
    println!("cargo:rerun-if-changed=src/dr_mp3.h");
    println!("cargo:rerun-if-changed=resources/NotoSansJP.ttf");
    println!("cargo:rerun-if-changed=../third_party/nanosvg/nanosvg.h");
    println!("cargo:rerun-if-changed=../third_party/nanosvg/nanosvgrast.h");
    println!("cargo:rerun-if-changed=../third_party/nanosvg/nanosvg_impl.c");

    let webp = PathBuf::from("../third_party/libwebp");
    let webp_sources: Vec<String> = [
        "src/dec/alpha_dec.c",
        "src/dec/buffer_dec.c",
        "src/dec/frame_dec.c",
        "src/dec/idec_dec.c",
        "src/dec/io_dec.c",
        "src/dec/quant_dec.c",
        "src/dec/tree_dec.c",
        "src/dec/vp8_dec.c",
        "src/dec/vp8l_dec.c",
        "src/dec/webp_dec.c",
        "src/dsp/alpha_processing.c",
        "src/dsp/cpu.c",
        "src/dsp/dec.c",
        "src/dsp/dec_clip_tables.c",
        "src/dsp/filters.c",
        "src/dsp/lossless.c",
        "src/dsp/rescaler.c",
        "src/dsp/upsampling.c",
        "src/dsp/yuv.c",
        "src/utils/bit_reader_utils.c",
        "src/utils/color_cache_utils.c",
        "src/utils/filters_utils.c",
        "src/utils/huffman_utils.c",
        "src/utils/palette.c",
        "src/utils/quant_levels_dec_utils.c",
        "src/utils/random_utils.c",
        "src/utils/rescaler_utils.c",
        "src/utils/thread_utils.c",
        "src/utils/utils.c",
        "src/demux/demux.c",
    ]
    .iter()
    .map(|path| webp.join(path).display().to_string())
    .collect();
    for source in &webp_sources {
        println!("cargo:rerun-if-changed={source}");
    }

    let mut cc = cc::Build::new();
    cc.compiler(format!("{devkitarm}/bin/arm-none-eabi-gcc"))
        .archiver(format!("{devkitarm}/bin/arm-none-eabi-ar"))
        .include(format!("{devkitpro}/libctru/include"))
        .include(&portlibs.join("include"))
        .include(portlibs.join("include/opus"))
        .include(&webp)
        .include(webp.join("src"))
        .include("../third_party/nanosvg")
        .file("src/ui_renderer.c")
        .file("src/clock.c")
        .files(&webp_sources)
        .file("../third_party/nanosvg/nanosvg_impl.c")
        .define("ARM11", None)
        .define("__3DS__", None)
        .define("NDEBUG", None)
        .flag("-march=armv6k")
        .flag("-mtune=mpcore")
        .flag("-mfloat-abi=hard")
        .flag("-mfpu=vfp")
        .flag("-mtp=soft")
        .flag("-O2")
        .flag("-ffunction-sections")
        .compile("ui_renderer");

    println!("cargo:rustc-link-arg=-Wl,--wrap=clock_gettime,--wrap=time");
    println!(
        "cargo:rustc-link-search=native={}",
        portlibs.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=citro2d");
    println!("cargo:rustc-link-lib=static=citro3d");
    println!("cargo:rustc-link-lib=static=opusfile");
    println!("cargo:rustc-link-lib=static=opus");
    println!("cargo:rustc-link-lib=static=vorbisidec");
    println!("cargo:rustc-link-lib=static=ogg");
    println!("cargo:rustc-link-lib=m");
}
