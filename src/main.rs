use std::io::Result;
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task;

use command::{CmdInfo, Config, MenuCommand};
use crossterm::event::{self, Event, EventStream, KeyCode, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::palette::material::{BLUE, GREEN};
use ratatui::style::palette::tailwind::SLATE;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Masked, Span};
use ratatui::widgets::{
    Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph,
    Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap,
};
use ratatui::DefaultTerminal;
use std::time::{Duration, Instant, SystemTime};
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
    comfyui_isrunning: bool,
    cron_isrunning: bool,
    comfyui_logs: Vec<String>,
    cron_logs: Vec<String>,
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
const TODO_HEADER_STYLE: Style = Style::new().fg(SLATE.c100).bg(BLUE.c800);
const NORMAL_ROW_BG: Color = SLATE.c950;
const ALT_ROW_BG_COLOR: Color = SLATE.c900;
const SELECTED_STYLE: Style = Style::new().bg(SLATE.c800).add_modifier(Modifier::BOLD);
const TEXT_FG_COLOR: Color = SLATE.c200;
const COMPLETED_TEXT_FG_COLOR: Color = GREEN.c500;
const fn state_color(i: usize) -> Color {
    if i % 2 == 0 {
        NORMAL_ROW_BG
    } else {
        ALT_ROW_BG_COLOR
    }
}

enum CommandMode {
    COMFYUI,
    CRON,
}

#[derive(Debug, Clone)]
struct App {
    config: Config,
    selected_config: Option<CmdInfo>,
    should_exit: bool,
    scroll: u16,
    last_tick: Instant,
    menu_list: MenuList,
    logs: Arc<RwLock<LogLists>>,
    debug: String,
    comfylog_scrollbar_state: ScrollbarState,
    comfylog_scroll: usize,
}

impl App {
    const TICK_RATE: Duration = Duration::from_millis(16);
    const FRAMES_PER_SECOND: f32 = 60.0;
    const LIMIT_LOG: usize = 10;

    fn new() -> Self {
        let cnfg = Config::new();
        let config = match cnfg.read() {
            Ok(c) => c,
            Err(e) => {
                panic!("Cant read shit");
            }
        };
        let items = config
            .all
            .iter()
            .map(|c| MenuInfo::new(c.command.clone(), &c.name, &c.desc, Status::Idle))
            .collect::<Vec<MenuInfo>>();

        Self {
            config: config,
            should_exit: false,
            scroll: 0,
            selected_config: None,
            last_tick: Instant::now(),
            logs: Arc::new(RwLock::new(LogLists::default())),
            debug: String::new(),
            comfylog_scrollbar_state: ScrollbarState::new(10),
            comfylog_scroll: 0,
            menu_list: MenuList {
                items: items,
                state: ListState::default(),
            },
        }
    }
    async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let period = Duration::from_secs_f32(1.0 / Self::FRAMES_PER_SECOND);
        let mut interval = tokio::time::interval(period);
        let mut events = EventStream::new();

        while !self.should_exit {
            tokio::select! {
                _ = interval.tick() => {  terminal.draw(|frame| {
                    frame.render_widget(Clear,frame.area());
                    frame.render_widget(&mut self, frame.area())})?;},
                Some(Ok(event)) = events.next() => self.handle_events(&event),
            }
        }

        Ok(())
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

            // Wrap them up and merge them.
            let stdout = LinesStream::new(BufReader::new(stdout).lines());
            let stderr = LinesStream::new(BufReader::new(stderr).lines());
            let mut merged = StreamExt::merge(stdout, stderr);
            // Iterate through the stream line-by-line.
            while let Some(line) = merged.next().await {
                let mut logs = logs.write().await;
                if menucommand == MenuCommand::ComfyRun {
                    logs.comfyui_pid = cmd.id();
                }
                match mode {
                    CommandMode::COMFYUI => {
                        logs.comfyui_logs.push(line.unwrap());
                        if logs.comfyui_logs.len() > App::LIMIT_LOG {
                            logs.comfyui_logs.remove(0); // Remove the oldest log (first element)
                        }
                    }
                    CommandMode::CRON => {
                        logs.cron_logs.push(line.unwrap());

                        if logs.cron_logs.len() > App::LIMIT_LOG {
                            logs.cron_logs.remove(0); // Remove the oldest log (first element)
                        }
                    }
                }
            }
        });
    }

    fn kill_command(pid: u32) -> Result<()> {
        let cmd = Command::new("taskkill")
            .args(vec!["/F", "/PID", format!("{}", pid).as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }

    fn _run_command(&mut self, mode: CommandMode) -> tokio::sync::watch::Sender<bool> {
        let logs = Arc::clone(&self.logs);
        let cmdifo = match &self.selected_config {
            Some(cmd) => cmd.clone(),
            None => panic!("nothing selected"),
        };
        let (tx, mut rx) = tokio::sync::watch::channel(false); // Watch channel to signal stop
        tokio::spawn(async move {
            let mut cmd = Command::new(cmdifo.exe_path)
                .env("PYTHONUNBUFFERED", "1")
                .current_dir(cmdifo.work_dir)
                .args(cmdifo.args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("Failed to execute command");

            if let Some(stdout) = cmd.stdout.take() {
                let mut reader = tokio::io::BufReader::new(stdout).lines();
                while let Some(line) = reader.next_line().await.unwrap_or_else(|_| None) {
                    //read cancel here
                    let mut logs = logs.write().await;

                    match mode {
                        CommandMode::COMFYUI => {
                            //STOPING COMFYUI BITS
                            if *rx.borrow() {
                                logs.comfyui_logs.clear();
                                logs.comfyui_logs.push("Terminating comfyui...".to_string());
                                match cmd.kill().await {
                                    Ok(_) => {
                                        logs.comfyui_logs.clear();
                                        logs.comfyui_logs.push("Terminated...".to_string());
                                        logs.comfyui_isrunning = false;
                                    }
                                    Err(error) => {
                                        logs.comfyui_logs.clear();
                                        logs.comfyui_logs.push(format!("error {:?}", error));
                                        logs.comfyui_isrunning = false;
                                    }
                                };
                            } else {
                                logs.comfyui_logs.push(line.clone());
                            }
                            if logs.comfyui_logs.len() > App::LIMIT_LOG {
                                logs.comfyui_logs.remove(0); // Remove the oldest log (first element)
                            }
                        }
                        CommandMode::CRON => {
                            //STOPING CRON TASK
                            if logs.cron_isrunning {
                                logs.cron_logs.clear();
                                logs.cron_logs.push("Terminated...".to_string());
                                _ = cmd.kill().await.unwrap();
                                logs.cron_isrunning = false;
                            } else {
                                logs.cron_logs.push(line.clone());
                            }
                            if logs.cron_logs.len() > App::LIMIT_LOG {
                                logs.cron_logs.remove(0); // Remove the oldest log (first element)
                            }
                        }
                    }
                }
            }
            if let Some(stderr) = cmd.stderr.take() {
                let mut reader = tokio::io::BufReader::new(stderr).lines();
                while let Some(line) = reader.next_line().await.unwrap_or_else(|_| None) {
                    //read cancel here
                    let mut logs = logs.write().await;

                    match mode {
                        CommandMode::COMFYUI => {
                            if *rx.borrow() {
                                logs.comfyui_logs.clear();
                            } else {
                                logs.comfyui_logs.push(line.clone());
                            }
                            if logs.comfyui_logs.len() > App::LIMIT_LOG {
                                logs.comfyui_logs.remove(0); // Remove the oldest log (first element)
                            }
                        }
                        CommandMode::CRON => {
                            logs.cron_logs.push(line.clone());

                            if logs.cron_logs.len() > App::LIMIT_LOG {
                                logs.cron_logs.remove(0); // Remove the oldest log (first element)
                            }
                        }
                    }
                }
            }
        });
        tx
    }
    fn process_menu(self, menucommand: MenuCommand) -> Result<()> {
        let logs = Arc::clone(&self.logs);

        match menucommand {
            MenuCommand::ComfyRun => {
                tokio::spawn(async move {
                    let mut logs = logs.write().await;
                    if !logs.comfyui_isrunning {
                        self.run_command(CommandMode::COMFYUI, MenuCommand::ComfyRun);
                        logs.comfyui_isrunning = true;
                    }
                });
            }
            MenuCommand::ComfyKill => {
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

                // tokio::spawn(async move {
                //     if let Some(sender) = sender {
                //         let mut logs = logs.write().await;
                //         logs.comfyui_logs.clear();
                //         match sender.send(true) {
                //             Ok(_) => {
                //                 logs.comfyui_logs.push(format!("Sending Exit Signal"));
                //             }
                //             Err(err) => {
                //                 logs.comfyui_logs.push(format!("error: {}", err));
                //             }
                //         }
                //     }
                // });
                // tokio::spawn(async move {
                //     let mut logs = logs.write().await;
                //     logs.comfyui_logs.clear();
                //     logs.comfyui_logs.push("Termination failed...".to_string());
                //     logs.stop_comfyui = true;
                // });
            }
            MenuCommand::ComfyUpdate => {
                self.run_command(CommandMode::COMFYUI, MenuCommand::ComfyUpdate);
            }
            MenuCommand::CronRun => {
                self.run_command(CommandMode::CRON, MenuCommand::CronRun);
            }
            _ => {}
        }
        Ok(())
    }
    fn handle_events(&mut self, event: &Event) {
        let timeout = Self::TICK_RATE.saturating_sub(self.last_tick.elapsed());

        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => self.should_exit = true,
                    KeyCode::Down => {
                        self.menu_list.state.select_next();
                    }
                    KeyCode::Up => {
                        self.menu_list.state.select_previous();
                    }
                    KeyCode::Enter => {
                        if let Some(i) = self.menu_list.state.selected() {
                            self.selected_config = Some(self.config.all[i].clone());

                            for idx in 0..self.menu_list.items.len() {
                                if idx != i {
                                    self.menu_list.items[idx].status = Status::Idle;
                                } else {
                                    self.menu_list.items[idx].status = Status::Run;
                                }
                            }
                            let _ = self
                                .clone()
                                .process_menu(self.menu_list.items[i].cmd.clone());

                            // self.menu_list.items[i].status = match self.menu_list.items[i].status {
                            //     Status::Idle => {
                            //         let cmd = self.menu_list.items[i].cmd.clone();
                            //         let _ = self.process_menu(cmd).unwrap();
                            //         self.menu_list.items[i].title =
                            //             format!("::{}", self.menu_list.items[i].title);

                            //         self.debug = format!(
                            //             "{:?}",
                            //             self.menu_list
                            //                 .items
                            //                 .iter()
                            //                 .map(|i| format!("{}::{:?}\n", i.title, i.status))
                            //                 .collect::<String>()
                            //         );
                            //         Status::Run
                            //     }
                            //     _ => Status::Run,
                            // }
                        }
                    }

                    KeyCode::Char('j') => {
                        self.comfylog_scroll = self.comfylog_scroll.saturating_add(1);
                        self.comfylog_scrollbar_state =
                            self.comfylog_scrollbar_state.position(self.comfylog_scroll);
                    }
                    KeyCode::Char('k') => {
                        self.comfylog_scroll = self.comfylog_scroll.saturating_sub(1);
                        self.comfylog_scrollbar_state =
                            self.comfylog_scrollbar_state.position(self.comfylog_scroll);
                    }

                    _ => {}
                }
            }
        }
    }
    /// Update the app state on each tick.
    fn on_tick(&mut self) {
        self.scroll = (self.scroll + 1) % 10;
    }
    fn render_list(&mut self, area: Rect, buf: &mut Buffer) {
        let block = Block::new().title("Menu").borders(Borders::ALL);

        let menu: Vec<ListItem> = self
            .menu_list
            .items
            .iter()
            .enumerate()
            .map(|(i, menu)| {
                let color = state_color(i);
                match menu.status {
                    Status::Idle => ListItem::from(menu).fg(color),
                    Status::Run => ListItem::from(menu).fg(color).bg(ALT_ROW_BG_COLOR),
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
    fn render_selected_menu(&self, area: Rect, buf: &mut Buffer) {
        let info = if let Some(i) = self.menu_list.state.selected() {
            format!("{}", self.menu_list.items[i].info)
        } else {
            "Nothing selected...".to_string()
        };
        // We show the list item's info under the list in this paragraph
        let block = Block::new()
            .title("Info")
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1));

        // We can now render the item info
        Paragraph::new(info)
            .block(block)
            .fg(TEXT_FG_COLOR)
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }
}
impl Widget for &mut App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let horizontal =
            Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)]);
        let [left, right] = horizontal.areas(area);
        let lefts =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(left);

        let rights =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(right);
        self.render_list(lefts[0], buf);

        let logs = task::block_in_place(|| {
            // This block executes in a thread pool for synchronous code
            // Here we need to access the lock asynchronously
            let logs = tokio::runtime::Handle::current().block_on(self.logs.read()); // block_on is used to await the future in sync context
            logs // Process the logs here
        });
        let log_len = logs.comfyui_logs.len();

        let comfyui_log_lines = logs
            .comfyui_logs
            .iter()
            .map(|s| Line::from(Span::raw(s.clone())))
            .collect::<Vec<_>>();

        let cron_log_lines = logs
            .cron_logs
            .iter()
            .map(|s| Line::from(Span::raw(s.clone())))
            .collect::<Vec<_>>();

        let comfyuilog_paragraph = Paragraph::new(comfyui_log_lines)
            .block(Block::new().title("ComfyUI Logs").borders(Borders::ALL))
            .scroll((self.comfylog_scroll as u16, 0))
            .wrap(Wrap { trim: true });

        comfyuilog_paragraph.render(rights[0], buf);

        let cronuilog_paragraph = Paragraph::new(cron_log_lines)
            .block(Block::new().title("Cron Logs").borders(Borders::ALL))
            .scroll((self.comfylog_scroll as u16, 0))
            .wrap(Wrap { trim: true });
        cronuilog_paragraph.render(rights[1], buf);

        self.comfylog_scrollbar_state = self.comfylog_scrollbar_state.content_length(log_len);
        let scrl = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        self.render_selected_menu(lefts[1], buf);
        StatefulWidget::render(scrl, rights[0], buf, &mut self.comfylog_scrollbar_state);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let terminal = ratatui::init();
    let app_result = App::new().run(terminal).await;
    ratatui::restore();
    app_result
}
