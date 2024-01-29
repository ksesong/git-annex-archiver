use async_cron_scheduler::{cron, Job, Scheduler};
use chrono::{prelude::*, Duration};
use cron::Schedule;
use glob::glob;
use home::home_dir;
use rand::Rng;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::fs::File;
use tokio::process::Command;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tray_icon::{
    menu::{
        accelerator::{Accelerator, Code, Modifiers},
        Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    },
    TrayIconBuilder,
};

#[cfg(target_os = "macos")]
use tao::platform::macos::ActivationPolicy;
#[cfg(target_os = "macos")]
use tao::platform::macos::EventLoopExtMacOS;

use crate::commands::maintain::maintain;
use crate::commands::sync::sync;
use crate::commands::LogTarget;
use crate::format::{
    format_command_log_path, format_last_submenu_item_text, format_last_submenu_text,
    format_maintain_status_text, format_next_item_text, format_repo_path_display,
    format_repo_path_suffix, format_schedule_active_text, format_sync_status_text,
    parse_command_log_path,
};
use crate::types::{CommandArgs, CommandLog, CommandMessage, CommandMessageType, CommandName};

const BASE_ICON_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/icons/icon-base.png"
));
const ACTIVE_ICON_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/icons/icon-active.png"
));
const ERROR_ICON_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/icons/icon-error.png"
));

fn load_icon(buffer: &[u8]) -> tray_icon::Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(buffer)
            .expect("unable to open icon")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    tray_icon::Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("unable to open icon")
}

pub(crate) async fn run_daemon() {
    static LOG_MAX_CT: usize = 4;

    let rng = &mut rand::thread_rng();

    #[derive(Deserialize, Debug)]
    struct Config {
        repo_paths: Vec<String>,
        maintain_timeout_m: Option<u64>,
        maintain_schedule: Option<String>,
        sync_schedule: Option<String>,
        sync_unchanged_schedule: Option<String>,
    }

    let config_dir_path = home_dir()
        .expect("unable to find home dir")
        .join(".config/git-annex/archiver");
    fs::create_dir_all(config_dir_path.join("sync"))
        .expect("unable to create config sync directory");
    fs::create_dir_all(config_dir_path.join("maintain"))
        .expect("unable to create config maintain directory");

    let config: Config = toml::from_str(
        &fs::read_to_string(config_dir_path.join("config")).expect("unable to read config"),
    )
    .expect("unable to parse config");

    let config_repo_paths: Vec<PathBuf> =
        config.repo_paths.iter().map(|s| PathBuf::from(s)).collect();
    let config_sync_schedule = config
        .sync_schedule
        .unwrap_or(format!("0 {} * * * * *", rng.gen_range(0..59)));
    let config_sync_unchanged_schedule = config
        .sync_unchanged_schedule
        .unwrap_or(format!("0 {} * 1,15 * * *", rng.gen_range(0..59)));
    let config_maintain_timeout_m = config.maintain_timeout_m.unwrap_or(120);
    let config_maintain_schedule = config
        .maintain_schedule
        .unwrap_or(format!("0 {} 4 * * * *", rng.gen_range(0..59)));

    let repo_paths: Vec<PathBuf> = config_repo_paths;
    let sync_schedule = Schedule::from_str(&config_sync_schedule)
        .expect("unabled to parse sync schedule, cron format");
    let sync_unchanged_schedule = Schedule::from_str(&config_sync_unchanged_schedule)
        .expect("unabled to parse sync unchanged schedule, cron format");
    let maintain_schedule = Schedule::from_str(&config_maintain_schedule)
        .expect("unabled to parse maintain schedule, cron format");
    let maintain_timeout_m = config_maintain_timeout_m;

    let mut sync_logs: Vec<CommandLog> = vec![];
    let mut maintain_logs: Vec<CommandLog> = vec![];
    for (pattern, logs) in vec![
        ("sync/sync-*.log", &mut sync_logs),
        ("maintain/maintain-*.log", &mut maintain_logs),
    ] {
        for log_path in glob(&config_dir_path.join(pattern).as_os_str().to_str().unwrap())
            .expect("unable to read glob pattern")
            .filter_map(Result::ok)
        {
            let _ = &logs.insert(0, parse_command_log_path(&log_path));
        }
    }
    let mut sync_schedule_is_enabled = true;
    let mut maintain_schedule_is_enabled = true;

    let mut sync_next_dt: DateTime<Local> = sync_schedule.upcoming(Local).next().unwrap();
    let mut maintain_next_dt: DateTime<Local> = maintain_schedule.upcoming(Local).next().unwrap();

    let base_icon = load_icon(BASE_ICON_IMAGE);
    let active_icon = load_icon(ACTIVE_ICON_IMAGE);
    let error_icon = load_icon(ERROR_ICON_IMAGE);

    #[cfg(not(target_os = "macos"))]
    let event_loop = EventLoopBuilder::<CustomEvent>::with_user_event().build();

    #[cfg(target_os = "macos")]
    let mut event_loop = EventLoopBuilder::<CustomEvent>::with_user_event().build();

    #[cfg(target_os = "macos")]
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    let tray_menu: Menu = Menu::new();

    let quit_i = MenuItem::new(
        "Quit",
        true,
        Some(Accelerator::new(Some(Modifiers::META), Code::KeyQ)),
    );

    let sync_status_i = MenuItem::new(format_sync_status_text(&None), false, None);
    let sync_last_i = Submenu::new(
        format_last_submenu_text(
            CommandName::Sync,
            if sync_logs.len() > 0 {
                Some(&sync_logs[0])
            } else {
                None
            },
        ),
        sync_logs.len() > 0,
    );
    for log in &sync_logs {
        sync_last_i
            .append(&MenuItem::new(
                format_last_submenu_item_text(log),
                true,
                None,
            ))
            .unwrap();
    }

    let sync_each_i = Submenu::new("Sync", repo_paths.len() > 0);
    for repo_path in &repo_paths {
        let _ = &sync_each_i.append(&MenuItem::new(
            format_repo_path_display(&repo_path),
            true,
            None,
        ));
    }
    let sync_all_i = MenuItem::new("Sync All", true, None);
    let sync_schedule_toggle_i = MenuItem::new(
        format_schedule_active_text(CommandName::Sync, &sync_schedule_is_enabled),
        true,
        None,
    );

    let sync_next_i: Submenu = Submenu::with_items(
        format_next_item_text(CommandName::Sync, &sync_schedule_is_enabled, &sync_next_dt),
        true,
        &[
            &sync_all_i,
            &sync_each_i,
            &PredefinedMenuItem::separator(),
            &sync_schedule_toggle_i,
        ],
    )
    .unwrap();

    let maintain_status_i = MenuItem::new(format_maintain_status_text(&false), false, None);
    let maintain_last_i = Submenu::new(
        format_last_submenu_text(
            CommandName::Maintain,
            if &maintain_logs.len() > &0 {
                Some(&maintain_logs[0])
            } else {
                None
            },
        ),
        maintain_logs.len() > 0,
    );
    for log in &maintain_logs {
        maintain_last_i
            .append(&MenuItem::new(
                format_last_submenu_item_text(log),
                true,
                None,
            ))
            .unwrap();
    }
    let maintain_all_i = MenuItem::new("Run Maintenance", true, None);
    let maintain_schedule_toggle_i = MenuItem::new(
        format_schedule_active_text(CommandName::Maintain, &maintain_schedule_is_enabled),
        true,
        None,
    );
    let maintain_next_i: Submenu = Submenu::with_items(
        format_next_item_text(
            CommandName::Maintain,
            &maintain_schedule_is_enabled,
            &maintain_next_dt,
        ),
        true,
        &[
            &maintain_all_i,
            &PredefinedMenuItem::separator(),
            &maintain_schedule_toggle_i,
        ],
    )
    .unwrap();

    tray_menu
        .append_items(&[
            &sync_status_i,
            &sync_last_i,
            &sync_next_i,
            &PredefinedMenuItem::separator(),
            &maintain_status_i,
            &maintain_last_i,
            &maintain_next_i,
            &PredefinedMenuItem::separator(),
            &quit_i,
        ])
        .unwrap();

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip(env!("CARGO_PKG_NAME"))
        .with_icon(base_icon.clone())
        .with_icon_as_template(true)
        .build()
        .unwrap();

    let menu_channel = MenuEvent::receiver();

    #[derive(Debug, Clone)]
    enum CustomEvent {
        ScheduledSyncTriggered {
            command_next_dt: DateTime<Local>,
        },
        ScheduledMaintainTriggered {
            command_next_dt: DateTime<Local>,
        },
        SyncStarted {
            command_dt: DateTime<Local>,
            suffix: Option<String>,
        },
        SyncEnded {
            is_ok: Vec<bool>,
        },
        MaintainStarted {
            command_dt: DateTime<Local>,
        },
        MaintainEnded {
            is_ok: bool,
        },
        DayChanged,
    }

    let (sync_command_tx, mut sync_command_rx): (Sender<CommandMessage>, Receiver<CommandMessage>) =
        mpsc::channel(1);
    let (maintain_command_tx, mut maintain_command_rx): (
        Sender<CommandMessage>,
        Receiver<CommandMessage>,
    ) = mpsc::channel(1);

    let spawn_sync_config_dir_path = config_dir_path.clone();
    let spawn_sync_event_loop_proxy: tao::event_loop::EventLoopProxy<CustomEvent> =
        event_loop.create_proxy();
    tokio::spawn(async move {
        let mut is_schedule_enabled = true;
        let mut prev_ended_dt: Option<DateTime<Local>> = None;
        while let Some(command_message) = sync_command_rx.recv().await {
            if command_message.message_type == CommandMessageType::ScheduleDisable {
                is_schedule_enabled = false;
            } else if command_message.message_type == CommandMessageType::ScheduleEnable {
                is_schedule_enabled = true;
            } else {
                let command_dt = command_message.command_dt;
                if command_message.message_type == CommandMessageType::StartBySchedule {
                    if !is_schedule_enabled {
                        continue;
                    } else if prev_ended_dt.is_some() && command_dt < prev_ended_dt.unwrap() {
                        continue;
                    }
                }
                spawn_sync_event_loop_proxy
                    .send_event(CustomEvent::SyncStarted {
                        command_dt,
                        suffix: command_message.command_args.suffix.clone(),
                    })
                    .ok();

                let mut logfile = File::create(&format_command_log_path(
                    &spawn_sync_config_dir_path,
                    CommandName::Sync,
                    &command_dt,
                    &command_message.command_args.suffix,
                ))
                .await
                .expect("unable to create sync log");
                let is_ok = sync(
                    &command_message.command_args.repo_paths,
                    command_message.command_args.includes_unchanged.unwrap(),
                    &mut LogTarget::File(&mut logfile),
                )
                .await
                .unwrap();
                spawn_sync_event_loop_proxy
                    .send_event(CustomEvent::SyncEnded { is_ok })
                    .ok();
                prev_ended_dt = Some(Local::now());
            }
        }
    });

    let spawn_maintain_config_dir_path = config_dir_path.clone();
    let spawn_maintain_event_loop_proxy: tao::event_loop::EventLoopProxy<CustomEvent> =
        event_loop.create_proxy();
    tokio::spawn(async move {
        let mut is_schedule_enabled = true;
        let mut prev_ended_dt: Option<DateTime<Local>> = None;
        while let Some(command_message) = maintain_command_rx.recv().await {
            if command_message.message_type == CommandMessageType::ScheduleDisable {
                is_schedule_enabled = false;
            } else if command_message.message_type == CommandMessageType::ScheduleEnable {
                is_schedule_enabled = true;
            } else {
                let command_dt = command_message.command_dt;
                if command_message.message_type == CommandMessageType::StartBySchedule {
                    if !is_schedule_enabled {
                        continue;
                    } else if prev_ended_dt.is_some() && command_dt < prev_ended_dt.unwrap() {
                        continue;
                    }
                }
                spawn_maintain_event_loop_proxy
                    .send_event(CustomEvent::MaintainStarted { command_dt })
                    .ok();

                let mut logfile = File::create(&format_command_log_path(
                    &spawn_maintain_config_dir_path,
                    CommandName::Maintain,
                    &command_dt,
                    &None,
                ))
                .await
                .expect("unable to create sync log");
                let mut logfile_sync: File = logfile.try_clone().await.unwrap();

                let is_ok = maintain(
                    &command_message.command_args.repo_paths,
                    maintain_timeout_m,
                    (
                        &mut LogTarget::File(&mut logfile),
                        &mut LogTarget::File(&mut logfile_sync),
                    ),
                )
                .await
                .unwrap();
                spawn_maintain_event_loop_proxy
                    .send_event(CustomEvent::MaintainEnded { is_ok })
                    .ok();
                prev_ended_dt = Some(Local::now());
            }
        }
    });

    let (mut scheduler, scheduler_service) = Scheduler::<Local>::launch(tokio::time::sleep);

    let scheduler_sync_job = Job::cron_schedule(sync_schedule.clone());
    let scheduler_sync_schedule = sync_schedule.clone();
    let scheduler_sync_unchanged_schedule = sync_unchanged_schedule.clone();
    let scheduler_sync_command_tx = sync_command_tx.clone();
    let scheduler_sync_repo_paths: Vec<PathBuf> = repo_paths.clone();
    let scheduler_sync_event_loop_proxy: tao::event_loop::EventLoopProxy<CustomEvent> =
        event_loop.create_proxy();
    scheduler
        .insert(scheduler_sync_job, move |_id| {
            let scheduler_sync_command_tx: Sender<CommandMessage> =
                scheduler_sync_command_tx.clone();
            let scheduler_sync_repo_paths = scheduler_sync_repo_paths.clone();
            let scheduler_sync_event_loop_proxy = scheduler_sync_event_loop_proxy.clone();
            let scheduler_sync_schedule = scheduler_sync_schedule.clone();
            let scheduler_sync_unchanged_schedule = scheduler_sync_unchanged_schedule.clone();

            tokio::spawn(async move {
                let command_dt = Local::now();
                let command_next_dt = scheduler_sync_schedule
                    .after(
                        &command_dt
                            .clone()
                            .checked_add_signed(Duration::minutes(1))
                            .unwrap(),
                    )
                    .next()
                    .unwrap();
                let includes_unchanged = scheduler_sync_unchanged_schedule
                    .upcoming(Local)
                    .nth(0)
                    .unwrap()
                    < command_next_dt;
                scheduler_sync_event_loop_proxy
                    .send_event(CustomEvent::ScheduledSyncTriggered { command_next_dt })
                    .ok();

                scheduler_sync_command_tx
                    .send(CommandMessage {
                        message_type: CommandMessageType::StartBySchedule,
                        command_dt,
                        command_name: CommandName::Sync,
                        command_args: CommandArgs {
                            repo_paths: scheduler_sync_repo_paths,
                            includes_unchanged: Some(includes_unchanged),
                            suffix: match includes_unchanged {
                                true => Some(String::from("*")),
                                false => None,
                            },
                        },
                    })
                    .await
                    .unwrap();
            });
        })
        .await;

    let scheduler_maintain_job = Job::cron_schedule(maintain_schedule.clone());
    let scheduler_maintain_schedule = maintain_schedule.clone();
    let scheduler_maintain_command_tx = maintain_command_tx.clone();
    let scheduler_maintain_repo_paths: Vec<PathBuf> = repo_paths.clone();
    let scheduler_maintain_event_loop_proxy: tao::event_loop::EventLoopProxy<CustomEvent> =
        event_loop.create_proxy();
    scheduler
        .insert(scheduler_maintain_job, move |_id| {
            let scheduler_maintain_command_tx = scheduler_maintain_command_tx.clone();
            let scheduler_maintain_repo_paths: Vec<PathBuf> = scheduler_maintain_repo_paths.clone();
            let scheduler_maintain_event_loop_proxy = scheduler_maintain_event_loop_proxy.clone();
            let scheduler_maintain_schedule = scheduler_maintain_schedule.clone();
            tokio::spawn(async move {
                let command_dt = Local::now();
                let command_next_dt = scheduler_maintain_schedule
                    .after(
                        &command_dt
                            .clone()
                            .checked_add_signed(Duration::minutes(1))
                            .unwrap(),
                    )
                    .next()
                    .unwrap();
                scheduler_maintain_event_loop_proxy
                    .send_event(CustomEvent::ScheduledMaintainTriggered { command_next_dt })
                    .ok();

                scheduler_maintain_command_tx
                    .send(CommandMessage {
                        message_type: CommandMessageType::StartBySchedule,
                        command_dt,
                        command_name: CommandName::Maintain,
                        command_args: CommandArgs {
                            repo_paths: scheduler_maintain_repo_paths,
                            includes_unchanged: None,
                            suffix: None,
                        },
                    })
                    .await
                    .unwrap();
            });
        })
        .await;

    let event_loop_day_proxy: tao::event_loop::EventLoopProxy<CustomEvent> =
        event_loop.create_proxy();
    let scheduler_day_job = Job::cron("0 0 0 * * *").unwrap();
    scheduler
        .insert(scheduler_day_job, move |_id| {
            let event_loop_day_proxy = event_loop_day_proxy.clone();
            event_loop_day_proxy
                .send_event(CustomEvent::DayChanged)
                .ok();
        })
        .await;

    tokio::spawn(scheduler_service);

    let event_repo_paths: Vec<PathBuf> = repo_paths.clone();
    let event_sync_command_tx = sync_command_tx.clone();
    let event_maintain_command_tx = maintain_command_tx.clone();
    let event_base_icon = base_icon.clone();
    let event_active_icon = active_icon.clone();
    let event_error_icon = error_icon.clone();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        match event {
            Event::UserEvent(CustomEvent::ScheduledSyncTriggered { command_next_dt }) => {
                sync_next_dt = command_next_dt;
                sync_next_i.set_text(format_next_item_text(
                    CommandName::Sync,
                    &sync_schedule_is_enabled,
                    &sync_next_dt,
                ));
            }
            Event::UserEvent(CustomEvent::SyncStarted { command_dt, suffix }) => {
                tray_icon.set_icon(Some(event_active_icon.clone())).unwrap();
                tray_icon.set_icon_as_template(true);
                sync_each_i.set_enabled(false);
                sync_all_i.set_enabled(false);

                sync_logs.insert(
                    0,
                    CommandLog {
                        command_dt,
                        suffix,
                        is_ongoing: true,
                    },
                );
                if sync_logs.len() > LOG_MAX_CT {
                    let deleted_log = sync_logs.get(LOG_MAX_CT).unwrap();
                    fs::remove_file(format_command_log_path(
                        &config_dir_path,
                        CommandName::Sync,
                        &deleted_log.command_dt,
                        &deleted_log.suffix,
                    ))
                    .expect("unable to remove log");
                    sync_logs.remove(LOG_MAX_CT);
                }
                sync_last_i.set_text(format_last_submenu_text(
                    CommandName::Sync,
                    Some(&sync_logs[0]),
                ));
                if sync_last_i.items().len() < LOG_MAX_CT {
                    sync_last_i
                        .prepend(&MenuItem::new(
                            format_last_submenu_item_text(&sync_logs[0]),
                            true,
                            None,
                        ))
                        .unwrap();
                } else {
                    for (item_index, _item) in sync_last_i.items().iter().enumerate() {
                        _item
                            .as_menuitem()
                            .unwrap()
                            .set_text(format_last_submenu_item_text(&sync_logs[item_index]))
                    }
                }
                sync_last_i.set_enabled(true);
            }
            Event::UserEvent(CustomEvent::SyncEnded { is_ok }) => {
                sync_logs[0].is_ongoing = false;
                sync_each_i.set_enabled(true);
                sync_all_i.set_enabled(true);
                sync_last_i.set_text(format_last_submenu_text(
                    CommandName::Sync,
                    Some(&sync_logs[0]),
                ));
                if maintain_all_i.is_enabled() {
                    let is_unhealthy = is_ok.contains(&false);

                    if is_unhealthy {
                        tray_icon.set_icon(Some(event_error_icon.clone())).unwrap();
                        tray_icon.set_icon_as_template(true);
                    } else {
                        tray_icon.set_icon(Some(event_base_icon.clone())).unwrap();
                        tray_icon.set_icon_as_template(true);
                    }
                }

                sync_last_i
                    .items()
                    .first()
                    .unwrap()
                    .as_menuitem()
                    .unwrap()
                    .set_text(format_last_submenu_item_text(&sync_logs[0]));

                sync_status_i.set_text(format_sync_status_text(&Some(is_ok)));
            }
            Event::UserEvent(CustomEvent::ScheduledMaintainTriggered { command_next_dt }) => {
                maintain_next_dt = command_next_dt;
                maintain_next_i.set_text(format_next_item_text(
                    CommandName::Maintain,
                    &maintain_schedule_is_enabled,
                    &maintain_next_dt,
                ));
            }
            Event::UserEvent(CustomEvent::MaintainStarted { command_dt }) => {
                tray_icon.set_icon(Some(event_active_icon.clone())).unwrap();
                tray_icon.set_icon_as_template(true);

                maintain_all_i.set_enabled(false);
                maintain_logs.insert(
                    0,
                    CommandLog {
                        command_dt,
                        suffix: None,
                        is_ongoing: true,
                    },
                );
                if maintain_logs.len() > LOG_MAX_CT {
                    let deleted_log = maintain_logs.get(LOG_MAX_CT).unwrap();
                    fs::remove_file(format_command_log_path(
                        &config_dir_path,
                        CommandName::Maintain,
                        &deleted_log.command_dt,
                        &deleted_log.suffix,
                    ))
                    .expect("unable to remove log");
                    maintain_logs.remove(LOG_MAX_CT);
                }
                maintain_last_i.set_text(format_last_submenu_text(
                    CommandName::Maintain,
                    Some(&maintain_logs[0]),
                ));
                if maintain_last_i.items().len() < LOG_MAX_CT {
                    maintain_last_i
                        .prepend(&MenuItem::new(
                            format_last_submenu_item_text(&maintain_logs[0]),
                            true,
                            None,
                        ))
                        .unwrap();
                } else {
                    for (item_index, _item) in maintain_last_i.items().iter().enumerate() {
                        _item
                            .as_menuitem()
                            .unwrap()
                            .set_text(format_last_submenu_item_text(&maintain_logs[item_index]))
                    }
                }
                maintain_last_i.set_enabled(true);
            }
            Event::UserEvent(CustomEvent::MaintainEnded { is_ok }) => {
                maintain_logs[0].is_ongoing = false;
                maintain_all_i.set_enabled(true);
                if sync_all_i.is_enabled() {
                    tray_icon.set_icon(Some(event_base_icon.clone())).unwrap();
                    tray_icon.set_icon_as_template(true);
                }

                maintain_last_i.set_text(format_last_submenu_text(
                    CommandName::Maintain,
                    Some(&maintain_logs[0]),
                ));
                maintain_last_i
                    .items()
                    .first()
                    .unwrap()
                    .as_menuitem()
                    .unwrap()
                    .set_text(format_last_submenu_item_text(&maintain_logs[0]));
                maintain_status_i.set_text(format_maintain_status_text(&is_ok));
            }
            Event::UserEvent(CustomEvent::DayChanged) => {
                sync_next_i.set_text(format_next_item_text(
                    CommandName::Sync,
                    &sync_schedule_is_enabled,
                    &sync_next_dt,
                ));
                maintain_next_i.set_text(format_next_item_text(
                    CommandName::Maintain,
                    &maintain_schedule_is_enabled,
                    &maintain_next_dt,
                ));
                sync_last_i.set_text(format_last_submenu_text(
                    CommandName::Sync,
                    if &sync_logs.len() > &0 {
                        Some(&sync_logs[0])
                    } else {
                        None
                    },
                ));
                for (index, last_i) in sync_last_i.items().iter().enumerate() {
                    last_i
                        .as_menuitem()
                        .unwrap()
                        .set_text(format_last_submenu_item_text(&sync_logs[index]));
                }
                maintain_last_i.set_text(format_last_submenu_text(
                    CommandName::Maintain,
                    if &maintain_logs.len() > &0 {
                        Some(&maintain_logs[0])
                    } else {
                        None
                    },
                ));
                for (index, last_i) in maintain_last_i.items().iter().enumerate() {
                    last_i
                        .as_menuitem()
                        .unwrap()
                        .set_text(format_last_submenu_item_text(&maintain_logs[index]));
                }
            }
            _ => (),
        }

        match menu_channel.try_recv() {
            Ok(event) => {
                if event.id == quit_i.id() {
                    *control_flow = ControlFlow::Exit;
                } else if event.id == sync_schedule_toggle_i.id() {
                    sync_schedule_is_enabled = !sync_schedule_is_enabled;
                    sync_schedule_toggle_i.set_text(format_schedule_active_text(
                        CommandName::Sync,
                        &sync_schedule_is_enabled,
                    ));
                    sync_next_i.set_text(format_next_item_text(
                        CommandName::Sync,
                        &sync_schedule_is_enabled,
                        &sync_next_dt,
                    ));

                    let event_sync_command_tx = event_sync_command_tx.clone();
                    tokio::spawn(async move {
                        event_sync_command_tx
                            .send(CommandMessage {
                                message_type: match &sync_schedule_is_enabled {
                                    true => CommandMessageType::ScheduleEnable,
                                    false => CommandMessageType::ScheduleDisable,
                                },
                                command_dt: Local::now(),
                                command_name: CommandName::Sync,
                                command_args: CommandArgs {
                                    repo_paths: vec![],
                                    includes_unchanged: None,
                                    suffix: None,
                                },
                            })
                            .await
                            .unwrap();
                    });
                } else if event.id == maintain_schedule_toggle_i.id() {
                    maintain_schedule_is_enabled = !maintain_schedule_is_enabled;
                    maintain_schedule_toggle_i.set_text(format_schedule_active_text(
                        CommandName::Maintain,
                        &maintain_schedule_is_enabled,
                    ));
                    maintain_next_i.set_text(format_next_item_text(
                        CommandName::Maintain,
                        &maintain_schedule_is_enabled,
                        &maintain_next_dt,
                    ));

                    let event_maintain_command_tx = event_maintain_command_tx.clone();
                    tokio::spawn(async move {
                        event_maintain_command_tx
                            .send(CommandMessage {
                                message_type: match &maintain_schedule_is_enabled {
                                    true => CommandMessageType::ScheduleEnable,
                                    false => CommandMessageType::ScheduleDisable,
                                },
                                command_dt: Local::now(),
                                command_name: CommandName::Maintain,
                                command_args: CommandArgs {
                                    repo_paths: vec![],
                                    includes_unchanged: None,
                                    suffix: None,
                                },
                            })
                            .await
                            .unwrap();
                    });
                } else if event.id == sync_all_i.id() {
                    let event_sync_command_tx = event_sync_command_tx.clone();
                    let event_repo_paths = event_repo_paths.clone();

                    tokio::spawn(async move {
                        event_sync_command_tx
                            .send(CommandMessage {
                                message_type: CommandMessageType::StartByManual,
                                command_dt: Local::now(),
                                command_name: CommandName::Sync,
                                command_args: CommandArgs {
                                    repo_paths: event_repo_paths.clone(),
                                    includes_unchanged: Some(false),
                                    suffix: None,
                                },
                            })
                            .await
                            .unwrap();
                    });
                } else if event.id == maintain_all_i.id() {
                    let event_maintain_command_tx = event_maintain_command_tx.clone();
                    let event_repo_paths = event_repo_paths.clone();

                    tokio::spawn(async move {
                        event_maintain_command_tx
                            .send(CommandMessage {
                                message_type: CommandMessageType::StartByManual,
                                command_dt: Local::now(),
                                command_name: CommandName::Maintain,
                                command_args: CommandArgs {
                                    repo_paths: event_repo_paths.clone(),
                                    includes_unchanged: None,
                                    suffix: None,
                                },
                            })
                            .await
                            .unwrap();
                    });
                } else {
                    for (repo_index, _item) in sync_each_i.items().iter().enumerate() {
                        if event.id == _item.id() {
                            let event_sync_command_tx = event_sync_command_tx.clone();
                            let event_repo_paths = event_repo_paths.clone();

                            tokio::spawn(async move {
                                let repo_path = event_repo_paths.get(repo_index).unwrap();
                                event_sync_command_tx
                                    .send(CommandMessage {
                                        message_type: CommandMessageType::StartByManual,
                                        command_dt: Local::now(),
                                        command_name: CommandName::Sync,
                                        command_args: CommandArgs {
                                            repo_paths: vec![repo_path.to_owned()],
                                            includes_unchanged: Some(false),
                                            suffix: Some(format_repo_path_suffix(repo_path)),
                                        },
                                    })
                                    .await
                                    .unwrap();
                            });
                            return;
                        }
                    }

                    for (command_name, submenu_last, logs) in vec![
                        (CommandName::Sync, &sync_last_i, &sync_logs),
                        (CommandName::Maintain, &maintain_last_i, &maintain_logs),
                    ] {
                        for (log_index, _item) in submenu_last.items().iter().enumerate() {
                            if event.id == _item.id() {
                                let config_dir_path = config_dir_path.clone();
                                let log = &logs[log_index];

                                Command::new("open")
                                    .args([
                                        "/System/Applications/Utilities/Console.app",
                                        &format!(
                                            "{}",
                                            format_command_log_path(
                                                &config_dir_path,
                                                command_name,
                                                &log.command_dt,
                                                &log.suffix,
                                            )
                                            .display()
                                        ),
                                    ])
                                    .spawn()
                                    .unwrap();
                                return;
                            }
                        }
                    }
                }
            }
            _ => (),
        }
    });
}
