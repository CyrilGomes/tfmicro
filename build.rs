//! Build

#[macro_use]
extern crate error_chain;
error_chain! {
    foreign_links {
        Io(::std::io::Error);
        EnvVar(::std::env::VarError);
        StringFromUtf8(::std::string::FromUtf8Error);
    }
}

use glob::glob;

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
}

fn submodules() -> PathBuf {
    manifest_dir().join("submodules")
}

fn flatbuffers_include_dir() -> PathBuf {
    submodules().join("tensorflow/tensorflow/lite/micro/tools/make/downloads/flatbuffers/include")
}

pub fn is_cross_compiling() -> Result<bool> {
    Ok(env::var("TARGET")? != env::var("HOST")?)
}

/// Move tensorflow source to $OUT_DIR
fn prepare_tensorflow_source() -> PathBuf {
    println!("Moving tflite source");
    let start = Instant::now();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let tf_src_dir = out_dir.join("tensorflow/tensorflow");
    let submodules = submodules();

    let copy_dir = fs_extra::dir::CopyOptions {
        overwrite: true,
        skip_exist: false,
        buffer_size: 65536,
        copy_inside: false,
        depth: 0,
    };

    if !tf_src_dir.exists() {
        fs_extra::dir::copy(submodules.join("tensorflow"), &out_dir, &copy_dir)
            .expect("Unable to copy tensorflow");
    }

    println!("Moving source took {:?}", start.elapsed());

    tf_src_dir
}

/// Return a Vec of all *.cc files in `path`, excluding those that have a
/// name containing 'test.cc'
fn get_cc_files_glob(path: PathBuf) -> Vec<String> {
    let mut paths: Vec<String> = vec![];

    for entry in glob(&path.to_string_lossy()).unwrap() {
        let p: PathBuf = entry.unwrap();
        paths.push(p.to_string_lossy().to_string());
    }

    paths
        .into_iter()
        .filter(|p| !p.contains("test.cc"))
        .filter(|p| !p.contains("debug_log.cc"))
        .collect()
}

fn cc_tensorflow_library() {
    let tflite = prepare_tensorflow_source();
    let out_dir = env::var("OUT_DIR").unwrap();
    let tf_lib_name =
        Path::new(&out_dir).join(format!("libtensorflow-microlite.a"));

    if !tf_lib_name.exists() || cfg!(feature = "build") {
        println!("Building tflite");
        let start = Instant::now();

        cc::Build::new()
            .cpp(true)
            .flag("-std=c++11")
            //.flag("-O3")
            .warnings(false) // TODO remove
            .extra_warnings(false)
            .define("TF_LITE_STATIC_MEMORY", None)
            .include(tflite.parent().unwrap())
            .include(tflite.join("lite/micro/tools/make/downloads"))
            .include(tflite.join("lite/micro/tools/make/downloads/gemmlowp"))
            .include(
                tflite.join(
                    "lite/micro/tools/make/downloads/flatbuffers/include",
                ),
            )
            .include(tflite.join("lite/micro/tools/make/downloads/ruy"))
            .files(get_cc_files_glob(tflite.join("lite/micro/*.cc")))
            .files(get_cc_files_glob(tflite.join("lite/micro/kernels/*.cc")))
            .files(get_cc_files_glob(
                tflite.join("lite/micro/memory_planner/*.cc"),
            ))
            .file(tflite.join("lite/c/common.c"))
            .file(tflite.join("lite/core/api/error_reporter.cc"))
            .file(tflite.join("lite/core/api/flatbuffer_conversions.cc"))
            .file(tflite.join("lite/core/api/op_resolver.cc"))
            .file(tflite.join("lite/core/api/tensor_utils.cc"))
            .file(tflite.join("lite/kernels/internal/quantization_util.cc"))
            .file(tflite.join("lite/kernels/kernel_util.cc"))
            .file(tflite.join("lite/micro/testing/test_utils.cc"))
            .compile("tensorflow-microlite");

        println!("Building tflite from source took {:?}", start.elapsed());
    } else {
        println!("Didn't rebuild tflite, using {:?}", tf_lib_name);

        println!("cargo:rustc-link-lib=static=tensorflow-microlite");
        println!("cargo:rustc-link-search=native={}", out_dir);
        println!("cargo:rustc-link-lib=stdc++");
    }
}

/// Configure bindgen for cross-compiling
fn bindgen_cross_builder() -> Result<bindgen::Builder> {
    let builder = bindgen::Builder::default().clang_arg("--verbose");

    if is_cross_compiling()? {
        // Setup target triple
        let target = env::var("TARGET")?;
        let builder = builder.clang_arg(format!("--target={}", target));
        println!("Setting bindgen to cross compile to {}", target);

        // Find the sysroot used by the crosscompiler, and pass this to clang
        let path = cc::Build::new()
            .get_compiler()
            .to_command()
            .arg("--print-sysroot")
            .output()
            .chain_err(|| "Couldn't find target GCC executable.")
            .and_then(|output| {
                if output.status.success() {
                    Ok(String::from_utf8(output.stdout)?)
                } else {
                    panic!("Couldn't determine target GCC sysroot.")
                }
            })?;
        let builder = builder.clang_arg(format!("--sysroot={}", path.trim()));

        // Add a path to the system headers for the target
        // compiler. Possibly we end up using a gcc header with clang
        // frontend, which is sketchy.
        let _path = cc::Build::new()
            .cpp(true)
            .get_compiler()
            .to_command()
            .arg("-E")
            .arg("-Wp,-v")
            .arg("-xc++")
            .arg(".")
            .output()
            .chain_err(|| "Couldn't find target GCC executable.")
            .and_then(|output| {
                // We have to scrape the gcc console output to find where
                // the c++ headers are. If we only needed the c headers we
                // could use `--print-file-name=include` but that's not
                // possible.
                let console_output = String::from_utf8(output.stderr)?;
                println!("From gcc invocation: {:?}", console_output);
                // TODO scraping
                Ok(console_output)
            })?;

        //println!("Found path {}", path.trim());

        // TODO add scraped paths here
        Ok(builder
            .clang_arg("-I/usr/arm-none-eabi/include")
            .clang_arg("-I/usr/lib/gcc/arm-none-eabi/7.3.1/include")
            .clang_arg("-I/usr/arm-none-eabi/include/c++/7.3.1/")
            .clang_arg("-I/usr/arm-none-eabi/include/c++/7.3.1/arm-none-eabi")
            .detect_include_paths(false))
    } else {
        Ok(builder)
    }
}

/// This generates "tflite_types.rs" containing structs and enums which are
/// inter-operable with rust
fn bindgen_tflite_types() {
    use bindgen::*;

    let submodules = submodules();
    let submodules_str = submodules.to_string_lossy();

    println!("Running bindgen");
    let start = Instant::now();

    let bindings = bindgen_cross_builder()
        .expect("Error setting up bindgen for cross compiling")
        .whitelist_recursively(true)
        .prepend_enum_name(false)
        .impl_debug(true)
        .with_codegen_config(CodegenConfig::TYPES)
        .layout_tests(false)
        .enable_cxx_namespaces()
        .derive_default(true)
        .size_t_is_usize(true)
        .use_core()
        .ctypes_prefix("cty")
        // Types
        .whitelist_type("tflite::ErrorReporter")
        .opaque_type("tflite::ErrorReporter")
        .whitelist_type("tflite::Model")
        .opaque_type("tflite::Model")
        .whitelist_type("tflite::MicroInterpreter")
        .opaque_type("tflite::MicroInterpreter")
        .whitelist_type("tflite::ops::micro::AllOpsResolver")
        .opaque_type("tflite::ops::micro::AllOpsResolver")
        .whitelist_type("tflite::MicroOpResolver")
        .opaque_type("tflite::MicroOpResolver")
        .whitelist_type("TfLiteTensor")
        // Types - blacklist
        .blacklist_type("std")
        .blacklist_type("tflite::Interpreter_TfLiteDelegatePtr")
        .blacklist_type("tflite::Interpreter_State")
        .default_enum_style(EnumVariation::Rust {
            non_exhaustive: false,
        })
        .derive_partialeq(true)
        .derive_eq(true)
        .header("csrc/tflite_wrapper.hpp")
        .clang_arg(format!("-I{}/tensorflow", submodules_str))
        .clang_arg(format!(
            // -> flatbuffers/flatbuffers.h
            "-I{}",
            flatbuffers_include_dir().to_string_lossy()
        ))
        .clang_arg("-DGEMMLOWP_ALLOW_SLOW_SCALAR_FALLBACK")
        //.clang_arg("-stdlib=libc++")
        .clang_arg("-xc++")
        .clang_arg("-std=c++11");

    let bindings = bindings.generate().expect("Unable to generate bindings");

    // Write the bindings to $OUT_DIR/tflite_types.rs
    let out_path =
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("tflite_types.rs");
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings!");

    println!("Running bindgen took {:?}", start.elapsed());
}

fn build_inline_cpp() {
    let submodules = submodules();

    println!("Building inline cpp");
    let start = Instant::now();

    cpp_build::Config::new()
        .include(submodules.join("tensorflow"))
        .include(flatbuffers_include_dir())
        .cpp_link_stdlib(None)
        .flag("-fPIC")
        .flag("-std=c++14")
        .flag("-Wno-sign-compare")
        .define("GEMMLOWP_ALLOW_SLOW_SCALAR_FALLBACK", None)
        .debug(true)
        .opt_level(if cfg!(debug_assertions) { 0 } else { 2 })
        .build("src/lib.rs");

    println!("Building inline cpp took {:?}", start.elapsed());
}

fn main() {
    bindgen_tflite_types();
    build_inline_cpp();
    cc_tensorflow_library();
}
