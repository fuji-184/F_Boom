use tokio::io::{AsyncBufReadExt, BufReader};

pub fn run_app() -> Result<tokio::process::Child, Box<dyn std::error::Error>> {
  let mut backend_ready = false;
    
    let child_result = tokio::process::Command::new("./tes/target/release/tes")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child_result {
        Ok(handle) => {
            backend_ready = true;
            handle
        }
        Err(e) => {
            println!("failed to start backend: {:?}", e);
            return Err(Box::new(e));
        }
    };
    let pid = child.id().unwrap();
    
    println!("waiting backend for ready");
    while !backend_ready {
      
    }
    println!("backend is ready");
    
    println!("Backend started with pid {}", pid);
    
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    
    app_stdout(stdout);
    app_stderr(stderr, pid);
    
    Ok(child)
}

fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn app_stdout(stdout: tokio::process::ChildStdout){
  tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            println!("[backend stdout] {}", line);
        }
    });
}

fn app_stderr(stderr: tokio::process::ChildStderr, pid: u32){
  tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            eprintln!("[backend stderr] {}", line);
            if !pid_alive(pid) {
                eprintln!("Backend crashed (pid gone)");
              //return Ok(Box::new(()));
            }
        }
      
    });
}