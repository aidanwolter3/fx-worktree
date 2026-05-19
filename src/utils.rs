use anyhow::{Context, Result, anyhow};
use std::path::Path;
use std::process::Command;

pub fn run_command(
    cmd: &str,
    args: &[&str],
    cwd: &Path,
    env: &[(&str, &str)],
) -> Result<std::process::Output> {
    let mut command = Command::new(cmd);
    command.args(args);
    command.current_dir(cwd);
    for (key, val) in env {
        command.env(key, val);
    }

    log::debug!("Running command: {} {} in {:?}", cmd, args.join(" "), cwd);

    let output = command
        .output()
        .with_context(|| format!("Failed to execute command: {} {}", cmd, args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow!(
            "Command failed: {} {}\nExit Status: {}\nStdout: {}\nStderr: {}",
            cmd,
            args.join(" "),
            output.status,
            stdout,
            stderr
        ));
    }

    Ok(output)
}
