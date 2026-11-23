fn main() {
    // 让 cargo 在 tauri.*.conf.json(被 `tauri dev/build --config` 引用)内容变更时
    // 自动重新运行 tauri-build。否则这些 overlay 文件不在 tauri-build 自己的
    // read_from 里,它也不会发出 rerun-if-changed,改完 endpoint 会被 cargo 增量编译跳过,
    // 二进制里继续保留旧值。
    // 路径相对于 CARGO_MANIFEST_DIR(即本 build.rs 所在目录 src-tauri/)。
    for overlay in [
        "tauri.dev.conf.json",
        "tauri.test.conf.json",
        "tauri.production.conf.json",
    ] {
        println!("cargo:rerun-if-changed={overlay}");
    }

    tauri_build::build()
}
