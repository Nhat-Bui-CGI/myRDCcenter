fn main() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .unwrap();
    embed_windows_icon();
}

/// Put the app icon into the .exe (shown in Explorer and the taskbar).
fn embed_windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../assets/icon.ico");
        res.compile().expect("could not embed Windows icon");
    }
}
