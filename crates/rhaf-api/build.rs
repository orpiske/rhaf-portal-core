#[path = "src/models.rs"]
mod models;
#[path = "src/openapi.rs"]
mod openapi;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/models.rs");
    println!("cargo:rerun-if-changed=src/openapi.rs");
    let directory = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(directory.join("openapi.json"), openapi::openapi_json()?)?;
    Ok(())
}
