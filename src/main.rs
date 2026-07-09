mod ports;
mod ui;

use std::{
    env, io,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use anyhow::Result;
use cmux_client::{ClientConfig, CmuxClient, CmuxError, Tree};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ports::PortRow;
use ratatui::{Terminal, backend::CrosstermBackend};

const REFRESH_EVERY: Duration = Duration::from_secs(2);
const POLL_EVERY: Duration = Duration::from_millis(100);
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(500);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(8);
const DEFAULT_STATUS: &str = "↑↓ move • Enter open • k kill • r refresh";

fn main() -> Result<()> {
    // Restore the terminal before the default panic output so a panic never
    // leaves the host terminal (or the cmux sidebar PTY) stuck in raw mode +
    // alternate screen with the message swallowed.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));
    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new();
    app.refresh_ports();
    app.connect_or_schedule();

    loop {
        terminal.draw(|frame| ui::draw(frame, &app.view()))?;

        if event::poll(POLL_EVERY)?
            && let Event::Key(key) = event::read()?
            && app.handle_key(key)
        {
            break;
        }

        app.tick();
    }

    Ok(())
}

struct App {
    rows: Vec<PortRow>,
    selected: usize,
    scan_error: Option<String>,
    has_scanned: bool,
    client: Option<CmuxClient>,
    socket_path: Option<PathBuf>,
    connection: ConnectionStatus,
    message: String,
    pending_kill: Option<ports::PortRow>,
    last_refresh: Instant,
    next_reconnect: Instant,
    reconnect_delay: Duration,
}

#[derive(Debug, Clone)]
enum ConnectionStatus {
    Ready,
    Reconnecting { message: String },
}

impl App {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            scan_error: None,
            has_scanned: false,
            client: None,
            socket_path: None,
            connection: ConnectionStatus::Reconnecting {
                message: "connecting".to_string(),
            },
            message: DEFAULT_STATUS.to_string(),
            pending_kill: None,
            last_refresh: Instant::now(),
            next_reconnect: Instant::now(),
            reconnect_delay: INITIAL_RECONNECT_DELAY,
        }
    }

    fn view(&self) -> ui::View<'_> {
        let status = match &self.connection {
            ConnectionStatus::Reconnecting { message } => ui::ViewStatus::Reconnecting { message },
            ConnectionStatus::Ready if self.pending_kill.is_some() => ui::ViewStatus::ConfirmKill {
                row: self.pending_kill.as_ref().expect("checked is_some"),
            },
            ConnectionStatus::Ready => ui::ViewStatus::Ready {
                message: &self.message,
            },
        };
        ui::View {
            rows: &self.rows,
            selected: self.selected,
            scan_error: self.scan_error.as_deref(),
            status,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }

        if self.pending_kill.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm_kill(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_kill = None;
                    self.message = "Kill cancelled".to_string();
                }
                _ => {}
            }
            return false;
        }

        match key.code {
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection(-1)
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection(1)
            }
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char('k') if key.modifiers.is_empty() => self.request_kill(),
            KeyCode::Char('r') if key.modifiers.is_empty() => self.refresh_ports(),
            // cmux owns the focus escape. It must never terminate a plugin.
            KeyCode::Esc => {}
            _ => {}
        }

        false
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_refresh) >= REFRESH_EVERY {
            self.refresh_ports();
        }
        if self.client.is_none() && now >= self.next_reconnect {
            self.connect_or_schedule();
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.rows.len() - 1);
    }

    fn refresh_ports(&mut self) {
        let previous = ports::port_set(&self.rows);
        match ports::scan() {
            Ok(mut rows) => {
                if self.has_scanned {
                    ports::mark_new_ports(&mut rows, &previous);
                }
                self.rows = rows;
                self.scan_error = None;
                self.has_scanned = true;
                self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            }
            Err(error) => {
                self.rows.clear();
                self.selected = 0;
                self.scan_error = Some(error);
                self.has_scanned = true;
            }
        }
        self.last_refresh = Instant::now();
    }

    fn open_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let port = row.port;
        let result = match self.client.as_mut() {
            Some(client) => open_browser(client, port),
            None => {
                self.message = "Cannot open: cmux is disconnected".to_string();
                return;
            }
        };

        match result {
            Ok(()) => self.message = format!("Opened http://localhost:{port}"),
            Err(CmuxError::Command { message, .. }) => {
                self.message = format!("Browser error: {message}")
            }
            Err(error) => self.disconnect(format!("cmux socket dropped: {error}")),
        }
    }

    fn request_kill(&mut self) {
        // Capture the kill target at k-press: the 2s refresh can reorder rows
        // before the user confirms, so the confirm must never resolve through
        // the selected row index.
        self.pending_kill = self.rows.get(self.selected).cloned();
    }

    fn confirm_kill(&mut self) {
        let Some(target) = self.pending_kill.take() else {
            return;
        };
        // Re-validate: the process must still be listening on the same port.
        if !self
            .rows
            .iter()
            .any(|row| row.pid == target.pid && row.port == target.port)
        {
            self.message = "Process changed since prompt; kill cancelled".to_string();
            return;
        }
        if target.pid <= 1 {
            self.message = "Refusing to signal pid <= 1".to_string();
            return;
        }
        let pid = target.pid;
        let process = target.process.clone();
        match Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
        {
            Ok(status) if status.success() => {
                self.message = format!("Sent SIGTERM to {process} ({pid})");
            }
            Ok(status) => {
                self.message = format!("kill failed for {pid} (status {status})");
            }
            Err(error) => {
                self.message = format!("could not run kill for {pid}: {error}");
            }
        }
        self.last_refresh = Instant::now() - REFRESH_EVERY;
    }

    fn connect_or_schedule(&mut self) {
        let socket_path = match env::var_os("CMUX_TUI_SOCKET")
            .filter(|path| !path.is_empty())
            .or_else(|| env::var_os("CMUX_MUX_SOCKET").filter(|path| !path.is_empty()))
        {
            Some(path) => PathBuf::from(path),
            None => {
                self.socket_path = None;
                self.disconnect_with_backoff(
                    "CMUX_TUI_SOCKET is not set. Launch this plugin from cmux, or run standalone with CMUX_TUI_SOCKET=/path/to/cmux-tui.sock cargo run.".to_string(),
                );
                return;
            }
        };

        self.socket_path = Some(socket_path.clone());
        match CmuxClient::connect(ClientConfig::from_socket_path(socket_path)) {
            Ok(mut client) => match client.identify() {
                Ok(_) => {
                    self.client = Some(client);
                    self.connection = ConnectionStatus::Ready;
                    self.message = DEFAULT_STATUS.to_string();
                    self.reconnect_delay = INITIAL_RECONNECT_DELAY;
                }
                Err(error) => {
                    self.disconnect_with_backoff(format!("cmux did not respond: {error}"))
                }
            },
            Err(error) => self.disconnect_with_backoff(format!("cannot connect to cmux: {error}")),
        }
    }

    fn disconnect(&mut self, message: String) {
        self.client = None;
        self.pending_kill = None;
        self.disconnect_with_backoff(message);
    }

    fn disconnect_with_backoff(&mut self, message: String) {
        self.connection = ConnectionStatus::Reconnecting { message };
        self.next_reconnect = Instant::now() + self.reconnect_delay;
        self.reconnect_delay = (self.reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
    }
}

fn open_browser(client: &mut CmuxClient, port: u16) -> cmux_client::Result<()> {
    let tree = client.list_workspaces()?;
    let pane = focused_pane(&tree).ok_or_else(|| CmuxError::Command {
        message: "cmux has no focused pane".to_string(),
        id: None,
    })?;
    let url = format!("http://localhost:{port}");
    client.new_browser_tab(&url, Some(pane), None, None)?;
    Ok(())
}

fn focused_pane(tree: &Tree) -> Option<u64> {
    let workspace = tree
        .workspaces
        .iter()
        .find(|workspace| workspace.active)
        .or_else(|| tree.workspaces.first())?;
    let screen = workspace
        .screens
        .iter()
        .find(|screen| screen.active)
        .or_else(|| workspace.screens.first())?;
    Some(screen.active_pane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_client::{Layout, Pane, Screen, Workspace};

    #[test]
    fn resolves_focused_pane_from_active_workspace_and_screen() {
        let tree = Tree {
            workspaces: vec![Workspace {
                id: 1,
                name: "1".into(),
                active: true,
                screens: vec![Screen {
                    id: 2,
                    name: None,
                    active: true,
                    active_pane: 42,
                    layout: Layout::Leaf { pane: 42 },
                    panes: vec![Pane {
                        id: 42,
                        name: None,
                        active_tab: 0,
                        tabs: vec![],
                        dead: false,
                    }],
                }],
            }],
        };

        assert_eq!(focused_pane(&tree), Some(42));
    }
}
