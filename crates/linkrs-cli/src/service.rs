use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn linkrs_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "cannot determine home directory".to_string())?;
    let dir = PathBuf::from(home).join(".linkrs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn pid_file() -> Result<PathBuf, String> {
    Ok(linkrs_dir()?.join("server.pid"))
}

fn log_file() -> Result<PathBuf, String> {
    Ok(linkrs_dir()?.join("server.log"))
}

fn server_bin(explicit: Option<&str>) -> String {
    explicit
        .map(|s| s.to_string())
        .or_else(|| std::env::var("LINKRS_SERVER_BIN").ok())
        .unwrap_or_else(|| "linkrs-server".to_string())
}

fn read_pid() -> Result<Option<u32>, String> {
    let path = pid_file()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let pid = content.trim().parse::<u32>().map_err(|e| e.to_string())?;
    Ok(Some(pid))
}

fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if !Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return false;
        }
        // Treat zombie processes as not running.
        if let Ok(stat) = fs::read_to_string(format!("/proc/{}/stat", pid)) {
            let mut fields = stat.split_whitespace();
            let _pid = fields.next();
            let _comm = fields.next();
            if let Some(state) = fields.next() {
                if state == "Z" {
                    return false;
                }
            }
        }
        true
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

fn systemctl_available() -> bool {
    Command::new("systemctl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn unit_installed() -> bool {
    if !systemctl_available() {
        return false;
    }
    Command::new("systemctl")
        .args(["list-unit-files", "linkrs.service"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("linkrs.service"))
        .unwrap_or(false)
}

fn service_active() -> bool {
    if !systemctl_available() {
        return false;
    }
    Command::new("systemctl")
        .args(["is-active", "linkrs"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cleanup_stale_pid() {
    if let Ok(Some(pid)) = read_pid() {
        if !is_alive(pid) {
            let _ = fs::remove_file(pid_file().unwrap_or_default());
        }
    }
}

pub fn start(
    bin: Option<&str>,
    config: Option<&str>,
    data_dir: Option<&str>,
    http_port: Option<u16>,
    grpc_port: Option<u16>,
    foreground: bool,
) -> Result<(), String> {
    cleanup_stale_pid();
    if let Some(pid) = read_pid()? {
        if is_alive(pid) {
            return Err(format!("linkrs-server already running (pid {})", pid));
        }
        let _ = fs::remove_file(pid_file()?);
    }

    let mut cmd = Command::new(server_bin(bin));
    cmd.arg("serve");
    if let Some(v) = config {
        cmd.args(["--config", v]);
    }
    if let Some(v) = data_dir {
        cmd.args(["--data-dir", v]);
    }
    if let Some(v) = http_port {
        cmd.args(["--http-port", &v.to_string()]);
    }
    if let Some(v) = grpc_port {
        cmd.args(["--grpc-port", &v.to_string()]);
    }

    if foreground {
        let status = cmd.status().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("linkrs-server exited with status {}", status))
        }
    } else {
        let log = log_file()?;
        let file = fs::File::create(&log).map_err(|e| e.to_string())?;
        let child = cmd
            .stdin(Stdio::null())
            .stdout(file.try_clone().map_err(|e| e.to_string())?)
            .stderr(file)
            .spawn()
            .map_err(|e| e.to_string())?;
        fs::write(pid_file()?, child.id().to_string()).map_err(|e| e.to_string())?;
        println!(
            "Started linkrs-server (pid {}) log={}",
            child.id(),
            log.display()
        );
        Ok(())
    }
}

pub fn stop() -> Result<(), String> {
    cleanup_stale_pid();
    if systemctl_available() && service_active() {
        let _ = Command::new("systemctl").args(["stop", "linkrs"]).status();
        for _ in 0..50 {
            if !service_active() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if service_active() {
            return Err("failed to stop linkrs-server via systemd".to_string());
        }
        let _ = fs::remove_file(pid_file().unwrap_or_default());
        println!("Stopped linkrs-server via systemd");
        return Ok(());
    }

    match read_pid()? {
        Some(pid) if is_alive(pid) => {
            #[cfg(unix)]
            {
                let _ = Command::new("kill").arg(pid.to_string()).status();
            }
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .status();
            }
            for _ in 0..50 {
                if !is_alive(pid) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if is_alive(pid) {
                return Err(format!("failed to stop linkrs-server (pid {})", pid));
            }
            let _ = fs::remove_file(pid_file()?);
            println!("Stopped linkrs-server (pid {})", pid);
            Ok(())
        }
        _ => {
            let _ = fs::remove_file(pid_file().unwrap_or_default());
            Err("linkrs-server is not running".to_string())
        }
    }
}

pub fn status() -> Result<(), String> {
    cleanup_stale_pid();
    if systemctl_available() {
        let active = Command::new("systemctl")
            .args(["is-active", "linkrs"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let enabled = Command::new("systemctl")
            .args(["is-enabled", "linkrs"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        println!(
            "systemd active: {}",
            if active.is_empty() {
                "unknown"
            } else {
                &active
            }
        );
        println!(
            "systemd enabled: {}",
            if enabled.is_empty() {
                "unknown"
            } else {
                &enabled
            }
        );

        if active == "active" {
            // Validate the reported main PID when possible.
            if let Ok(out) = Command::new("systemctl")
                .args(["show", "linkrs", "-p", "MainPID", "--value"])
                .output()
            {
                let pid_text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(pid) = pid_text.parse::<u32>() {
                    if pid > 0 {
                        if is_alive(pid) {
                            println!("linkrs-server is running (pid {})", pid);
                            return Ok(());
                        }
                        println!("systemd reports pid {} but process is not running", pid);
                    }
                }
            }
            println!("linkrs-server is running via systemd");
            return Ok(());
        }
    }

    match read_pid()? {
        Some(pid) if is_alive(pid) => {
            println!("linkrs-server is running (pid {})", pid);
            Ok(())
        }
        Some(pid) => {
            println!(
                "linkrs-server pid file exists but process {} is not running",
                pid
            );
            let _ = fs::remove_file(pid_file().unwrap_or_default());
            Err("not running".to_string())
        }
        None => {
            println!("linkrs-server is not running");
            Ok(())
        }
    }
}

pub fn restart(
    bin: Option<&str>,
    config: Option<&str>,
    data_dir: Option<&str>,
    http_port: Option<u16>,
    grpc_port: Option<u16>,
) -> Result<(), String> {
    cleanup_stale_pid();
    if systemctl_available() && unit_installed() {
        let _ = stop();
        let status = Command::new("systemctl")
            .args(["restart", "linkrs"])
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("failed to restart linkrs-server via systemd".to_string());
        }
        println!("Restarted linkrs-server via systemd");
        return Ok(());
    }
    let _ = stop();
    start(bin, config, data_dir, http_port, grpc_port, false)
}
