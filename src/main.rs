use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use std::env;
use std::io::{stdout, Result};
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task;

use command::{CmdInfo, Config, MenuCommand};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::palette::tailwind::SLATE;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph,
    Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap,
};
use ratatui::DefaultTerminal;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_stream::wrappers::LinesStream;
use tokio_stream::StreamExt;
mod command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Status {
    Idle,
    Run,
}

#[derive(Debug, Default)]
struct LogLists {
    comfyui_pid: Option<u32>,
    cron_pid: Option<u32>,
    comfyui_isrunning: bool,
    cron_isrunning: bool,
    comfyui_logs: Vec<String>,
    cron_logs: Vec<String>,
    debug_logs: Vec<String>,
}

#[derive(Debug, Clone)]
struct MenuInfo {
    cmd: MenuCommand,
    title: String,
    info: String,
    status: Status,
}
impl MenuInfo {
    fn new(cmd: MenuCommand, title: &str, info: &str, status: Status) -> Self {
        Self {
            cmd,
            title: title.to_string(),
            info: info.to_string(),
            status,
        }
    }
}

#[derive(Debug, Clone)]
struct MenuList {
    items: Vec<MenuInfo>,
    state: ListState,
}
impl FromIterator<(MenuCommand, &'static str, &'static str, Status)> for MenuList {
    fn from_iter<T: IntoIterator<Item = (MenuCommand, &'static str, &'static str, Status)>>(
        iter: T,
    ) -> Self {
        let items = iter
            .into_iter()
            .map(|(cmd, title, info, status)| MenuInfo::new(cmd, title, info, status))
            .collect();
        let state = ListState::default();
        Self { items, state }
    }
}

impl From<&MenuInfo> for ListItem<'_> {
    fn from(mn: &MenuInfo) -> Self {
        let line = Line::styled(format!("{}", mn.title), TEXT_FG_COLOR);
        ListItem::new(line)
    }
}
const NORMAL_ROW_BG: Color = SLATE.c950;
const ALT_ROW_BG_COLOR: Color = SLATE.c900;
const SELECTED_STYLE: Style = Style::new().bg(SLATE.c800).add_modifier(Modifier::BOLD);
const TEXT_FG_COLOR: Color = SLATE.c200;
const fn state_color(i: usize) -> Color {
    if i % 2 == 0 {
        NORMAL_ROW_BG
    } else {
        Color::Red
    }
}

enum CommandMode {
    COMFYUI,
    CRON,
}
#[derive(Debug, Clone, PartialEq)]
enum ActivePanel {
    Menu,
    ComfyLog,
    CronLog,
    DebugLog,
}
impl ActivePanel {
    pub fn next(&self) -> Self {
        match self {
            ActivePanel::Menu => ActivePanel::ComfyLog,
            ActivePanel::ComfyLog => ActivePanel::CronLog,
            ActivePanel::CronLog => ActivePanel::Menu,
            ActivePanel::DebugLog => ActivePanel::DebugLog,
        }
    }
}
#[derive(Debug, Clone)]
struct App {
    config: Config,
    selected_config: Option<CmdInfo>,
    should_exit: bool,
    menu_list: MenuList,
    logs: Arc<RwLock<LogLists>>,
    show_debugconsole: bool,
    comfylog_scrollbar_state: ScrollbarState,
    cronlog_scrollbar_state: ScrollbarState,
    comfylog_scroll: usize,
    cronlog_scroll: usize,
    active_panel: ActivePanel,
}

impl App {
    fn new() -> Self {
        let config = Config::new();
        let items = config
            .commands
            .iter()
            .map(|c| MenuInfo::new(c.command.clone(), &c.name, &c.desc, Status::Idle))
            .collect::<Vec<MenuInfo>>();

        Self {
            config,
            should_exit: false,
            selected_config: None,
            logs: Arc::new(RwLock::new(LogLists::default())),

            show_debugconsole: false,
            comfylog_scrollbar_state: ScrollbarState::new(10),
            cronlog_scrollbar_state: ScrollbarState::new(10),
            comfylog_scroll: 0,
            cronlog_scroll: 0,
            active_panel: ActivePanel::Menu,
            menu_list: MenuList {
                items: items,
                state: ListState::default(),
            },
        }
    }

    async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let period = Duration::from_secs_f32(1.0 / self.config.fps);
        let mut interval = tokio::time::interval(period);
        let mut events = EventStream::new();

        while !self.should_exit {
            tokio::select! {
                _ = interval.tick() => {  terminal.draw(|frame| {
                    frame.render_widget(Clear,frame.area());
                    frame.render_widget(&mut self, frame.area())})?;},
                Some(Ok(event)) = events.next() => self.handle_events(&event,&mut terminal).await,
            }
        }

        Ok(())
    }
    fn trim_line(line: String) -> String {
        line.trim_end().to_string()
        // if input.len() > 50 {
        //     let start = &input[..20];
        //     let end = &input[input.len() - 20..];
        //     format!("{}......{}", start, end)
        // } else {
        //     input.to_string()
        // }
    }

    fn run_command(self, mode: CommandMode, menucommand: MenuCommand) {
        let logs = Arc::clone(&self.logs);
        let cmdifo = match &self.selected_config {
            Some(cmd) => cmd.clone(),
            None => panic!("nothing selected"),
        };

        tokio::spawn(async move {
            let mut cmd = Command::new(cmdifo.exe_path)
                .env("PYTHONUNBUFFERED", "1")
                .current_dir(cmdifo.work_dir)
                .args(cmdifo.args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("Failed to execute command");

            let stdout = cmd.stdout.take().unwrap();
            let stderr = cmd.stderr.take().unwrap();

            let stdout = LinesStream::new(BufReader::new(stdout).lines());
            let stderr = LinesStream::new(BufReader::new(stderr).lines());
            let mut merged = StreamExt::merge(stdout, stderr);
            while let Some(line) = merged.next().await {
                let mut logs = logs.write().await;
                let line = match line {
                    Ok(l) => App::trim_line(l),
                    Err(_) => String::from("----deducted---"),
                };

                match menucommand {
                    MenuCommand::ComfyRun => {
                        logs.comfyui_pid = cmd.id();
                    }
                    MenuCommand::CronRun => {
                        logs.cron_pid = cmd.id();
                    }
                    _ => {}
                }
                match mode {
                    CommandMode::COMFYUI => {
                        logs.comfyui_logs.push(line);
                        if logs.comfyui_logs.len() > self.config.limit {
                            logs.comfyui_logs.remove(0);
                        }
                    }
                    CommandMode::CRON => {
                        logs.cron_logs.push(line);
                        if logs.cron_logs.len() > self.config.limit {
                            logs.cron_logs.remove(0);
                        }
                    }
                }
            }
        });
    }

    fn kill_command(pid: u32) -> Result<()> {
        let _ = Command::new("taskkill")
            .args(vec!["/F", "/PID", format!("{}", pid).as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
    fn run_quick_command(&mut self, cmd: String, args: Vec<String>) {
        let logs = Arc::clone(&self.logs);

        tokio::spawn(async move {
            let mut debug = logs.write().await;
            debug.debug_logs.push(args.join(" "));
            let mut cmd = Command::new(cmd)
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("Failed to execute command");

            let stdout = cmd.stdout.take().unwrap();
            let stderr = cmd.stderr.take().unwrap();

            let stdout = LinesStream::new(BufReader::new(stdout).lines());
            let stderr = LinesStream::new(BufReader::new(stderr).lines());
            let mut merged = StreamExt::merge(stdout, stderr);

            while let Some(line) = merged.next().await {
                let line = match line {
                    Ok(l) => App::trim_line(l),
                    Err(_) => String::from("----deducted---"),
                };

                debug.debug_logs.push(line);
            }
            debug.debug_logs.push("Finished...".to_string());
        });
    }

    fn change_status(&mut self, command: MenuCommand, status: Status) {
        if let Some(whichis) = self
            .menu_list
            .items
            .iter_mut()
            .find(|item| item.cmd == command)
        {
            whichis.status = status;
        }
    }

    async fn process_menu(&mut self, index: usize, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut menuinfo = self.menu_list.items[index].clone();
        let logs = Arc::clone(&self.logs);
        let this = self.clone();
        let menucommand = menuinfo.cmd;
        match menucommand {
            MenuCommand::ComfyRun => {
                self.change_status(MenuCommand::ComfyRun, Status::Run);
                tokio::spawn(async move {
                    let mut logs = logs.write().await;
                    menuinfo.status = Status::Run;
                    if !logs.comfyui_isrunning {
                        this.run_command(CommandMode::COMFYUI, MenuCommand::ComfyRun);
                        logs.comfyui_isrunning = true;
                    }
                });
            }
            MenuCommand::ComfyKill => {
                self.change_status(MenuCommand::ComfyRun, Status::Idle);
                tokio::spawn(async move {
                    let mut logs = logs.write().await;
                    if let Some(sender) = logs.comfyui_pid {
                        logs.comfyui_logs.clear();
                        logs.comfyui_logs
                            .push(format!("Sending Exit Signal :{}", sender));
                        match App::kill_command(sender) {
                            Ok(_) => {
                                logs.comfyui_logs.push(format!("Success :{}", sender));
                                logs.comfyui_isrunning = false;
                            }
                            Err(err) => {
                                logs.comfyui_isrunning = false;
                                logs.comfyui_logs
                                    .push(format!("Failed to terminate pid :{}", err));
                            }
                        }
                    }
                });
            }
            MenuCommand::ComfyUpdate => {
                this.run_command(CommandMode::COMFYUI, MenuCommand::ComfyUpdate);
            }
            MenuCommand::CronRun => {
                self.change_status(MenuCommand::CronRun, Status::Run);
                tokio::spawn(async move {
                    let mut logs = logs.write().await;
                    menuinfo.status = Status::Run;
                    if !logs.cron_isrunning {
                        this.run_command(CommandMode::CRON, MenuCommand::CronRun);
                        logs.cron_isrunning = true;
                    }
                });
            }
            MenuCommand::CronKill => {
                self.change_status(MenuCommand::CronRun, Status::Idle);
                tokio::spawn(async move {
                    let mut logs = logs.write().await;
                    if let Some(sender) = logs.cron_pid {
                        logs.cron_logs.clear();
                        logs.cron_logs
                            .push(format!("Sending Exit Signal :{}", sender));
                        match App::kill_command(sender) {
                            Ok(_) => {
                                logs.cron_logs.push(format!("Success :{}", sender));
                                logs.cron_isrunning = false;
                            }
                            Err(err) => {
                                logs.cron_isrunning = false;
                                logs.cron_logs
                                    .push(format!("Failed to terminate pid :{}", err));
                            }
                        }
                    }
                });
            }
            MenuCommand::Config => {
                let _ = self.run_editor(terminal).await;

                //self.debug = self.config.to_string();
            }
            MenuCommand::About => {
                //let _ = self.run_editor(terminal).await;

                //self.debug = self.config.to_string();
            }
            MenuCommand::Exit => {
                self.should_exit = true;
            }
        }
        Ok(())
    }
    fn clear_log_panel(&self) {
        let log = Arc::clone(&self.logs);
        tokio::spawn(async move {
            let mut logs = log.write().await;
            logs.comfyui_logs.clear();
            logs.cron_logs.clear();
            logs.debug_logs.clear();
        });
    }
    async fn run_editor(&self, terminal: &mut DefaultTerminal) -> Result<()> {
        stdout().execute(LeaveAlternateScreen)?;
        disable_raw_mode()?;
        let _ = Command::new("hx").arg("tool.json").status().await;
        stdout().execute(EnterAlternateScreen)?;
        enable_raw_mode()?;
        terminal.clear()?;
        Ok(())
    }
    async fn handle_events(&mut self, event: &Event, terminal: &mut DefaultTerminal) {
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => self.should_exit = true,
                    KeyCode::Char('d') => {
                        self.show_debugconsole = !self.show_debugconsole;
                    }
                    KeyCode::Char('t') => {
                        self.run_quick_command(
                            "pwsh".to_string(),
                            vec![
                                "-NoProfile",
                                "-Command",
                                "tasklist",
                                "|",
                                "rg",
                                "\"deno*|python*\"",
                            ]
                            .iter()
                            .map(|m| m.to_string())
                            .collect(),
                        );
                    }
                    KeyCode::Char('k') => {
                        self.run_quick_command(
                            "pwsh".to_string(),
                            vec![
                                "-NoProfile",
                                "-Command",
                                "taskkill",
                                "/f",
                                "/im",
                                "deno*",
                                "&&",
                                "taskkill",
                                "/f",
                                "/im",
                                "python*",
                            ]
                            .iter()
                            .map(|m| m.to_string())
                            .collect(),
                        );
                    }
                    KeyCode::Down => match self.active_panel {
                        ActivePanel::Menu => {
                            self.menu_list.state.select_next();
                        }
                        ActivePanel::ComfyLog => {
                            self.comfylog_scroll = self.comfylog_scroll.saturating_add(1);
                            self.comfylog_scrollbar_state =
                                self.comfylog_scrollbar_state.position(self.comfylog_scroll);
                        }
                        ActivePanel::CronLog => {
                            self.cronlog_scroll = self.cronlog_scroll.saturating_add(1);
                            self.cronlog_scrollbar_state =
                                self.cronlog_scrollbar_state.position(self.cronlog_scroll);
                        }
                        ActivePanel::DebugLog => {}
                    },
                    KeyCode::Up => match self.active_panel {
                        ActivePanel::Menu => {
                            self.menu_list.state.select_previous();
                        }
                        ActivePanel::ComfyLog => {
                            self.comfylog_scroll = self.comfylog_scroll.saturating_sub(1);
                            self.comfylog_scrollbar_state =
                                self.comfylog_scrollbar_state.position(self.comfylog_scroll);
                        }
                        ActivePanel::CronLog => {
                            self.cronlog_scroll = self.cronlog_scroll.saturating_sub(1);
                            self.cronlog_scrollbar_state =
                                self.cronlog_scrollbar_state.position(self.cronlog_scroll);
                        }
                        ActivePanel::DebugLog => {}
                    },
                    KeyCode::Enter => {
                        if let Some(i) = self.menu_list.state.selected() {
                            self.selected_config = Some(self.config.commands[i].clone());
                            let _ = self.process_menu(i, terminal).await;
                        }
                    }
                    KeyCode::Tab => {
                        self.active_panel = self.active_panel.next();
                    }

                    KeyCode::Char('c') => {
                        self.clear_log_panel();
                    }

                    _ => {}
                }
            }
        }
    }

    fn set_title<'a>(&self, title: &'a str, active_panel: ActivePanel) -> Block<'a> {
        if self.active_panel == active_panel {
            Block::new()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::new().red().bold())
        } else {
            Block::new().title(title).borders(Borders::ALL)
        }
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer) {
        let block = self.set_title("Menu", ActivePanel::Menu);
        let menu: Vec<ListItem> = self
            .menu_list
            .items
            .iter()
            .enumerate()
            .map(|(i, menu)| {
                let color = state_color(i);
                match menu.status {
                    Status::Idle => ListItem::from(menu).fg(color),
                    Status::Run => ListItem::from(menu).fg(color).bg(Color::Red),
                }
            })
            .collect();
        let list = List::new(menu)
            .block(block)
            .highlight_style(SELECTED_STYLE)
            .highlight_symbol("✓ ")
            .highlight_spacing(HighlightSpacing::Always);

        StatefulWidget::render(list, area, buf, &mut self.menu_list.state);
    }
    fn about(&self) -> String {
        format!(
            r#"
░░░░░░░░░░░░░░░░░
░███░███░███░█░░░
░░█░░█░█░█░█░█░░░
░░█░░███░███░███░
░░░░░░░░░░░░░░░░░
version  :{}"#,
            env!("CARGO_PKG_VERSION")
        )
    }
    fn render_selected_menu(&self, area: Rect, buf: &mut Buffer) {
        let info = if let Some(i) = self.menu_list.state.selected() {
            if self.menu_list.items[i].cmd == MenuCommand::About {
                self.about()
            } else {
                format!("{}", self.menu_list.items[i].info)
            }
        } else {
            self.about()
        };
        // We show the list item's info under the list in this paragraph
        let block = Block::new()
            .title("Info")
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1));

        // We can now render the item info
        Paragraph::new(info)
            .block(block)
            .fg(Color::Red)
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }
}
impl Widget for &mut App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let parent =
            Layout::vertical([Constraint::Percentage(100), Constraint::Min(1)]).split(area);
        let horizontal =
            Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)]);
        let [left, right] = horizontal.areas(parent[0]);
        let lefts =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(left);
        let rights =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(right);

        //Render Menus
        self.render_list(lefts[0], buf);
        //Render menu info
        self.render_selected_menu(lefts[1], buf);
        let logs = task::block_in_place(|| {
            let logs = tokio::runtime::Handle::current().block_on(self.logs.read());
            logs
        });
        let comfylog_len = logs.comfyui_logs.len();
        let cronlog_len = logs.cron_logs.len();

        //Render ComfyLog Log
        let comfyui_log_lines = logs
            .comfyui_logs
            .iter()
            .map(|s| Line::from(Span::raw(s.clone())))
            .collect::<Vec<_>>();
        let comfyuilog_paragraph = Paragraph::new(comfyui_log_lines)
            .block(self.set_title("ComfyUI Logs", ActivePanel::ComfyLog))
            .scroll((self.comfylog_scroll as u16, 0))
            .wrap(Wrap { trim: true })
            .left_aligned();

        //Render Cron Log
        let cron_log_lines = logs
            .cron_logs
            .iter()
            .map(|s| Line::from(Span::raw(s.clone())))
            .collect::<Vec<_>>();
        let cronuilog_paragraph = Paragraph::new(cron_log_lines)
            .block(self.set_title("Cron Logs", ActivePanel::CronLog))
            .scroll((self.cronlog_scroll as u16, 0))
            .wrap(Wrap { trim: true })
            .left_aligned();

        //Scrollbar for ComfyUI Logs
        self.comfylog_scrollbar_state = self.comfylog_scrollbar_state.content_length(comfylog_len);
        let scrl_top = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        //Scrollbar for Cron Logs
        self.cronlog_scrollbar_state = self.cronlog_scrollbar_state.content_length(cronlog_len);
        let scrl_bottom = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        if self.show_debugconsole {
            let debug = Paragraph::new(
                logs.debug_logs
                    .iter()
                    .map(|s| Line::from(Span::raw(s.clone())))
                    .collect::<Vec<_>>(),
            )
            .block(self.set_title("Debug Console", ActivePanel::DebugLog))
            .wrap(Wrap { trim: true })
            .left_aligned();
            debug.render(right, buf);
        } else {
            comfyuilog_paragraph.render(rights[0], buf);
            cronuilog_paragraph.render(rights[1], buf);

            StatefulWidget::render(scrl_top, rights[0], buf, &mut self.comfylog_scrollbar_state);
            StatefulWidget::render(
                scrl_bottom,
                rights[1],
                buf,
                &mut self.cronlog_scrollbar_state,
            );
        }
        let legend = match self.show_debugconsole {
            true => " t : list task | k : kill all ".bold(),
            false => {
                " c : clear | Tab : switch panel | ▲ ▼ : scroll | Enter : activate | d : debug "
                    .bold()
            }
        };

        let footer = Block::new()
            .borders(Borders::TOP)
            .title_alignment(Alignment::Center)
            .title(legend);
        footer.render(parent[1], buf);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let terminal = ratatui::init();
    let app_result = App::new().run(terminal).await;
    ratatui::restore();
    app_result
}
