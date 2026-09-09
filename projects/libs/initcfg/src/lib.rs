#![no_std]
//! Parser for the `init.cfg` service manifest: a line-oriented grammar with
//! one `service NAME { ... }` block per service. Parsing is total: any
//! malformed byte, duplicate name, unknown key or cycle is an error, and a
//! failed parse refuses to start the system (docs/service-manager.md section
//! on configuration). Requires `alloc` for owned names.

extern crate alloc;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    Never,
    OnFailure,
    Always,
}
impl Restart {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "never" => Some(Self::Never),
            "on-failure" => Some(Self::OnFailure),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct ServiceCfg {
    pub name: String,
    pub elf: String,
    pub depends: Vec<String>,
    pub devices: Vec<String>,
    pub restart: Restart,
    pub max_restarts: u32,
    pub window_ms: u32,
    pub backoff_ms: u32,
    pub budget_bits: u32,
    pub critical: bool,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub services: Vec<ServiceCfg>,
}

impl Config {
    /// All service names in no particular order.
    pub fn names(&self) -> Vec<&str> {
        self.services.iter().map(|s| s.name.as_str()).collect()
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

/// Parse the manifest. `budget_bits` bounds each service's Untyped budget
/// (log2 of bytes, 12..=30).
pub fn parse(text: &str) -> Result<Config, ParseError> {
    let mut services: Vec<ServiceCfg> = Vec::new();
    let mut line_number = 0usize;
    let mut current: Option<ServiceCfg> = None;
    for raw in text.lines() {
        line_number += 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let fail = || ParseError { line: line_number };
        if let Some(rest) = line.strip_prefix("service") {
            let mut parts = rest.split_whitespace();
            let name = parts.next().ok_or_else(fail)?;
            if parts.next() != Some("{") || parts.next().is_some() {
                return Err(fail());
            }
            if services.iter().any(|s| s.name == name)
                || current.as_ref().is_some_and(|s| s.name == name)
            {
                return Err(fail());
            }
            current = Some(ServiceCfg {
                name: name.to_string(),
                elf: String::new(),
                depends: Vec::new(),
                devices: Vec::new(),
                restart: Restart::OnFailure,
                max_restarts: 5,
                window_ms: 60_000,
                backoff_ms: 100,
                budget_bits: 20,
                critical: false,
            });
            continue;
        }
        if line == "}" {
            let service = current.take().ok_or_else(fail)?;
            if service.elf.is_empty() {
                return Err(fail());
            }
            services.push(service);
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(fail)?;
        let key = key.trim();
        let value = value.trim();
        let clean = value.trim_matches('"');
        let service = current.as_mut().ok_or_else(fail)?;
        let number = |text: &str| -> Result<u32, ParseError> { text.parse().map_err(|_| fail()) };
        match key {
            "elf" => service.elf = clean.to_string(),
            "restart" => service.restart = Restart::parse(clean).ok_or_else(fail)?,
            "max_restarts" => service.max_restarts = number(clean)?,
            "window_ms" => service.window_ms = number(clean)?,
            "backoff_ms" => service.backoff_ms = number(clean)?,
            "budget" => {
                // `budget` accepts 1..9 digit strings with an optional K or M
                // suffix; the result must be a power-of-two byte count
                // expressible as 12..=30 bits (for example "2M" = 21 bits).
                let text = clean.as_bytes();
                let invalid = || ParseError { line: line_number };
                let magnitude = |digits: &[u8]| -> Result<u32, ParseError> {
                    let value: u32 = core::str::from_utf8(digits)
                        .map_err(|_| invalid())?
                        .parse()
                        .map_err(|_| invalid())?;
                    if !value.is_power_of_two() {
                        return Err(invalid());
                    }
                    Ok(value.trailing_zeros())
                };
                let bits = match text {
                    [b'1'..=b'9'] => magnitude(text)?,
                    [b'1'..=b'9', b'K'] => magnitude(&text[..1])? + 10,
                    [b'1'..=b'9', b'M'] => magnitude(&text[..1])? + 20,
                    _ => return Err(invalid()),
                };
                if !(12..=30).contains(&bits) {
                    return Err(fail());
                }
                service.budget_bits = bits;
            }
            "depends" => {
                service.depends = value
                    .split(|c: char| c == ',' || c == ' ' || c == '\t')
                    .filter(|name| !name.is_empty())
                    .map(|name| name.trim_matches('"').to_string())
                    .collect();
            }
            "device" => {
                service.devices.push(clean.to_string());
            }
            "critical" => service.critical = value == "true",
            _ => return Err(fail()),
        }
    }
    if current.is_some() {
        return Err(ParseError { line: line_number });
    }
    // Dependency references must name existing services and stay acyclic.
    for service in &services {
        for dependency in &service.depends {
            if !services.iter().any(|s| s.name == *dependency) {
                return Err(ParseError { line: 0 });
            }
        }
    }
    if services.iter().any(|s| has_cycle(s, &services)) {
        return Err(ParseError { line: 0 });
    }
    Ok(Config { services })
}

fn has_cycle(service: &ServiceCfg, services: &[ServiceCfg]) -> bool {
    fn reaches(name: &str, target: &str, services: &[ServiceCfg], depth: usize) -> bool {
        if depth > services.len() {
            return true;
        }
        services
            .iter()
            .filter(|candidate| candidate.name == name)
            .flat_map(|service| service.depends.iter())
            .any(|dependency| {
                dependency == target || reaches(dependency, target, services, depth + 1)
            })
    }
    reaches(&service.name, &service.name, services, 0)
}
