// rust-embed needs the folder to exist; an empty one just means "UI not built" at runtime.
fn main() {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
    if !d.join("index.html").exists() {
        println!("cargo:warning=apps/web/dist/index.html missing: run `npm run build` in apps/web");
        let _ = std::fs::create_dir_all(&d);
    }
    println!("cargo:rerun-if-changed=../../apps/web/dist");
}
