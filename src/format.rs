use chrono::{prelude::*, Duration};
use lazy_static::lazy_static;
use regex::bytes::Regex;
use rev_buf_reader::RevBufReader;
use std::{
    fs::File,
    io::BufRead,
    path::{Path, PathBuf},
};

use crate::types::{CommandLog, CommandName};

static LOG_DT_FORMAT: &str = "%Y-%m-%d-%H%M%S";

fn format_dt(dt: &DateTime<Local>) -> String {
    return format!(
        "{} {}",
        if Local::now().date_naive() == dt.date_naive() {
            String::from("Today")
        } else if Local::now()
            .checked_add_signed(Duration::days(1))
            .unwrap()
            .date_naive()
            == dt.date_naive()
        {
            String::from("Tomorrow")
        } else {
            dt.format("%d/%m/%Y").to_string()
        },
        dt.format("%H:%M").to_string()
    );
}

fn format_is_ok(is_ok: &Option<bool>) -> String {
    match is_ok {
        None => String::from(""),
        Some(true) => String::from(""),
        Some(false) => String::from("*"),
    }
}

pub fn format_repo_path_display(path: &Path) -> String {
    lazy_static! {
        static ref REPO_PATH_IDENTIFIER_REGEX: Regex = Regex::new(r"^\d+\s").unwrap();
    }

    let mut identifier_path = PathBuf::new();
    for path_component in path.components().filter(|component| {
        REPO_PATH_IDENTIFIER_REGEX.is_match(component.as_os_str().to_str().unwrap().as_bytes())
    }) {
        identifier_path.push(path_component.as_os_str());
    }
    return format!("{}", identifier_path.display());
}

pub fn format_repo_path_suffix(path: &Path) -> String {
    return format!(
        "{}",
        path.components()
            .last()
            .unwrap()
            .as_os_str()
            .to_str()
            .unwrap()
            .split_once(|c: char| !c.is_ascii_digit())
            .unwrap()
            .0
    );
}

pub fn format_next_item_text(
    command_name: CommandName,
    is_schedule_enabled: &bool,
    dt: &DateTime<Local>,
) -> String {
    match is_schedule_enabled {
        true => {
            return format!(
                "Next {}, {}",
                match command_name {
                    CommandName::Sync => "Sync",
                    CommandName::Maintain => "Run",
                    _ => "",
                },
                format_dt(&dt)
            )
        }
        false => {
            return format!(
                "No Planned {}",
                String::from(match command_name {
                    CommandName::Sync => "Sync",
                    CommandName::Maintain => "Maintenance",
                    _ => "",
                })
            )
        }
    };
}

pub fn format_latest_submenu_text(command_name: CommandName, log: Option<&CommandLog>) -> String {
    return match log {
        None => format!(
            "No Recorded {}",
            match command_name {
                CommandName::Sync => "Sync",
                CommandName::Maintain => "Maintenance",
                CommandName::Allocate => "Allocation",
            }
        ),
        Some(log) => match log.is_ongoing {
            true => format!(
                "{}{}",
                match command_name {
                    CommandName::Sync => "Syncing",
                    CommandName::Maintain => "Running Maintenance",
                    CommandName::Allocate => "Allocating Files",
                },
                match &log.progress {
                    None => String::from(""),
                    Some(progress) => format!(" – {}", progress.to_string())
                }
            ),
            false => format!(
                "Latest {}, {}{}",
                match command_name {
                    CommandName::Sync => "Sync",
                    CommandName::Maintain => "Run",
                    CommandName::Allocate => "Allocation",
                },
                format_dt(&log.command_dt),
                format_is_ok(&log.is_ok),
            ),
        },
    };
}

pub fn format_latest_submenu_item_text(log: &CommandLog) -> String {
    return vec![
        format!("{}{}", format_dt(&log.command_dt), format_is_ok(&log.is_ok)),
        match &log.suffix {
            None => String::from(""),
            Some(suffix) => suffix.to_string(),
        },
        match &log.is_ongoing {
            true => String::from("Ongoing"),
            false => String::from(""),
        },
    ]
    .into_iter()
    .filter(|fragment: &String| fragment.len() > 0)
    .collect::<Vec<String>>()
    .join(" – ");
}

pub fn format_sync_status_text(status_repo_ok: &Option<Vec<bool>>) -> String {
    return match status_repo_ok {
        None => String::from("Waiting for Sync"),
        Some(status_repo_ok) => match status_repo_ok
            .iter()
            .into_iter()
            .filter(|b| **b == false)
            .count()
        {
            0 => String::from("Healthy"),
            ct => String::from(format!(
                "Unhealthy, {} {}",
                ct,
                match ct > 1 {
                    true => "issues",
                    false => "issue",
                }
            )),
        },
    };
}

pub fn format_maintain_status_text(status_ok: &bool) -> String {
    return match status_ok {
        true => String::from("Completed Maintenance"),
        false => String::from("Interrupted Maintenance"),
    };
}

pub fn format_schedule_active_text(
    command_name: CommandName,
    is_schedule_enabled: &bool,
) -> String {
    return format!(
        "{} Automatic {}",
        match is_schedule_enabled {
            true => String::from("Pause"),
            false => String::from("Resume"),
        },
        match command_name {
            CommandName::Sync => "Sync",
            CommandName::Maintain => "Maintenance",
            _ => "",
        },
    );
}

pub fn format_command_log_path(
    config_dir_path: &PathBuf,
    command_name: CommandName,
    dt: &DateTime<Local>,
    suffix: &Option<String>,
) -> PathBuf {
    config_dir_path.join(format!(
        "{}/{}{}{}.log",
        match command_name {
            CommandName::Sync => "sync",
            CommandName::Maintain => "maintain",
            CommandName::Allocate => "allocate",
        },
        match command_name {
            CommandName::Sync => "sync-",
            CommandName::Maintain => "maintain-",
            CommandName::Allocate => "allocate-",
        },
        dt.format(LOG_DT_FORMAT).to_string(),
        match suffix {
            Some(suffix) => format!("-{}", suffix),
            None => String::from(""),
        }
    ))
}

pub fn parse_command_log_path(log_path: &PathBuf) -> CommandLog {
    let log_name = log_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix(".log")
        .unwrap();
    let log_segments: Vec<&str> = log_name.split("-").collect();

    fn is_ok(log_path: &PathBuf) -> Option<bool> {
        let buf = RevBufReader::new(File::open(log_path).unwrap());
        match &buf.lines().next().unwrap().unwrap()[..] {
            "not ok" => Some(false),
            "ok" => Some(true),
            _ => None,
        }
    }

    return CommandLog {
        command_name: match log_segments[0] {
            "sync" => CommandName::Sync,
            "maintain" => CommandName::Maintain,
            "allocate" => CommandName::Allocate,
            _ => CommandName::Sync,
        },
        command_dt: NaiveDateTime::parse_from_str(&log_segments[1..5].join("-"), LOG_DT_FORMAT)
            .unwrap()
            .and_local_timezone(chrono::offset::Local)
            .unwrap(),
        suffix: if log_segments.len() == 5 {
            None
        } else {
            Some(String::from(log_segments[5]))
        },
        progress: None,
        is_ongoing: false,
        is_ok: is_ok(log_path),
    };
}
