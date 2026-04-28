fn main() {
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("assets/app.ico")
        .compile()
        .expect("embed Windows icon");
}
