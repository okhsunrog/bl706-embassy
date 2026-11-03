fn main() {
    let target = std::env::var("TARGET").unwrap();
    let fpu = std::env::var_os("CARGO_FEATURE_FPU").is_some();
    assert!(
        matches!(
            target.as_str(),
            "riscv32imc-unknown-none-elf" | "riscv32imfc-unknown-none-elf"
        ),
        "BL706 bring-up supports IMC or IMFC without hardware atomics only"
    );
    assert_eq!(
        fpu,
        target == "riscv32imfc-unknown-none-elf",
        "IMFC requires fpu (startup and IRQ FP context); IMC must omit fpu"
    );
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("bl706.x"), include_bytes!("bl706.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=bl706.x");
}
