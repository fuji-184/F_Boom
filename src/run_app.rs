use tokio::io::{AsyncBufReadExt, BufReader};

pub fn run_app(
    app_config: &crate::config_reader::App,
) -> Result<(tokio::process::Child, Option<tokio::process::Child>), Box<dyn std::error::Error>> {
    let mut backend_ready = false;
    let command = app_config.command.clone().unwrap();
    let perf = app_config.perf.unwrap();
    let child_result = if app_config.terminal {
        match command.args {
            Some(args) => tokio::process::Command::new(command.first.clone())
                .args(args)
                .kill_on_drop(true)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn(),
            None => tokio::process::Command::new(command.first.clone())
                .kill_on_drop(true)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn(),
        }
    } else {
        match command.args {
            Some(args) => tokio::process::Command::new(command.first.clone())
                .args(args)
                .kill_on_drop(true)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn(),
            None => tokio::process::Command::new(command.first.clone())
                .kill_on_drop(true)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn(),
        }
    };

    let mut child = match child_result {
        Ok(handle) => {
            backend_ready = true;
            handle
        }
        Err(e) => {
            println!("failed to start {}, reason : {:?}", command.first, e);
            return Err(Box::new(e));
        }
    };

    let pid = child.id().unwrap();

    println!("waiting {} for ready", command.first);

    std::thread::sleep(std::time::Duration::from_millis(1000));

    while !backend_ready {}
    println!("{} is ready", command.first);

    println!("{} started with pid {}", command.first, pid);

    if app_config.terminal {
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        app_stdout(stdout, &command.first);
        app_stderr(stderr, pid, &command.first);
    }

    if perf {
        let perf = run_perf(pid, &command.first).expect("failed to start perf");
        return Ok((child, Some(perf)));
    }

    Ok((child, None))
}

fn run_perf(pid: u32, command: &str) -> Result<tokio::process::Child, Box<std::io::Error>> {
    let perf_result = tokio::process::Command::new("perf")
        .arg("stat")
        .arg("-e")
        .arg("cycles,task-clock,context-switches,cpu-migrations,instructions,branches,branch-misses,cache-references,cache-misses,page-faults")
        .arg("-p")
        .arg(pid.to_string())
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut perf_ready = false;

    let mut perf = match perf_result {
        Ok(handle) => {
            perf_ready = true;
            handle
        }
        Err(e) => {
            println!("failed to start perf: {:?}", e);
            return Err(Box::new(e));
        }
    };

    while !perf_ready {}
    println!("perf is ready");

    let perf_stderrr = perf.stderr.take().unwrap();
    let perf_pid = perf.id().unwrap();
    perf_stderr(perf_stderrr, perf_pid, command);

    Ok(perf)
}

fn pid_alive(pid: u32) -> bool {
    //    unsafe { libc::kill(pid as i32, 0) == 0 }
    std::path::Path::new(&format!("/proc/{}", pid)).exists()
}

fn app_stdout(stdout: tokio::process::ChildStdout, command: &str) {
    let command = command.to_owned();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            println!("[{}] {}", command, line);
        }
    });
}

fn app_stderr(stderr: tokio::process::ChildStderr, pid: u32, command: &str) {
    let command = command.to_owned();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        let cmd = format!("/proc/{}", pid);
        let path = std::path::Path::new(&cmd);
        while let Ok(Some(line)) = reader.next_line().await {
            eprintln!("[{}] {}", command, line);
            if !path.exists() {
                eprintln!("Backend crashed (pid gone)");
                //return Ok(Box::new(()));
            }
        }
    });
}

fn perf_stderr(stderr: tokio::process::ChildStderr, pid: u32, command: &str) {
    let command = command.to_owned();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            eprintln!("[{}] {}", command, line);
            if !pid_alive(pid) {
                eprintln!("Perf crashed (pid gone)");
                //return Ok(Box::new(()));
            }
        }
    });
}
