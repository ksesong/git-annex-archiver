use std::time::Instant;
use std::{path::PathBuf, process::Stdio, str::from_utf8};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout},
    process::Command,
};

pub mod maintain;
pub mod sync;

#[cfg(not(target_os = "linux"))]
pub mod allocate;

pub enum LogTarget<'a> {
    File(&'a mut File),
    Stdout(&'a mut Stdout),
}

pub async fn log(message: &str, target: &mut LogTarget<'_>) {
    let message = format!("{}\n", message);

    match target {
        LogTarget::File(file) => {
            file.write(message.as_bytes()).await.unwrap();
        }
        LogTarget::Stdout(stdout) => {
            stdout.write(message.as_bytes()).await.unwrap();
        }
    }
}

pub async fn command_output_logfile(
    command: &mut Command,
    status_prefix: String,
    log_target: &mut LogTarget<'_>,
) -> bool {
    log(&status_prefix, log_target).await;

    let mut child = match command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_e) => panic!("unable to start process"),
    };
    let stdout = child.stdout.take().expect("no handle to stdout");
    let stderr = child.stderr.take().expect("no handle to stderr");
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();
    let mut success = false;

    loop {
        tokio::select! {
            result = stdout_reader.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        log(&line, log_target).await;
                    },
                    Err(_) => break,
                    _ => (),
                }
            }
            result = stderr_reader.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        log(&line, log_target).await;
                    },
                    Err(_) => break,
                    _ => (),
                }
            }
            result = child.wait() => {
                match result {
                    Ok(exit_code) => {
                        log(&format!("{} {}",
                            status_prefix,
                            match exit_code.success() {
                                true => "ok",
                                false => "not ok",
                            }),
                            log_target
                        ).await;
                        success = exit_code.success();
                    },
                    _ => (),
                }
                break // child process exited
            }
        };
    }
    return success;
}

pub async fn test_available_remotes(
    repo_path: &PathBuf,
    log_target: &mut LogTarget<'_>,
) -> Vec<String> {
    let mut available_remotes: Vec<String> = vec![];

    log(
        &format!("test-available-remotes {}", repo_path.display()),
        log_target,
    )
    .await;

    for remote in Vec::from_iter(
        from_utf8(
            &Command::new("git")
                .args(["remote"])
                .current_dir(repo_path)
                .kill_on_drop(true)
                .output()
                .await
                .expect("unable to get remote list")
                .stdout,
        )
        .unwrap()
        .trim()
        .split_whitespace()
        .map(|x| String::from(x)),
    ) {
        let remote_url_stdout = &Command::new("git")
            .args(["remote", "get-url", &remote])
            .current_dir(repo_path)
            .output()
            .await
            .expect("unable to get remote list")
            .stdout;

        let remote_url = from_utf8(remote_url_stdout).unwrap().trim();
        if remote_url.starts_with("gcrypt::rsync://") {
            let ls_start = Instant::now();

            let is_ok = &Command::new("git")
                .args(["ls-remote", "--heads", "--exit-code", &remote_url])
                .current_dir(repo_path)
                .output()
                .await
                .expect("unable to fetch remote")
                .status
                .success();

            let ls_duration = Instant::now().duration_since(ls_start).as_millis();
            if *(is_ok) {
                let cost = 200 + ls_duration / 100;
                Command::new("git")
                    .args([
                        "config",
                        "--replace-all",
                        &format!("remote.{}.annex-cost", remote),
                        &format!("{}", cost),
                    ])
                    .current_dir(repo_path)
                    .output()
                    .await
                    .unwrap();
                Command::new("git")
                    .args([
                        "config",
                        "--replace-all",
                        &format!("remote.{}.annex-ignore", remote),
                        "false",
                    ])
                    .current_dir(repo_path)
                    .output()
                    .await
                    .unwrap();
                log(&format!("{} ({}) ok", remote, cost), log_target).await;
                available_remotes.push(remote);
            } else {
                Command::new("git")
                    .args([
                        "config",
                        "--replace-all",
                        &format!("remote.{}.annex-ignore", remote),
                        "true",
                    ])
                    .current_dir(repo_path)
                    .output()
                    .await
                    .unwrap();
                log(&format!("{} not ok", remote), log_target).await;
            }
        } else {
            log(&format!("{} ok", remote), log_target).await;
            available_remotes.push(remote);
        }
    }

    log(
        &format!("test-available-remotes {} ok", repo_path.display()),
        log_target,
    )
    .await;
    available_remotes
}
