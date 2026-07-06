use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct AppHandle {
    pub child: tokio::process::Child,
    pub perf:  Option<tokio::process::Child>,
}

impl AppHandle {
    pub async fn kill(mut self) {
        let _ = self.child.kill().await;
        if let Some(mut p) = self.perf {
            let _ = p.wait().await;
        }
    }
}

// Spawn the application described by app_config
// Returns None (with an error printed) if the process cannot be started
pub fn spawn(app: &crate::config::App) -> Option<AppHandle> {
    let cmd_cfg = app.command.as_ref()?;
    let show_output = app.terminal.unwrap_or(false);
    let capture_perf = app.perf.unwrap_or(false);

    let mut cmd = tokio::process::Command::new(&cmd_cfg.first);
    if let Some(args) = &cmd_cfg.args {
        cmd.args(args);
    }
    cmd.kill_on_drop(true);

    if show_output {
        cmd.stdout(std::process::Stdio::piped())
           .stderr(std::process::Stdio::piped());
    } else {
        cmd.stdout(std::process::Stdio::null())
           .stderr(std::process::Stdio::null());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "error: failed to start '{}': {}",
                cmd_cfg.first, e
            );
            return None;
        }
    };

    let pid = child.id().unwrap_or(0);
    println!("started '{}' with pid {}", cmd_cfg.first, pid);

    // Give the process a moment to bind its socket / initialize
    // We do NOT busy loop here
    std::thread::sleep(Duration::from_millis(1000));

    if show_output {
        if let Some(stdout) = child.stdout.take() {
            stream_lines(stdout, cmd_cfg.first.clone(), false);
        }
        if let Some(stderr) = child.stderr.take() {
            stream_lines(stderr, cmd_cfg.first.clone(), true);
        }
    }

    let perf = if capture_perf {
        run_perf(pid, &cmd_cfg.first)
    } else {
        None
    };

    Some(AppHandle { child, perf })
}

fn run_perf(pid: u32, label: &str) -> Option<tokio::process::Child> {
    if pid == 0 { return None; }

    let mut cmd = tokio::process::Command::new("perf");
    cmd.args([
        "stat", "-e",
        "cycles,task-clock,context-switches,cpu-migrations,\
         instructions,branches,branch-misses,cache-references,\
         cache-misses,page-faults",
        "-p", &pid.to_string(),
    ])
    .kill_on_drop(true)
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped());

    match cmd.spawn() {
        Ok(mut child) => {
            if let Some(stderr) = child.stderr.take() {
                stream_lines(stderr, format!("perf[{label}]"), true);
            }
            println!("perf attached to pid {pid}");
            Some(child)
        }
        Err(e) => {
            eprintln!("warning: could not start perf for pid {pid}: {e}");
            None
        }
    }
}

fn stream_lines<R>(reader: R, label: String, is_stderr: bool)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if is_stderr {
                eprintln!("[{label}] {line}");
            } else {
                println!("[{label}] {line}");
            }
        }
    });
}