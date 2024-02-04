use filetime::FileTime;
use glob::glob;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::from_utf8;
use std::time::SystemTime;
use tokio::process::Command;
use walkdir::WalkDir;

use super::{command_output_logfile, log, test_available_remotes, LogTarget};

async fn make_embedded_git_copies(search_path: &PathBuf, log_target: &mut LogTarget<'_>) {
    const COPY_BASE_PATH: &str = "Copies";

    log(
        &format!("make-embedded-git-copies {}", search_path.display()),
        log_target,
    )
    .await;

    for entry in glob(
        Path::new(search_path)
            .join(Path::new("**/.git"))
            .to_str()
            .unwrap(),
    )
    .expect("unable to read glob pattern")
    {
        match entry {
            Ok(master_path) => {
                if master_path.display().to_string()
                    == Path::new(search_path).join(".git").display().to_string()
                {
                    continue;
                }
                let repository_name = &master_path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap();
                let copy_name = format!("{}.git", repository_name);

                let copy_path =
                    &master_path.join(&format!("../../{}/{}", COPY_BASE_PATH, copy_name));

                let mut copy_prev_mtime: Option<SystemTime> = None;
                match copy_path.exists() {
                    true => {
                        copy_prev_mtime = Some(copy_path.metadata().unwrap().modified().unwrap());
                        log(
                            &format!(
                                "ok {}/ (mtime: {})",
                                copy_name,
                                copy_prev_mtime
                                    .unwrap()
                                    .duration_since(SystemTime::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                            ),
                            log_target,
                        )
                        .await;
                    }
                    false => {
                        match fs::create_dir_all(copy_path) {
                            Ok(()) => log(&format!("mkdir {}/", copy_name), log_target).await,
                            Err(e) => {
                                log(&format!("error {} (mkdir, {:?})", copy_name, e), log_target)
                                    .await;
                                continue;
                            }
                        };
                    }
                }

                let mut copy_unprocessed_entry_relpaths: Vec<PathBuf> = vec![];
                for entry in WalkDir::new(&copy_path) {
                    let direntry = &entry.unwrap();
                    let entry_relpath = direntry
                        .path()
                        .strip_prefix(&copy_path)
                        .unwrap()
                        .to_path_buf();
                    if !&entry_relpath.as_os_str().is_empty() {
                        copy_unprocessed_entry_relpaths.push(entry_relpath.to_path_buf());
                    }
                }

                for entry in WalkDir::new(&master_path) {
                    let direntry = &entry.unwrap();
                    let entry_relpath = direntry
                        .path()
                        .strip_prefix(&master_path)
                        .unwrap()
                        .to_path_buf();

                    if !entry_relpath.as_os_str().is_empty() {
                        let entry_relpath_display =
                            format!("{}/{}", copy_name, &entry_relpath.as_path().display());
                        let master_entry_path: PathBuf =
                            Path::new(&master_path).join(&entry_relpath);
                        let copy_entry_path: PathBuf = Path::new(&copy_path).join(&entry_relpath);

                        match direntry.metadata().unwrap().is_dir() {
                            true => {
                                match copy_unprocessed_entry_relpaths.contains(&entry_relpath) {
                                    true => {
                                        match copy_entry_path.exists() {
                                            true => {}
                                            false => match fs::create_dir_all(copy_entry_path) {
                                                Ok(()) => {
                                                    log(
                                                        &format!(
                                                            "mkdir {}/",
                                                            entry_relpath_display
                                                        ),
                                                        log_target,
                                                    )
                                                    .await;
                                                }
                                                Err(e) => {
                                                    log(
                                                        &format!(
                                                            "error {} (mkdir, {:?})",
                                                            entry_relpath_display, e
                                                        ),
                                                        log_target,
                                                    )
                                                    .await;
                                                }
                                            },
                                        };
                                        copy_unprocessed_entry_relpaths.retain(|x: &PathBuf| {
                                            x.as_path() != entry_relpath.as_path()
                                        });
                                    }
                                    false => match fs::create_dir_all(copy_entry_path) {
                                        Ok(()) => {
                                            log(
                                                &format!("mkdir {}/", entry_relpath_display),
                                                log_target,
                                            )
                                            .await;
                                        }
                                        Err(e) => {
                                            log(
                                                &format!(
                                                    "error {} (mkdir, {:?})",
                                                    entry_relpath_display, e
                                                ),
                                                log_target,
                                            )
                                            .await;
                                        }
                                    },
                                };
                            }
                            false => {
                                match copy_unprocessed_entry_relpaths.contains(&entry_relpath) {
                                    true => {
                                        let master_mtime = master_entry_path
                                            .metadata()
                                            .unwrap()
                                            .modified()
                                            .unwrap();
                                        match copy_prev_mtime.is_none()
                                            || master_mtime > copy_prev_mtime.unwrap()
                                        {
                                            true => {
                                                match fs::copy(&master_entry_path, &copy_entry_path)
                                                {
                                                    Ok(_) => {
                                                        let mut perms =
                                                            fs::metadata(&copy_entry_path)
                                                                .unwrap()
                                                                .permissions();
                                                        if perms.readonly() {
                                                            perms.set_readonly(false);
                                                            fs::set_permissions(
                                                                &copy_entry_path,
                                                                perms,
                                                            )
                                                            .unwrap();
                                                        }
                                                        log(
                                                            &format!(
                                                                "cp {} (+{})",
                                                                entry_relpath_display,
                                                                master_mtime
                                                                    .duration_since(
                                                                        copy_prev_mtime.unwrap_or(
                                                                            SystemTime::UNIX_EPOCH
                                                                        )
                                                                    )
                                                                    .unwrap()
                                                                    .as_secs()
                                                            ),
                                                            log_target,
                                                        )
                                                        .await;
                                                    }
                                                    Err(e) => {
                                                        log(
                                                            &format!(
                                                                "error {} (cp, {:?})",
                                                                entry_relpath_display, e
                                                            ),
                                                            log_target,
                                                        )
                                                        .await;
                                                    }
                                                };
                                            }
                                            false => {}
                                        }
                                        copy_unprocessed_entry_relpaths.retain(|x: &PathBuf| {
                                            x.as_path() != entry_relpath.as_path()
                                        });
                                    }
                                    false => {
                                        match fs::copy(&master_entry_path, &copy_entry_path) {
                                            Ok(_) => {
                                                let mut perms = fs::metadata(&copy_entry_path)
                                                    .unwrap()
                                                    .permissions();
                                                if perms.readonly() {
                                                    perms.set_readonly(false);
                                                    fs::set_permissions(&copy_entry_path, perms)
                                                        .unwrap();
                                                }
                                                log(
                                                    &format!("cp {}", entry_relpath_display),
                                                    log_target,
                                                )
                                                .await;
                                            }
                                            Err(e) => {
                                                log(
                                                    &format!(
                                                        "error {} (cp, {:?})",
                                                        entry_relpath_display, e
                                                    ),
                                                    log_target,
                                                )
                                                .await;
                                            }
                                        };
                                    }
                                }
                            }
                        };
                    }
                }

                for copy_entry_unprocessed_relpath in copy_unprocessed_entry_relpaths {
                    let copy_entry_path: PathBuf =
                        Path::new(&copy_path).join(&copy_entry_unprocessed_relpath);
                    let entry_relpath_display = format!(
                        "{}/{}",
                        copy_name,
                        &copy_entry_unprocessed_relpath.as_path().display()
                    );

                    match copy_entry_path.is_dir() {
                        true => match fs::remove_dir(copy_entry_path) {
                            Ok(_) => {
                                log(&format!("rmdir {}", entry_relpath_display), log_target).await;
                            }
                            Err(e) => {
                                log(
                                    &format!("error {} (rmdir, {:?})", entry_relpath_display, e),
                                    log_target,
                                )
                                .await;
                            }
                        },
                        false => match fs::remove_file(copy_entry_path) {
                            Ok(_) => {
                                log(&format!("rm {}", entry_relpath_display), log_target).await;
                            }
                            Err(e) => {
                                log(
                                    &format!("error {} (copy, {:?})", entry_relpath_display, e),
                                    log_target,
                                )
                                .await;
                            }
                        },
                    }
                }
                filetime::set_file_mtime(copy_path, FileTime::now()).unwrap();
            }
            Err(e) => {
                log(&format!("{:?}", e), log_target).await;
            }
        }
    }
    log(
        &format!("make-embedded-git-copies {} ok", search_path.display()),
        log_target,
    )
    .await;
}

pub(crate) async fn sync(
    repo_paths: &Vec<PathBuf>,
    includes_all: bool,
    log_target: &mut LogTarget<'_>,
    notify_progress: impl Fn(String),
) -> Result<Vec<bool>, ()> {
    let mut repo_ok: Vec<bool> = vec![];
    for (repo_index, repo_path) in repo_paths.iter().enumerate() {
        notify_progress(format!("{} of {}", repo_index + 1, repo_paths.len()));

        let available_remotes = test_available_remotes(repo_path, log_target).await;
        make_embedded_git_copies(repo_path, log_target).await;

        let unchanged_stdout = &Command::new("git")
            .args(["ls-files", "-z", ":(attr:annex.archiver.unchanged)*"])
            .current_dir(repo_path)
            .output()
            .await
            .expect("unable to get file list")
            .stdout;

        let unchanged_paths: Vec<&str> = Vec::from_iter(
            from_utf8(unchanged_stdout)
                .unwrap()
                .trim()
                .split_terminator("\u{0}"),
        );

        if !includes_all {
            command_output_logfile(
                Command::new("git")
                    .args(
                        [
                            vec!["update-index", "--assume-unchanged"],
                            unchanged_paths.clone(),
                        ]
                        .concat(),
                    )
                    .current_dir(repo_path),
                format!(
                    "git-update-index-assume-unchanged {:?}",
                    repo_path.display()
                ),
                log_target,
            )
            .await;
        }

        repo_ok.push(if available_remotes.is_empty() {
            log(
                &format!("git-annex-assist {:?} not ok", repo_path.display()),
                log_target,
            )
            .await;
            false
        } else {
            command_output_logfile(
                Command::new("git")
                    .args(
                        [
                            vec!["annex", "assist", if includes_all { "--all" } else { "" }]
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
                format!("git-annex-assist {:?}", repo_path.display()),
                log_target,
            )
            .await
        });

        command_output_logfile(
            Command::new("git")
                .args(
                    [
                        vec!["update-index", "--no-assume-unchanged"],
                        unchanged_paths,
                    ]
                    .concat(),
                )
                .current_dir(repo_path),
            format!(
                "git-update-index-no-assume-unchanged {:?}",
                repo_path.display()
            ),
            log_target,
        )
        .await;
    }
    log(
        match repo_ok.contains(&false) {
            true => "not ok",
            false => "ok",
        },
        log_target,
    )
    .await;
    Ok(repo_ok)
}
