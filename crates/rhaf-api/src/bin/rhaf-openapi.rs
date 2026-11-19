use std::io::Write;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments.next();
    let check = first.as_deref() == Some(std::ffi::OsStr::new("--check"));
    let destination = if check { arguments.next() } else { first };
    if arguments.next().is_some() || (check && destination.is_none()) {
        return Err("usage: rhaf-openapi [--check] [PATH]".into());
    }
    match destination {
        Some(path) if check => {
            let path = PathBuf::from(path);
            if std::fs::read(&path)? != rhaf_api::OPENAPI_JSON {
                return Err(format!("OpenAPI snapshot differs: {}", path.display()).into());
            }
        }
        Some(path) => std::fs::write(path, rhaf_api::OPENAPI_JSON)?,
        None => std::io::stdout().lock().write_all(rhaf_api::OPENAPI_JSON)?,
    }
    Ok(())
}
