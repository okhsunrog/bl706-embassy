use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=vendor");
    if env::var_os("CARGO_FEATURE_CONTROLLER").is_none()
        || env::var("CARGO_CFG_TARGET_ARCH").unwrap() != "riscv32"
    {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for (name, expected) in [
        (
            "libblecontroller_bl702_m0s1.a",
            "94d05d9c87b64703bb6b8c330ea6b21a5f40258e3301324741e9141be7e2a9b5",
        ),
        (
            "libbl702_rf.a",
            "94dd54071072ea241367b094b109f97a85e731e301f94cce3946d89741893ad6",
        ),
    ] {
        let bytes = fs::read(format!("vendor/{name}")).unwrap();
        assert_eq!(
            Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            expected,
            "vendor archive hash"
        );
        fs::write(out.join(name), bytes).unwrap();
    }
    // Keep the original archive unchanged and remove only its RTOS/platform
    // wrapper from the build copy. Rust supplies this exact boundary instead.
    assert!(
        Command::new("ar")
            .current_dir(&out)
            .args(["d", "libblecontroller_bl702_m0s1.a", "arch_main.o"])
            .status()
            .unwrap()
            .success()
    );
    let symbols = Command::new("nm")
        .arg("-u")
        .arg(out.join("libblecontroller_bl702_m0s1.a"))
        .output()
        .unwrap();
    assert!(symbols.status.success());
    let symbols = String::from_utf8(symbols.stdout).unwrap();
    for forbidden in ["xTask", "vTask", "xQueue", "vQueue", "uxTask", "pvPort"] {
        assert!(
            !symbols.contains(forbidden),
            "unremoved RTOS import: {forbidden}"
        );
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=blecontroller_bl702_m0s1");
    println!("cargo:rustc-link-lib=static=bl702_rf");
}
