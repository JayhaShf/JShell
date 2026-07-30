fn main() {
    #[cfg(windows)]
    {
        // GPUI's deeply nested debug render paths can exceed the PE default
        // 1 MiB UI-thread stack when the terminal and SFTP workspace open.
        println!("cargo:rustc-link-arg-bin=jshell=/STACK:8388608");

        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icons/jshell.ico");
        res.compile().unwrap();
    }
}
