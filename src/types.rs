use chrono::{DateTime, Local};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum CommandName {
  Sync,
  Maintain,
  Allocate
}

#[derive(PartialEq, Debug)]
pub enum CommandMessageType {
  StartByManual,
  StartBySchedule,
  ScheduleEnable,
  ScheduleDisable,
}

pub struct CommandArgs {
  pub repo_paths: Vec<PathBuf>,
  pub includes_unchanged: Option<bool>,
  pub suffix: Option<String>,
}

pub struct CommandMessage {
  pub message_type: CommandMessageType,
  pub command_dt: DateTime<Local>,
  pub command_name: CommandName,
  pub command_args: CommandArgs,
}

pub struct CommandLog {
  pub command_name: CommandName,
  pub command_dt: DateTime<Local>,
  pub suffix: Option<String>,
  pub progress: Option<String>,
  pub is_ongoing: bool,
  pub is_ok: Option<bool>
}