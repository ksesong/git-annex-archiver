use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::{path::PathBuf, str::from_utf8};
use tokio::process::Command;

#[cfg(target_os = "macos")]
use crate::platform::macos::{has_file_drop_attr, set_file_drop_attr, unset_file_drop_attr};

#[cfg(target_os = "windows")]
use crate::platform::windows::{has_file_drop_attr, set_file_drop_attr, unset_file_drop_attr};

use super::{command_output_logfile, log, test_available_remotes, LogTarget};

#[derive(Serialize, Deserialize)]
struct AnnexLog {
    file: String,
}

static GET_MAX_CT: usize = 4;

pub async fn allocate(
    repo_paths: &Vec<PathBuf>,
    received_since: Option<DateTime<Local>>,
    log_target: &mut LogTarget<'_>,
    notify_progress: impl Fn(String),
) -> bool {
    let mut is_ok: bool = true;
    for (repo_index, repo_path) in repo_paths.iter().enumerate() {
        notify_progress(format!("{}/{}", repo_index + 1, repo_paths.len()));

        let mut is_repo_ok: bool = true;
        log(
            &format!("allocate-repo-files {}", repo_path.display()),
            log_target,
        )
        .await;

        let tracked_paths = HashSet::<PathBuf>::from_iter(
            from_utf8(
                &Command::new("git")
                    .args(["ls-files", "-z"])
                    .current_dir(repo_path)
                    .output()
                    .await
                    .expect("unable to get file list")
                    .stdout,
            )
            .unwrap()
            .trim()
            .split_terminator("\u{0}")
            .filter(|x| repo_path.join(x).try_exists().unwrap())
            .map(|x| PathBuf::from(x)),
        );
        log("tracked paths ok", log_target).await;

        let tracked_dropped_paths = HashSet::<PathBuf>::from_iter(
            from_utf8(
                &Command::new("git")
                    .args(["annex", "find", "--not", "--in=here", "--print0"])
                    .current_dir(repo_path)
                    .output()
                    .await
                    .expect("unable to get file list")
                    .stdout,
            )
            .unwrap()
            .trim()
            .split_terminator("\u{0}")
            .map(|x| PathBuf::from(x)),
        );
        log("tracked dropped paths ok", log_target).await;

        let received_paths = match received_since {
            None => tracked_paths.clone(),
            Some(received_since) => {
                let since_arg = format!("{}", received_since.format("%Y-%m-%d %H:%M:%S"));
                let modified_paths = tracked_paths
                    .iter()
                    .filter(|x| {
                        DateTime::<chrono::Local>::from(
                            repo_path.join(x).metadata().unwrap().modified().unwrap(),
                        ) > received_since
                    })
                    .map(|x| x.as_os_str().to_str().unwrap())
                    .collect::<Vec<&str>>();

                match modified_paths.is_empty() {
                    true => HashSet::<PathBuf>::new(),
                    false => HashSet::<PathBuf>::from_iter(
                        from_utf8(
                            &Command::new("git")
                                .args(
                                    [
                                        vec![
                                            "annex",
                                            "log",
                                            "--json",
                                            "--since",
                                            &since_arg,
                                            "--in=here",
                                            "--or",
                                            "--in",
                                            &format!("here@{}", since_arg),
                                        ],
                                        modified_paths,
                                    ]
                                    .concat(),
                                )
                                .current_dir(repo_path)
                                .output()
                                .await
                                .expect("unable to get file list")
                                .stdout,
                        )
                        .unwrap()
                        .trim()
                        .split_terminator("\n")
                        .map(|x| {
                            let v: AnnexLog = serde_json::from_str(x).unwrap();
                            PathBuf::from(v.file)
                        }),
                    ),
                }
            }
        };
        log("received paths ok", log_target).await;

        log(
            &format!("moved files ({})", received_paths.len()),
            log_target,
        )
        .await;

        for received_dropped_path in received_paths.intersection(&tracked_dropped_paths) {
            set_file_drop_attr(&repo_path.join(received_dropped_path), log_target).await;
        }
        for received_present_path in received_paths.difference(&tracked_dropped_paths) {
            unset_file_drop_attr(&repo_path.join(received_present_path), log_target).await;
        }

        if received_since.is_none() {
            for untracked_path in HashSet::<PathBuf>::from_iter(
                from_utf8(
                    &Command::new("git")
                        .args(["ls-files", "-z", "-o"])
                        .current_dir(repo_path)
                        .output()
                        .await
                        .expect("unable to get file list")
                        .stdout,
                )
                .unwrap()
                .trim()
                .split_terminator("\u{0}")
                .filter(|x| repo_path.join(x).try_exists().unwrap())
                .map(|x| PathBuf::from(x)),
            ) {
                unset_file_drop_attr(&repo_path.join(untracked_path), log_target).await;
            }
        }

        let send_paths = tracked_paths.difference(&received_paths);
        let send_paths_ct = send_paths.clone().count();
        log(&format!("files to move ({})", send_paths_ct), log_target).await;

        if send_paths_ct > 0 {
            let mut has_tested_available_remotes = false;

            let commit_date = DateTime::parse_from_rfc3339(
                from_utf8(
                    &Command::new("git")
                        .args(["log", "-1", "--format=%aI"])
                        .current_dir(repo_path)
                        .output()
                        .await
                        .expect("unable to get commit date")
                        .stdout,
                )
                .unwrap()
                .trim(),
            )
            .unwrap();
            let uncommitted_paths = tracked_paths
                .clone()
                .into_iter()
                .filter(|x| {
                    DateTime::<chrono::Local>::from(
                        repo_path.join(x).metadata().unwrap().modified().unwrap(),
                    ) > commit_date
                })
                .collect::<Vec<PathBuf>>();

            for send_path in send_paths {
                if has_file_drop_attr(&repo_path.join(send_path)) {
                    // Was present, now want to be dropped
                    if !tracked_dropped_paths.contains(send_path) {
                        if !has_tested_available_remotes {
                            test_available_remotes(repo_path, log_target).await;
                            has_tested_available_remotes = true;
                        }

                        if uncommitted_paths.contains(send_path) {
                            unset_file_drop_attr(&repo_path.join(send_path), log_target).await;
                            log("revert-drop-attribute, uncommited", log_target).await;
                        } else {
                            let is_command_ok = command_output_logfile(
                                Command::new("git")
                                    .args(["annex", "drop", &format!("{}", send_path.display())])
                                    .current_dir(repo_path),
                                format!("git-annex-drop {:?}", repo_path.display()),
                                log_target,
                            )
                            .await;
                            if is_command_ok {
                                set_file_drop_attr(&repo_path.join(send_path), log_target).await;
                            } else {
                                is_repo_ok = false;
                            }
                        }
                    }
                } else {
                    // Was dropped, now  want to be present
                    if tracked_dropped_paths.contains(send_path) {
                        if !has_tested_available_remotes {
                            test_available_remotes(repo_path, log_target).await;
                            has_tested_available_remotes = true;
                        }

                        if uncommitted_paths.contains(send_path) {
                            set_file_drop_attr(&repo_path.join(send_path), log_target).await;
                            log("revert-drop-attribute, uncommited", log_target).await;
                        } else {
                            let mut is_command_ok: bool = false;
                            let mut get_ct = 0;

                            while get_ct < GET_MAX_CT && !is_command_ok {
                                is_command_ok = command_output_logfile(
                                    Command::new("git")
                                        .args(["annex", "get", &format!("{}", send_path.display())])
                                        .current_dir(repo_path),
                                    format!("git-annex-get {:?}", repo_path.display()),
                                    log_target,
                                )
                                .await;
                                get_ct += 1;
                            }
                            if is_command_ok {
                                unset_file_drop_attr(&repo_path.join(send_path), log_target).await;
                            } else {
                                is_repo_ok = false;
                            }
                        }
                    }
                }
            }
        }
        log(
            &format!(
                "allocate-repo-files {} {}",
                repo_path.display(),
                match is_repo_ok {
                    true => "ok",
                    false => "not ok",
                }
            ),
            log_target,
        )
        .await;
        if !is_repo_ok {
            is_ok = false;
        }
    }

    log(
        match is_ok {
            true => "ok",
            false => "not ok",
        },
        log_target,
    )
    .await;
    is_ok
}
