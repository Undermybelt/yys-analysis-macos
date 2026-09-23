fn main() {
    // 构建脚本只生成 Tauri 所需元数据，不在编译期读取用户环境数据。
    tauri_build::build();
}
