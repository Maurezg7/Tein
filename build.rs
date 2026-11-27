fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        let bridge = "packaging/apple/ios.m";
        println!("cargo:rerun-if-changed={bridge}");
        cc::Build::new()
            .file(bridge)
            .flag("-fobjc-arc")
            .compile("tein_ios");
        for framework in [
            "UIKit",
            "UniformTypeIdentifiers",
            "AVFoundation",
            "MediaPlayer",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // Some native dependencies are C++ and expect the NDK's shared runtime.
        println!("cargo:rustc-link-lib=c++_shared");
        // Android's packed relocations work from API 23, below our minimum (26).
        println!("cargo:rustc-link-arg-cdylib=-Wl,--pack-dyn-relocs=android");
    }
    let mut config = slint_build::CompilerConfiguration::new().with_style("material-dark".into());
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        config = config.embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    }
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");
}
