use glob::glob;
use rand::seq::SliceRandom;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use super::{command_output_logfile, log, test_available_remotes, LogTarget};

async fn untrack_embedded_git(search_path: &PathBuf, log_target: &mut LogTarget<'_>) {
    log(
        &format!("untracked-embedded-git {}", search_path.display()),
        log_target,
    )
    .await;

    for entry in glob(search_path.join(Path::new("**/.git")).to_str().unwrap())
        .expect("unable to read glob pattern")
        .filter_map(Result::ok)
    {
        if !entry.parent().unwrap().eq(search_path) {
            let _ = Command::new("git")
                .args([
                    "rm",
                    "-r",
                    "--cached",
                    entry
                        .as_os_str()
                        .to_str()
                        .unwrap()
                        .strip_suffix(".git")
                        .unwrap(),
                ])
                .current_dir(search_path)
                .output()
                .await;
            log(&format!("git-rm-cached {} ok", entry.display()), log_target).await;
        }
    }
    log(
        &format!("untracked-embedded-git {} ok", search_path.display()),
        log_target,
    )
    .await;
}

pub(crate) async fn maintain(
    repo_paths: &Vec<PathBuf>,
    check_timeout_m: u64,
    log_targets: (&mut LogTarget<'_>, &mut LogTarget<'_>),
    notify_progress: impl Fn(String),
) -> Result<bool, ()> {
    let (log_target, log_target_sync) = log_targets;
    if let Err(_e) = tokio::time::timeout(
        std::time::Duration::from_secs(check_timeout_m * 60),
        async move {
            for (repo_index, repo_path) in repo_paths.iter().enumerate() {
                notify_progress(format!("Preparation, {} of {}", repo_index + 1, repo_paths.len()));
                untrack_embedded_git(repo_path, log_target).await;

                command_output_logfile(
                    Command::new("git").args(["fsck"]).current_dir(repo_path),
                    format!("git-fsck {:?}", repo_path.display()),
                    log_target,
                )
                .await;

                command_output_logfile(
                    Command::new("git")
                        .args(["annex", "unused"])
                        .current_dir(repo_path),
                    format!("git-annex-unused {:?}", repo_path.display()),
                    log_target,
                )
                .await;

                command_output_logfile(
                    Command::new("git")
                        .args(["annex", "restage"])
                        .current_dir(repo_path),
                    format!("git-annex-restage {:?}", repo_path.display()),
                    log_target,
                )
                .await;
            }

            for (repo_index, repo_path) in repo_paths.iter().enumerate() {
                notify_progress(format!("{}/{}", repo_index + 1, repo_paths.len()));
                let available_remotes = test_available_remotes(repo_path, log_target).await;

                command_output_logfile(
                    Command::new("git")
                        .args(
                            [
                                vec!["annex", "satisfy", "--all"]
                                    .into_iter()
                                    .filter(|arg| !arg.is_empty())
                                    .collect::<Vec<&str>>(),
                                available_remotes
                                    .iter()
                                    .map(|remote| remote.as_str())
                                    .collect(),
                            ]
                            .concat(),
                        )
                        .current_dir(repo_path),
                    format!("git-annex-satisfy {:?}", repo_path.display()),
                    log_target,
                )
                .await;

                let mut remotes: Vec<Option<&str>> = available_remotes
                    .iter()
                    .map(|remote| Some(remote.as_str()))
                    .collect();
                remotes.push(None);
                remotes.shuffle(&mut rand::thread_rng());

                for remote in remotes {
                    let remote_arg = match remote {
                        Some(remote_id) => format!("--from={}", remote_id),
                        None => "".to_string(),
                    };
                    command_output_logfile(
                        Command::new("git")
                            .args(
                                [
                                    "annex",
                                    "fsck",
                                    "--incremental-schedule=15d",
                                    "--time-limit=2h",
                                    "--all",
                                    &remote_arg,
                                ]
                                .into_iter()
                                .filter(|arg| !arg.is_empty())
                                .collect::<Vec<&str>>(),
                            )
                            .current_dir(repo_path),
                        format!(
                            "git-annex-fsck {:?} {}",
                            repo_path.display(),
                            match remote {
                                Some(remote_id) => remote_id,
                                None => "here",
                            }
                        ),
                        log_target,
                    )
                    .await;

                    command_output_logfile(
                        Command::new("git")
                            .args(
                                ["annex", "dropunused", "all", &remote_arg]
                                    .into_iter()
                                    .filter(|arg| !arg.is_empty())
                                    .collect::<Vec<&str>>(),
                            )
                            .current_dir(repo_path),
                        format!(
                            "git-annex-dropunused {:?} {}",
                            repo_path.display(),
                            match remote {
                                Some(remote_id) => remote_id,
                                None => "here",
                            }
                        ),
                        log_target,
                    )
                    .await;
                }
            }
            log("ok", log_target).await;
        },
    )
    .await
    {
        log("not ok", log_target_sync).await;
        return Ok(false);
    }
    Ok(true)
}
