//! Windows resources for lensz.exe: the app icon (shown by File Explorer, shortcuts, the taskbar)
//! and version info.

fn main() {
    println!("cargo:rerun-if-changed=installer/lensz.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("installer/lensz.ico").set("ProductName", "Lensz").set("FileDescription", "Lensz camera filters");
        res.compile().expect("compile Windows resources");
    }
}
