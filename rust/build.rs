// rust-embed needs the folder to exist at compile time. The frontend build
// (`cd web && npm ci && npm run build`) fills it; without it the binary is
// built with an empty frontend and warns at start (same as the Go version).
fn main() {
    let _ = std::fs::create_dir_all("../web/dist");
    println!("cargo:rerun-if-changed=../web/dist");
}
