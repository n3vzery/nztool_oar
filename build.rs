fn main() {
    let kit_dirs = [
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22000.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.20348.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\x64",
    ];

    let rc_exe = kit_dirs.iter().map(|d| format!(r"{}\rc.exe", d)).find(|p| std::path::Path::new(p).exists());

    if let Some(ref rc_exe) = rc_exe {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let rc_file = std::path::Path::new(&manifest_dir).join("src").join("app.rc");
        let res_file = std::path::Path::new(&out_dir).join("app.res");

        eprintln!("rc_exe: {}", rc_exe);
        eprintln!("rc_file: {}", rc_file.display());
        eprintln!("res_file: {}", res_file.display());

        let status = std::process::Command::new(rc_exe)
            .arg(format!("/fo{}", res_file.display()))
            .arg(&rc_file)
            .status()
            .expect("failed to execute rc.exe");

        eprintln!("rc.exe exit status: {}", status);

        if status.success() {
            println!("cargo:rustc-link-arg={}", res_file.display());
        }
    } else {
        eprintln!("rc.exe not found!");
    }

    println!("cargo:rerun-if-changed=src/app.rc");
    println!("cargo:rerun-if-changed=src/nz.ico");
}
