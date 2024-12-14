use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Result, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CmdInfo {
    pub name: String,
    pub desc: String,
    pub exe_path: String,
    pub work_dir: String,
    pub args: Vec<String>,
    pub command: MenuCommand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MenuCommand {
    ComfyRun,
    ComfyUpdate,
    ComfyKill,
    CronRun,
    CronKill,
}

impl FromStr for MenuCommand {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "ComfyRun" => Ok(Self::ComfyRun),
            "ComfyUpdate" => Ok(Self::ComfyUpdate),
            "ComfyKill" => Ok(Self::ComfyKill),
            "CronRun" => Ok(Self::CronRun),
            "CronKill" => Ok(Self::CronKill),
            _ => Err(()),
        }
    }
}

const FILEPATH: &str = "tool.json";

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub all: Vec<CmdInfo>,
}

impl Config {
    pub fn new() -> Self {
        if !fs::exists(FILEPATH).unwrap() {}
        Self { all: Vec::new() }
    }
    pub fn read(mut self) -> Result<Config> {
        let file = File::open(FILEPATH)?;
        let json: Vec<CmdInfo> = serde_json::from_reader(file)?;
        self.all = json;
        Ok(self)
    }
    pub fn _write(mut self) -> Result<()> {
        let file = File::create(FILEPATH)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &mut self.all)?;
        writer.flush()?;
        Ok(())
    }
}
