fn main() {
    // Embeds the icon into the Windows executable (Explorer, taskbar).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=cannot embed the icon: {e}");
        }
    }
}
