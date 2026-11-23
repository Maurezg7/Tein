fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // Some native dependencies are C++ and expect the NDK's shared runtime.
        println!("cargo:rustc-link-lib=c++_shared");
    }
    let config = slint_build::CompilerConfiguration::new().with_style("material-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");
}
