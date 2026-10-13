use std::io::{BufRead, BufReader};
use std::time::Duration;

// Sinkron (std) agar bisa dipakai tanpa Tokio runtime.
// Share-nothing benchmark tidak lagi memiliki runtime global,
// jadi manajemen proses server tidak boleh bergantung padanya.

pub struct AppHandle {
    pub child: std::process::Child,
    pub perf:  Option<std::process::Child>,
}

impl AppHandle {
    pub fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(mut p) = self.perf {
            let _ = p.wait();
        }
    }
}

// Spawn the application described by app_config
// Returns None (with an error printed) if the process cannot be started
pub fn spawn(app: &crate::config::App) -> Option<AppHandle> {
    let cmd_cfg = app.command.as_ref()?;
    let show_output = app.terminal.unwrap_or(false);
    let capture_perf = app.perf.unwrap_or(false);

    let mut cmd = std::process::Command::new(&cmd_cfg.first);
    if let Some(args) = &cmd_cfg.args {
        cmd.args(args);
    }

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

    let pid = child.id();
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

fn run_perf(pid: u32, label: &str) -> Option<std::process::Child> {
    if pid == 0 { return None; }

    match std::process::Command::new("perf")
        .args([
            "stat", "-e",
            "cycles,task-clock,context-switches,cpu-migrations,\
             instructions,branches,branch-misses,cache-references,\
             cache-misses,page-faults",
            "-p", &pid.to_string(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
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
    R: std::io::Read + Send + 'static,
{
    // Satu OS thread per stream; tidak bergantung pada runtime async.
    std::thread::spawn(move || {
        let lines = BufReader::new(reader).lines();
        for line in lines.flatten() {
            if is_stderr {
                eprintln!("[{label}] {line}");
            } else {
                println!("[{label}] {line}");
            }
        }
    });
}
