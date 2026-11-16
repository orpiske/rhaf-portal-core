# Project Standards

This rule file contains build tools, commands, and code style constraints for the project.

- **Build tool:** Cargo
- **Build command:** `cargo build --workspace --locked`
- **Test command:** `cargo test --workspace --locked`
- **Test with coverage command:** _(not configured)_
- **Format command:** `cargo fmt --all`
- **Lint command:** `cargo clippy --workspace --all-targets --locked -- -D warnings`
- **Module-specific build:** yes
- **Parallelized Maven:** n/a
- **Code style restrictions:** Rust 2024; MSRV 1.96; unsafe code, `unwrap`, `expect`, and `panic` are denied

## Version

1e25ac328876432cda1ca094a8cf41346f6b78f8
