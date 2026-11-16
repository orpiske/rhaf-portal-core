# RHAF Portal Core

RHAF Portal Core is the Rust runtime for RHAF Portal. The runtime builds on the [Wanaku](https://github.com/wanaku-ai/wanaku) project.


## Build and run

Rust 1.96.0 is pinned in `rust-toolchain.toml`. Install Rust through rustup, then run:

```bash
cargo build --workspace --locked
cargo run --locked
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The workspace builds on Wanaku's Rust and Clippy settings. It also denies
`todo!()` and `unimplemented!()` to reject unfinished code. The scaffold has no
runtime dependencies. Integration and end-to-end test infrastructure will be added later.

## Container image

The multi-stage `Containerfile` uses Fedora 44 to build the binary and
Fedora Minimal 44 to run it as a non-root user. The bootstrap image prints the
same greeting and exits. It does not expose an API or health endpoint yet.

Build with Docker or Podman:

```bash
cargo build-image --tag localhost/rhaf-portal-core:dev
podman run --rm localhost/rhaf-portal-core:dev-aarch64
```

Use `-x86_64` on an Intel or AMD host. The build command adds the native
architecture suffix. Docker takes precedence when both tools are installed.
Use the same tool to run the image.

To publish a multi-architecture image, run the first command on both Linux x86_64
and ARM64 hosts. Then run the second command once. Manifest publication requires
Podman and registry authentication.

```bash
cargo build-image --push --tag REGISTRY/GROUP/rhaf-portal-core:TAG
cargo build-image-manifest --tag REGISTRY/GROUP/rhaf-portal-core:TAG
```

## CI and releases

- `pr-build`: build, unit tests, formatting, Clippy, and container builds.
- `main-build`: Linux x86_64 and ARM64 builds and container publication.
- `lychee-links`: Markdown links and formatting.
- `early-access`: manually publish four-platform archives and rolling prereleases.
- `release`: publish versioned archives and images from a version tag or manual run.

Container publication uses organization variables `CONTAINER_REGISTRY` and
`CONTAINER_GROUP`, plus secrets `QUAY_USERNAME` and `QUAY_PASSWORD`.
JReleaser signs release archives with secrets `GPG_PUBLIC_KEY`, `GPG_PRIVATE_KEY`,
and `MAVEN_GPG_PASSPHRASE`, following Wanaku's release setup. GitHub releases use
the workflow's `GITHUB_TOKEN`.

For early access, supply the Cargo package version (`0.1.0-dev` initially).
For a stable release, first update the workspace version and `Cargo.lock` to the
release version. Push a matching version tag, such as `v0.1.0`, or supply the
version to the manual release workflow. Version validation runs before publication.

Dependabot checks Cargo dependencies and GitHub Actions weekly.

## License

[Apache License 2.0](LICENSE).
