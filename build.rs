fn main() {
    let kit_dirs = [
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22000.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.20348.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\x64",
    ];

    let rc_path = kit_dirs.iter().map(|d| format!(r"{}\rc.exe", d)).find(|p| std::path::Path::new(p).exists());

    if let Some(ref rc_exe) = rc_path {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let rc_file = std::path::Path::new("src").join("app.rc");
        let res_file = std::path::Path::new(&out_dir).join("app.res");

        let status = std::process::Command::new(rc_exe)
            .arg(format!("/fo{}", res_file.display()))
            .arg(rc_file)
            .status()
            .expect("failed to execute rc.exe");

        if status.success() {
            println!("cargo:rustc-link-arg={}", res_file.display());
        }
    }

    println!("cargo:rerun-if-changed=src/app.rc");
    println!("cargo:rerun-if-changed=src/nz.ico");
}
