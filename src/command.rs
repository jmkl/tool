use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Result, Write};
use std::str::FromStr;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CmdInfo {
    pub name: String,
    pub desc: String,
    pub command: MenuCommand,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub exe_path: String,
    #[serde(default)]
    pub work_dir: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MenuCommand {
    ComfyRun,
    ComfyUpdate,
    ComfyKill,
    CronRun,
    CronKill,
    Config,
    About,
    Exit,
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
            "Config" => Ok(Self::Config),
            "About" => Ok(Self::About),
            "Exit" => Ok(Self::Exit),
            _ => Err(()),
        }
    }
}

const FILEPATH: &str = "tool.json";

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub fps: f32,
    pub limit: usize,
    pub commands: Vec<CmdInfo>,
}

impl Config {
    pub fn new() -> Self {
        let config = match fs::exists(FILEPATH) {
            Ok(_) => match Self::read() {
                Ok(cnfg) => cnfg,
                Err(_) => Self::write_default(),
            },
            Err(_) => Self::write_default(),
        };
        Self {
            commands: config.commands,
            fps: config.fps,
            limit: config.limit,
        }
    }
    pub fn _to_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "Failed serializing...".to_string())
    }
    pub fn read() -> Result<Config> {
        let file = File::open(FILEPATH)?;
        let json: Config = serde_json::from_reader(file)?;
        Ok(json)
    }
    pub fn write_default() -> Config {
        let cmds = r#"
{
  "fps": 30.0,
  "limit": 20,
  "commands": [
    {
      "name": "run",
      "desc": "run comfyui server within the comfyui folder\nC:/Comfyui-2024\n",
      "exe_path": "C:/Users/jmkl/.conda/envs/comfyui/python.exe",
      "work_dir": "C:/Comfyui-2024",
      "command": "ComfyRun",
      "args": [
        "main.py",
        "--enable-cors-header",
        "--lowvram",
        "--preview-method",
        "auto",
        "--front-end-version",
        "Comfy-Org/ComfyUI_frontend@latest"
      ]
    },
    {
      "name": "update",
      "desc": "update comfyui",
      "exe_path": "git",
      "work_dir": "C:/Comfyui-2024",
      "command": "ComfyUpdate",
      "args": [
        "pull"
      ]
    },
    {
      "name": "kill",
      "desc": "kill process by save PID",
      "command": "ComfyKill"
    },
    {
      "name": "start",
      "desc": "start deno server for the wsm-mandala.vercel.app updating database",
      "exe_path": "deno",
      "command": "CronRun",
      "work_dir": "E:/_CODE/typescript/wisma-doc-service",
      "args": [
        "run",
        "dev"
      ]
    },
    {
      "name": "stop",
      "desc": "stop deno server",
      "command": "CronKill"
    },
    {
      "name": "config",
      "desc": "show the current config",
      "command": "Config"
    },
    {
      "name": "about",
      "desc": "quit the application",
      "command": "About"
    },
    {
      "name": "quit",
      "desc": "quit the application",
      "command": "Exit"
    }
  ]
}
        "#;
        let config = serde_json::from_str::<Config>(&cmds).unwrap();
        let file = File::create(FILEPATH).expect("Cannot create a file");
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &config).expect("Cannot write a file");
        writer.flush().expect("Cannot flush the writer");
        config
    }
}
