use std::{
    collections::HashSet,
    io,
    process::{Command, Output},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortRow {
    pub port: u16,
    pub process: String,
    pub pid: u32,
    pub is_new: bool,
}

impl PortRow {
    pub fn label(&self) -> String {
        format!(":{}  {}  {}", self.port, self.process, self.pid)
    }

    pub fn is_common_dev_port(&self) -> bool {
        (3000..=9999).contains(&self.port)
    }
}

pub fn scan() -> Result<Vec<PortRow>, String> {
    let output = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .map_err(lsof_start_error)?;
    parse_lsof_result(output)
}

fn lsof_start_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        "lsof is required to list listening ports; install lsof and press r to retry".to_string()
    } else {
        format!("could not start lsof: {error}")
    }
}

fn parse_lsof_result(output: Output) -> Result<Vec<PortRow>, String> {
    // lsof returns 1 when its query has no matches, so an empty result without
    // a diagnostic is valid even when the process status is not successful.
    if !output.status.success() && !output.stderr.is_empty() {
        return Err(format!(
            "lsof failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_lsof(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_lsof(output: &str) -> Result<Vec<PortRow>, String> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();

    for line in output.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 3 || columns[0] == "COMMAND" {
            continue;
        }

        // COMMAND may contain spaces (lsof truncates to 9 chars but keeps
        // them, e.g. "Google Ch"), shifting every column. Anchor on the NODE
        // column ("TCP"): PID sits 6 columns before it (USER FD TYPE DEVICE
        // SIZE/OFF in between) and everything before that is the command.
        let Some(tcp_index) = columns.iter().position(|column| *column == "TCP") else {
            continue;
        };
        if tcp_index < 7 {
            continue;
        }
        let Ok(pid) = columns[tcp_index - 6].parse::<u32>() else {
            continue;
        };
        let process = columns[..tcp_index - 6].join(" ");
        let Some(address) = columns.get(tcp_index + 1) else {
            continue;
        };
        let Some(port) = parse_port(address) else {
            continue;
        };

        if seen.insert((pid, port)) {
            rows.push(PortRow {
                port,
                process,
                pid,
                is_new: false,
            });
        }
    }

    sort_rows(&mut rows);
    Ok(rows)
}

fn parse_port(address: &str) -> Option<u16> {
    address.rsplit(':').next()?.parse().ok()
}

pub fn sort_rows(rows: &mut [PortRow]) {
    rows.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then_with(|| a.process.cmp(&b.process))
            .then_with(|| a.pid.cmp(&b.pid))
    });
}

pub fn mark_new_ports(rows: &mut [PortRow], previous: &HashSet<u16>) {
    for row in rows {
        row.is_new = !previous.contains(&row.port);
    }
}

pub fn port_set(rows: &[PortRow]) -> HashSet<u16> {
    rows.iter().map(|row| row.port).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSOF_FIXTURE: &str = r#"COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
node      431 alice  21u  IPv6 0xbeef      0t0  TCP *:3000 (LISTEN)
python3   812 alice   4u  IPv4 0xfeed      0t0  TCP 127.0.0.1:8123 (LISTEN)
node      431 alice  22u  IPv4 0xcafe      0t0  TCP 0.0.0.0:3000 (LISTEN)
server    900 alice   8u  IPv6 0xabcd      0t0  TCP [::1]:443 (LISTEN)
"#;

    #[test]
    fn parses_commands_with_spaces() {
        // lsof truncates COMMAND to 9 chars but preserves inner spaces,
        // shifting every whitespace-split column.
        let output = "COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME\n\
Google Ch  321 me   45u  IPv4 0xdead      0t0  TCP 127.0.0.1:9222 (LISTEN)\n";
        let rows = parse_lsof(output).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].process, "Google Ch");
        assert_eq!(rows[0].pid, 321);
        assert_eq!(rows[0].port, 9222);
    }

    #[test]
    fn parses_ipv4_ipv6_and_deduplicates_pid_port() {
        let rows = parse_lsof(LSOF_FIXTURE).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].label(), ":443  server  900");
        assert_eq!(rows[1].label(), ":3000  node  431");
        assert_eq!(rows[2].label(), ":8123  python3  812");
    }

    #[test]
    fn sorts_equal_ports_by_process_then_pid() {
        let mut rows = vec![
            PortRow {
                port: 8080,
                process: "zeta".into(),
                pid: 2,
                is_new: false,
            },
            PortRow {
                port: 3000,
                process: "node".into(),
                pid: 3,
                is_new: false,
            },
            PortRow {
                port: 8080,
                process: "alpha".into(),
                pid: 9,
                is_new: false,
            },
        ];

        sort_rows(&mut rows);
        let labels = rows.iter().map(PortRow::label).collect::<Vec<_>>();
        assert_eq!(
            labels,
            [":3000  node  3", ":8080  alpha  9", ":8080  zeta  2"]
        );
    }

    #[test]
    fn marks_only_ports_absent_from_previous_tick() {
        let previous = HashSet::from([3000, 8123]);
        let mut rows = parse_lsof(LSOF_FIXTURE).unwrap();
        mark_new_ports(&mut rows, &previous);

        assert!(rows.iter().find(|row| row.port == 443).unwrap().is_new);
        assert!(!rows.iter().find(|row| row.port == 3000).unwrap().is_new);
        assert!(!rows.iter().find(|row| row.port == 8123).unwrap().is_new);
    }
}
