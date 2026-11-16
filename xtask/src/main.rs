//! Container build tasks invoked through the Cargo aliases in `.cargo/config.toml`.

use std::process::{Command, Stdio};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

const DEFAULT_TAG: &str = "localhost/rhaf-portal-core:latest";

#[derive(Debug, Parser)]
#[command(about = "RHAF Portal build task runner")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    /// Build an image with the native architecture appended to its tag.
    BuildImage {
        /// Base image reference, including a tag.
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
        /// Push the image after building it.
        #[arg(long)]
        push: bool,
    },
    /// Publish a manifest from existing <tag>-x86_64 and <tag>-aarch64 images.
    BuildImageManifest {
        /// Base image reference used when building both architectures.
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Task::BuildImage { tag, push } => build_image(&tag, push),
        Task::BuildImageManifest { tag } => build_manifest(&tag),
    }
}

fn build_image(tag: &str, push: bool) -> anyhow::Result<()> {
    let tool = container_tool()?;
    let arch = native_arch()?;
    let image = format!("{tag}-{arch}");
    run(Command::new(tool).args(["build", "-f", "Containerfile", "-t", &image, "."]))?;
    if push {
        run(Command::new(tool).args(["push", &image]))?;
    }
    Ok(())
}

fn build_manifest(tag: &str) -> anyhow::Result<()> {
    // A previous local manifest may remain after a failed publication.
    let _ = Command::new("podman")
        .args(["manifest", "rm", tag])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    run(Command::new("podman").args([
        "manifest",
        "create",
        tag,
        &format!("docker://{tag}-aarch64"),
        &format!("docker://{tag}-x86_64"),
    ]))?;
    run(Command::new("podman").args(["manifest", "push", "--all", tag, &format!("docker://{tag}")]))
}

fn native_arch() -> anyhow::Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        arch => bail!("unsupported container architecture: {arch}"),
    }
}

fn container_tool() -> anyhow::Result<&'static str> {
    for tool in ["docker", "podman"] {
        if Command::new(tool)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return Ok(tool);
        }
    }
    bail!("install Docker or Podman to build container images")
}

fn run(command: &mut Command) -> anyhow::Result<()> {
    println!("Running {command:?}");
    let status = command
        .status()
        .with_context(|| format!("failed to start {command:?}"))?;
    if !status.success() {
        bail!("{command:?} failed: {status}");
    }
    Ok(())
}
